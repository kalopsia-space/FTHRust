//! The in-process export runner (`core/export_queue.py` `run_export`,
//! `core/export_lifecycle.py`, `_validate_export_output`).

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use fthrust_core::editor::snapshot::Effects;
use fthrust_core::export::filters::{MixRole, MixSource};
use fthrust_core::export::graph::{ExportSpec, plan};
use fthrust_media::export::{ExportState, Job, Quality, run};

/// A 160×90 30 fps clip with `tracks` AAC tones (0 = video only).
fn clip(dir: &Path, name: &str, seconds: u32, tracks: usize) -> Option<PathBuf> {
    use fthrust_core::capture_config::{Codec, EncoderPref};
    use fthrust_media::clip_writer::{ClipAudio, ClipVideo, write_clip};
    use fthrust_media::encoder::{Encoder, EncoderConfig};
    use fthrust_media::ring::EncodedRing;
    support::skip_unless_encoder(&["libopenh264", "libx264"])?;
    const FPS: u32 = 30;
    const SECOND: i64 = 1_000_000_000;
    let cfg = EncoderConfig {
        src_width: 160,
        src_height: 90,
        enc_width: 160,
        enc_height: 90,
        fps: FPS,
        bitrate_kbps: 300,
        codec: Codec::H264,
        encoder: EncoderPref::Software,
        preset: 1,
    };
    let mut encoder = Encoder::open(&cfg).unwrap();
    let ring = EncodedRing::new(u64::from(seconds) * 1000 + 5000, FPS, 0);
    let mut out = Vec::new();
    for i in 0..seconds * FPS {
        let bgra = support::synthetic_bgra(160, 90, i);
        encoder
            .encode(
                &bgra,
                160 * 4,
                160,
                90,
                i64::from(i) * SECOND / i64::from(FPS),
                &mut out,
            )
            .unwrap();
    }
    encoder.flush(&mut out).unwrap();
    for p in out {
        ring.push(p);
    }
    let snapshot = ring
        .snapshot(seconds * 1000, i64::from(seconds) * SECOND, 0)
        .unwrap();
    let video = ClipVideo {
        snapshot: &snapshot,
        codec_id: encoder.codec_id(),
        extradata: &encoder.extradata(),
        width: 160,
        height: 90,
        bitrate_kbps: 300,
        separate_audio: tracks > 1,
    };
    let tones: Vec<Vec<f32>> = (0..tracks)
        .map(|t| {
            let hz = 440.0 * (t + 1) as f32;
            (0..u64::from(seconds) * 48_000)
                .flat_map(|i| {
                    let v = (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                    [v, v]
                })
                .collect()
        })
        .collect();
    let titles = ["Desktop", "Mic", "Game"];
    let audio: Vec<ClipAudio<'_>> = tones
        .iter()
        .enumerate()
        .map(|(i, s)| ClipAudio {
            samples: s,
            sample_rate: 48_000,
            channels: 2,
            title: titles[i],
        })
        .collect();
    let path = dir.join(name);
    write_clip(&path, &video, &audio, None).unwrap();
    Some(path)
}

fn spec(ranges: &[(f64, f64)]) -> ExportSpec {
    ExportSpec {
        ranges_s: ranges.to_vec(),
        crop: None,
        effects: Effects::default(),
        stretch: 1.0,
        speed: 1.0,
        preserve_pitch: true,
        sources: vec![],
        master_percent: 100,
        has_manifest_tracks: false,
        has_audio: true,
        watermark: None,
        audio_bitrate_kbps: 320,
        extra_video_filters: vec![],
        force_video_encode: false,
        force_audio_encode: false,
    }
}

fn job(source: &Path, output: PathBuf, s: &ExportSpec, quality: Quality) -> Job {
    Job {
        source: source.to_path_buf(),
        output,
        plan: plan(s).unwrap(),
        quality,
        video_bitrate_kbps: None,
        max_rate_kbps: None,
        expected_duration_s: 1.0,
        target_size_mb: None,
        fail_first_encoder: false,
    }
}

struct Probe {
    duration_s: f64,
    /// `(kind, seconds)` per stream.
    streams: Vec<(String, f64)>,
    video: usize,
    audio: usize,
    video_codec: String,
    audio_codec: String,
}

fn probe(path: &Path) -> Probe {
    ffmpeg_next::init().unwrap();
    let ictx = ffmpeg_next::format::input(&path).unwrap();
    let mut p = Probe {
        duration_s: ictx.duration() as f64 / 1_000_000.0,
        streams: Vec::new(),
        video: 0,
        audio: 0,
        video_codec: String::new(),
        audio_codec: String::new(),
    };
    for s in ictx.streams() {
        let id = s.parameters().id();
        let tb = s.time_base();
        p.streams.push((
            format!("{:?}", s.parameters().medium()),
            s.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()),
        ));
        match s.parameters().medium() {
            ffmpeg_next::media::Type::Video => {
                p.video += 1;
                p.video_codec = format!("{id:?}");
            }
            ffmpeg_next::media::Type::Audio => {
                p.audio += 1;
                p.audio_codec = format!("{id:?}");
            }
            _ => {}
        }
    }
    p
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".partial."))
        .collect()
}

fn run_job(j: &Job) -> (fthrust_media::export::ExportResult, Vec<String>, Vec<f64>) {
    let cancel = AtomicBool::new(false);
    let mut statuses = Vec::new();
    let mut progress = Vec::new();
    let r = run(j, &cancel, &mut |s| statuses.push(s.to_owned()), &mut |p| {
        progress.push(p)
    });
    (r, statuses, progress)
}

#[test]
fn export_two_ranges_with_mix_and_speed() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "two.mp4", 4, 2) else {
        return;
    };
    let mut s = spec(&[(0.0, 1.0), (2.0, 3.0)]);
    s.speed = 2.0;
    s.has_manifest_tracks = true;
    s.sources = (0..2)
        .map(|i| MixSource {
            audio_index: Some(i),
            available: true,
            role: MixRole::Direct,
            gain_percent: 100,
            muted: false,
        })
        .collect();
    let out = tmp.path().join("out.mp4");
    let (r, statuses, progress) = run_job(&job(&src, out.clone(), &s, Quality::Quality));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    let p = probe(&out);
    assert!(
        (p.duration_s - 1.0).abs() < 0.15,
        "duration {} {:?}",
        p.duration_s,
        p.streams
    );
    assert_eq!((p.video, p.audio), (1, 1));
    assert!(
        statuses.iter().any(|s| s.starts_with("Exporting · ")),
        "{statuses:?}"
    );
    assert!(progress.iter().any(|&v| v >= 50.0), "{progress:?}");
    assert!(progress.iter().all(|&v| v <= 99.0));
    assert!(leftovers(tmp.path()).is_empty());
}

#[test]
fn stream_copy_single_range() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "c.mp4", 4, 1) else {
        return;
    };
    let out = tmp.path().join("copy.mp4");
    let (r, statuses, _) = run_job(&job(
        &src,
        out.clone(),
        &spec(&[(1.0, 3.0)]),
        Quality::Quality,
    ));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert!(
        statuses.contains(&"Exporting · stream copy".to_owned()),
        "{statuses:?}"
    );
    let p = probe(&out);
    assert!(
        (p.duration_s - 2.0).abs() < 0.6,
        "duration {}",
        p.duration_s
    );
    assert_eq!(p.video_codec, "H264");
    assert_eq!(p.audio, 1);
}

#[test]
fn lossless_writes_mkv_ffv1_flac() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "l.mp4", 2, 1) else {
        return;
    };
    let mut s = spec(&[(0.0, 1.0)]);
    s.crop = Some((0, 0, 80, 60));
    let out = tmp.path().join("l.mkv");
    let (r, statuses, _) = run_job(&job(&src, out.clone(), &s, Quality::Lossless));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert!(
        statuses.contains(&"Exporting · lossless".to_owned()),
        "{statuses:?}"
    );
    let p = probe(&out);
    assert_eq!(p.video_codec, "FFV1");
    assert_eq!(p.audio_codec, "FLAC");
}

#[test]
fn cancel_removes_staged() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "c.mp4", 4, 1) else {
        return;
    };
    let mut s = spec(&[(0.0, 4.0)]);
    s.crop = Some((0, 0, 80, 60));
    let out = tmp.path().join("cancelled.mp4");
    let cancel = AtomicBool::new(false);
    let j = job(&src, out.clone(), &s, Quality::Quality);
    let r = run(&j, &cancel, &mut |_| {}, &mut |_| {
        cancel.store(true, Ordering::Relaxed)
    });
    assert_eq!(r.state, ExportState::Cancelled);
    assert_eq!(r.detail, "Export cancelled");
    assert!(!out.exists());
    assert!(leftovers(tmp.path()).is_empty());
}

#[test]
fn missing_source_fails_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "gone.mp4", 2, 1) else {
        return;
    };
    let j = job(
        &src,
        tmp.path().join("o.mp4"),
        &spec(&[(0.0, 1.0)]),
        Quality::Quality,
    );
    std::fs::remove_file(&src).unwrap();
    let (r, _, _) = run_job(&j);
    assert_eq!(r.state, ExportState::Failed);
    assert!(!r.detail.is_empty());
    assert!(!tmp.path().join("o.mp4").exists());
    assert!(leftovers(tmp.path()).is_empty());
}

#[test]
fn read_only_folder_fails_cleanly() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "a.mp4", 2, 1) else {
        return;
    };
    let ro = tmp.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (r, _, _) = run_job(&job(
        &src,
        ro.join("o.mp4"),
        &spec(&[(0.0, 1.0)]),
        Quality::Quality,
    ));
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.state, ExportState::Failed);
    assert!(
        r.detail.to_lowercase().contains("permission"),
        "{}",
        r.detail
    );
    assert!(std::fs::read_dir(&ro).unwrap().next().is_none());
}

#[test]
fn no_audio_and_all_muted() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(silent) = clip(tmp.path(), "v.mp4", 2, 0) else {
        return;
    };
    let mut s = spec(&[(0.0, 0.5), (1.0, 1.5)]);
    s.has_audio = false;
    let out = tmp.path().join("v_out.mp4");
    let (r, _, _) = run_job(&job(&silent, out.clone(), &s, Quality::Fast));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert_eq!(probe(&out).audio, 0);
    let Some(two) = clip(tmp.path(), "m.mp4", 2, 2) else {
        return;
    };
    let mut s = spec(&[(0.0, 1.0)]);
    s.has_manifest_tracks = true;
    s.sources = (0..2)
        .map(|i| MixSource {
            audio_index: Some(i),
            available: true,
            role: MixRole::Direct,
            gain_percent: 100,
            muted: true,
        })
        .collect();
    let out = tmp.path().join("m_out.mp4");
    let (r, _, _) = run_job(&job(&two, out.clone(), &s, Quality::Fast));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert_eq!(probe(&out).audio, 1, "a silent mix is still a track");
}

#[test]
fn hardware_failure_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "h.mp4", 2, 1) else {
        return;
    };
    let mut s = spec(&[(0.0, 1.0)]);
    s.crop = Some((0, 0, 80, 60));
    let mut j = job(&src, tmp.path().join("h_out.mp4"), &s, Quality::Fast);
    j.fail_first_encoder = true;
    let (r, statuses, _) = run_job(&j);
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert!(
        statuses.contains(&"Software fallback · software".to_owned()),
        "{statuses:?}"
    );
}

#[test]
fn media_facts_describe_the_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "f.mp4", 2, 1) else {
        return;
    };
    let f = fthrust_media::export::media_facts(&src).unwrap();
    assert_eq!((f.width, f.height), (160, 90));
    assert_eq!(f.video_codec, "h264");
    assert!((f.fps - 30.0).abs() < 0.5, "{}", f.fps);
    assert!((f.duration_s - 2.0).abs() < 0.2, "{}", f.duration_s);
    assert!(f.has_audio);
    assert_eq!(f.audio_codec, "aac");
    assert!(f.audio_kbps > 0);
    assert_eq!(f.size_bytes, std::fs::metadata(&src).unwrap().len());
    let missing = fthrust_media::export::media_facts(&tmp.path().join("nope.mp4")).unwrap_err();
    assert!(
        missing.starts_with("Media inspection failed: "),
        "{missing}"
    );
}

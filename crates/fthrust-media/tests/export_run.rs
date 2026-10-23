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
        output_fps: None,
        decode_from_s: 0.0,
        inactivity_s: None,
        copy_metadata: false,
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

// ---------------------------------------------------------------------------
// Review fixes.

/// Video frames and the average rate of the first video stream.
fn video_frames(path: &Path) -> (usize, f64) {
    ffmpeg_next::init().unwrap();
    let mut ictx = ffmpeg_next::format::input(&path).unwrap();
    let v = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap();
    let (index, rate) = (v.index(), v.avg_frame_rate());
    let n = ictx.packets().filter(|(s, _)| s.index() == index).count();
    (
        n,
        f64::from(rate.numerator()) / f64::from(rate.denominator().max(1)),
    )
}

#[test]
fn preset_frame_rate_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "f.mp4", 2, 1) else {
        return;
    };
    let mut s = spec(&[(0.0, 2.0)]);
    s.extra_video_filters = vec!["fps=15".into(), "scale=trunc(iw/2)*2:trunc(ih/2)*2".into()];
    s.force_video_encode = true;
    let out = tmp.path().join("f15.mp4");
    let mut j = job(&src, out.clone(), &s, Quality::Fast);
    j.output_fps = Some(15.0);
    let (r, _, _) = run_job(&j);
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    let (frames, fps) = video_frames(&out);
    assert!((fps - 15.0).abs() < 0.5, "{fps} fps");
    assert!((28..=32).contains(&frames), "{frames} frames");
}

/// Copies every stream of `src` into a Matroska file (1/1000 time bases).
fn remux_mkv(src: &Path, dst: &Path) {
    use ffmpeg_next as ff;
    ff::init().unwrap();
    let mut ictx = ff::format::input(&src).unwrap();
    let mut octx = ff::format::output(&dst).unwrap();
    for ist in ictx.streams() {
        let mut ost = octx
            .add_stream(ff::encoder::find(ff::codec::Id::None))
            .unwrap();
        ost.set_parameters(ist.parameters());
        unsafe { (*ost.parameters().as_mut_ptr()).codec_tag = 0 };
    }
    octx.write_header().unwrap();
    for (ist, mut pkt) in ictx.packets() {
        let otb = octx.stream(ist.index()).unwrap().time_base();
        pkt.rescale_ts(ist.time_base(), otb);
        pkt.set_position(-1);
        pkt.write_interleaved(&mut octx).unwrap();
    }
    octx.write_trailer().unwrap();
}

#[test]
fn audio_from_a_matroska_source_keeps_its_length() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "a.mp4", 2, 1) else {
        return;
    };
    let mkv = tmp.path().join("a.mkv");
    remux_mkv(&src, &mkv);
    let mut s = spec(&[(0.0, 2.0)]);
    s.force_audio_encode = true;
    let out = tmp.path().join("a_out.mp4");
    let (r, _, _) = run_job(&job(&mkv, out.clone(), &s, Quality::Fast));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    let p = probe(&out);
    let audio = p.streams.iter().find(|(k, _)| k == "Audio").unwrap().1;
    assert!((audio - 2.0).abs() < 0.2, "audio {audio} s {:?}", p.streams);
}

#[test]
fn late_ranges_of_a_long_clip_do_not_time_out() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "long.mp4", 40, 1) else {
        return;
    };
    let s = spec(&[(37.0, 38.0), (38.5, 39.5)]);
    let out = tmp.path().join("late.mp4");
    let mut j = job(&src, out.clone(), &s, Quality::Fast);
    j.decode_from_s = 37.0;
    // Decoding 37 s before the first output takes longer than this.
    j.inactivity_s = Some(0.05);
    let (r, _, _) = run_job(&j);
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    let p = probe(&out);
    assert!(
        (p.duration_s - 2.0).abs() < 0.2,
        "{} {:?}",
        p.duration_s,
        p.streams
    );
}

#[test]
fn a_corrupt_packet_does_not_abort_the_export() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "c.mp4", 3, 1) else {
        return;
    };
    // Garble a run of bytes in the middle of the media data.
    let mut bytes = std::fs::read(&src).unwrap();
    let len = bytes.len();
    let mut x: u32 = 0x1234_5678;
    for k in 3..9 {
        let at = len * k / 10;
        for b in &mut bytes[at..at + 1500] {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8;
        }
    }
    let bad = tmp.path().join("bad.mp4");
    std::fs::write(&bad, bytes).unwrap();
    let mut s = spec(&[(0.0, 3.0)]);
    s.force_video_encode = true;
    s.force_audio_encode = true;
    let out = tmp.path().join("bad_out.mp4");
    let (r, _, _) = run_job(&job(&bad, out.clone(), &s, Quality::Fast));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    assert!(out.exists());
}

// ---------------------------------------------------------------------------
// Post-save rewrites (plan 6d).

/// RGB at `(x, y)` of the frame nearest `t` seconds.
fn pixel_at(path: &Path, t: f64, x: u32, y: u32) -> [u8; 3] {
    use ffmpeg_next as ff;
    ff::init().unwrap();
    let mut ictx = ff::format::input(&path).unwrap();
    let s = ictx.streams().best(ff::media::Type::Video).unwrap();
    let (index, tb) = (s.index(), s.time_base());
    let mut dec = ff::codec::context::Context::from_parameters(s.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let mut frame = ff::frame::Video::empty();
    let mut found = None;
    'outer: for (st, p) in ictx.packets() {
        if st.index() != index {
            continue;
        }
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut frame).is_ok() {
            let ft = frame.pts().unwrap_or(0) as f64 * f64::from(tb.numerator())
                / f64::from(tb.denominator());
            if ft >= t {
                found = Some(frame.clone());
                break 'outer;
            }
        }
    }
    let f = found.expect("a frame at t");
    let mut sc = ff::software::scaling::Context::get(
        f.format(),
        f.width(),
        f.height(),
        ff::format::Pixel::RGB24,
        f.width(),
        f.height(),
        ff::software::scaling::Flags::BILINEAR,
    )
    .unwrap();
    let mut rgb = ff::frame::Video::empty();
    sc.run(&f, &mut rgb).unwrap();
    let i = y as usize * rgb.stride(0) + x as usize * 3;
    let d = rgb.data(0);
    [d[i], d[i + 1], d[i + 2]]
}

fn close_rgb(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= tol)
}

fn rewrite_job(src: &Path, out: PathBuf, plan: fthrust_core::export::graph::GraphPlan) -> Job {
    Job {
        source: src.to_path_buf(),
        output: out,
        plan,
        quality: Quality::Quality,
        video_bitrate_kbps: None,
        max_rate_kbps: None,
        expected_duration_s: 2.0,
        target_size_mb: None,
        output_fps: None,
        decode_from_s: 0.0,
        copy_metadata: true,
        inactivity_s: None,
        fail_first_encoder: false,
    }
}

/// Audio stream titles in order.
fn audio_titles(path: &Path) -> Vec<String> {
    let ictx = ffmpeg_next::format::input(&path).unwrap();
    ictx.streams()
        .filter(|s| s.parameters().medium() == ffmpeg_next::media::Type::Audio)
        .map(|s| s.metadata().get("handler_name").unwrap_or("").to_owned())
        .collect()
}

#[test]
fn rewrite_keeps_tracks_and_titles() {
    use fthrust_core::finalize::{CAMERA_DEFAULT, ImageLayer, Rect, plan as burn};
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "m.mp4", 2, 3) else {
        return;
    };
    let png = tmp.path().join("red.png");
    image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255]))
        .save(&png)
        .unwrap();
    let layer = ImageLayer {
        path: png,
        opacity: 100,
        fill: true,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.6,
        },
    };
    let p = burn(&[layer], None, CAMERA_DEFAULT, (160, 90)).unwrap();
    let out = tmp.path().join("m_out.mp4");
    let (r, _, _) = run_job(&rewrite_job(&src, out.clone(), p));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    let before = probe(&src);
    let after = probe(&out);
    assert_eq!(after.audio, 3);
    assert_eq!(audio_titles(&src), ["Desktop", "Mic", "Game"]);
    assert_eq!(audio_titles(&out), audio_titles(&src));
    assert_eq!(after.audio_codec, before.audio_codec, "audio copied");
    assert!((after.duration_s - before.duration_s).abs() < 0.2);
    // The layer is burned: red inside its box, the clip outside it.
    let inside = pixel_at(&out, 1.0, 20, 20);
    assert!(
        inside[0] > 200 && inside[1] < 60 && inside[2] < 60,
        "{inside:?}"
    );
    assert!(close_rgb(
        pixel_at(&out, 1.0, 140, 80),
        pixel_at(&src, 1.0, 140, 80),
        40
    ));
}

#[test]
fn camera_movie_overlay_burns() {
    use fthrust_core::finalize::{Rect, plan as burn};
    use fthrust_media::camera::{CameraRecorder, Source};
    let tmp = tempfile::tempdir().unwrap();
    let Some(src) = clip(tmp.path(), "c.mp4", 2, 1) else {
        return;
    };
    let rec = CameraRecorder::start(Source::synthetic(160, 120, 15)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2300));
    let cam = tmp.path().join("camera.mkv");
    rec.write_segment(std::time::Instant::now(), 2.0, &cam)
        .unwrap();
    rec.stop();
    let rect = Rect {
        x: 0.5,
        y: 0.4,
        w: 0.5,
        h: 0.6,
    };
    let p = burn(&[], Some(&cam), rect, (160, 90)).unwrap();
    let out = tmp.path().join("c_out.mp4");
    let (r, _, _) = run_job(&rewrite_job(&src, out.clone(), p));
    assert_eq!(r.state, ExportState::Completed, "{}", r.detail);
    // Box at x 80..160, y 36..90: the test pattern replaces the clip there.
    let differs = [(100, 50), (120, 60), (140, 80)]
        .iter()
        .filter(|(x, y)| !close_rgb(pixel_at(&out, 1.0, *x, *y), pixel_at(&src, 1.0, *x, *y), 40))
        .count();
    assert!(differs >= 2, "camera not visible");
    assert!(close_rgb(
        pixel_at(&out, 1.0, 20, 10),
        pixel_at(&src, 1.0, 20, 10),
        40
    ));
}

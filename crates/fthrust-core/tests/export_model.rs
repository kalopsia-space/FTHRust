//! The export model. Expected strings were printed by the reference's own
//! functions (`ui/clip_viewer.py` `_atempo_chain`, `_pitch_changed_chain`,
//! `_speed_text`, `_video_filter_chain`, `_build_export_cmd`;
//! `core/playback_mix_model.py` `ffmpeg_mix_filter`;
//! `core/capture_card_watermark.py`; `core/export_profiles.py`
//! `plan_export`) under `.venv-reference`.

use fthrust_core::editor::snapshot::Effects;
use fthrust_core::export::filters::{
    MixRole, MixSource, atempo_chain, mix_filter, pitch_changed_chain, speed_text,
    video_filter_chain,
};
use fthrust_core::export::graph::{AudioOut, ExportSpec, MapRef, VideoOut, plan};
use fthrust_core::export::naming::{
    export_name, share_name, speed_adjusted_duration, staged_name, unique,
};
use fthrust_core::export::plan::{MediaFacts, plan_export};
use fthrust_core::export::watermark::{WatermarkStyle, ffmpeg_color, filter_path, filters};
use fthrust_core::export_presets::ExportPreset;

#[test]
fn speed_and_pitch_chains() {
    assert_eq!(atempo_chain(0.25).as_deref(), Some("atempo=0.5,atempo=0.5"));
    assert_eq!(atempo_chain(0.3).as_deref(), Some("atempo=0.5,atempo=0.6"));
    assert_eq!(atempo_chain(0.75).as_deref(), Some("atempo=0.75"));
    assert_eq!(atempo_chain(1.0), None);
    assert_eq!(atempo_chain(1.5).as_deref(), Some("atempo=1.5"));
    assert_eq!(
        atempo_chain(3.0).as_deref(),
        Some("atempo=2"),
        "speed clamps to 2"
    );
    assert_eq!(
        pitch_changed_chain(0.5).as_deref(),
        Some("aresample=48000,asetrate=24000,aresample=48000")
    );
    assert_eq!(pitch_changed_chain(1.0), None);
    assert_eq!(
        pitch_changed_chain(1.5).as_deref(),
        Some("aresample=48000,asetrate=72000,aresample=48000")
    );
    assert_eq!(speed_text(1.25), "1.25");
    assert_eq!(speed_text(2.0), "2");
    assert_eq!(speed_text(0.333333), "0.333333");
    assert_eq!(speed_text(1.0), "1");
}

fn fx() -> Effects {
    Effects {
        exposure: 10,
        contrast: -20,
        saturation: 30,
        temperature: -15,
        sharpness: 40,
        vignette: 50,
    }
}

#[test]
fn video_chain_order_and_format() {
    assert_eq!(
        video_filter_chain(Some((10, 20, 641, 361)), &fx(), 1.5, false, 0.5),
        vec![
            "crop=640:360:10:20",
            "curves=all=0.000/0.090 0.500/0.530 1.000/0.970",
            "hue=s=1.300",
            "colorbalance=rs=-0.045:bs=0.045",
            "unsharp=5:5:0.600:5:5:0.0",
            "vignette=PI/12.50",
            "scale=trunc(iw*1.5000/2)*2:trunc(ih/2)*2",
            "setsar=1",
            "setpts=PTS/0.5",
        ]
    );
    assert!(video_filter_chain(None, &Effects::default(), 1.0, false, 1.0).is_empty());
}

fn two_sources(desk: u8, mic: u8, mic_muted: bool) -> Vec<MixSource> {
    vec![
        MixSource {
            audio_index: Some(0),
            available: true,
            role: MixRole::Direct,
            gain_percent: desk,
            muted: false,
        },
        MixSource {
            audio_index: Some(1),
            available: true,
            role: MixRole::Direct,
            gain_percent: mic,
            muted: mic_muted,
        },
    ]
}

#[test]
fn mix_graph() {
    let (f, out) = mix_filter(&two_sources(100, 60, true), 80, "aout");
    assert_eq!(
        f,
        vec![
            "[0:a:0]volume=1.000000[src0]",
            "[0:a:1]volume=0.000000[src1]",
            "[src0][src1]amix=inputs=2:normalize=0,volume=1.000000,alimiter=limit=0.98:level=disabled,volume=0.800000[aout]",
        ]
    );
    assert_eq!(out.as_deref(), Some("[aout]"));
    let (f, _) = mix_filter(&two_sources(70, 40, false), 100, "amixed");
    assert_eq!(
        f[2],
        "[src0][src1]amix=inputs=2:normalize=0,volume=0.707107,alimiter=limit=0.98:level=disabled,volume=1.000000[amixed]"
    );
    let none = vec![MixSource {
        audio_index: None,
        available: true,
        role: MixRole::Direct,
        gain_percent: 100,
        muted: false,
    }];
    assert_eq!(mix_filter(&none, 100, "aout"), (vec![], None));
    let base = MixSource {
        audio_index: Some(0),
        available: true,
        role: MixRole::Base,
        gain_percent: 10,
        muted: true,
    };
    let delta = MixSource {
        audio_index: Some(1),
        available: true,
        role: MixRole::Delta,
        gain_percent: 50,
        muted: false,
    };
    let (f, _) = mix_filter(&[base, delta], 100, "aout");
    assert_eq!(f[0], "[0:a:0]volume=1.000000[src0]", "base is always unity");
    assert_eq!(f[1], "[0:a:1]volume=-0.500000[src1]", "delta is gain - 1");
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
        master_percent: 80,
        has_manifest_tracks: false,
        has_audio: true,
        watermark: None,
        audio_bitrate_kbps: 320,
        extra_video_filters: vec![],
        force_video_encode: false,
        force_audio_encode: false,
    }
}

#[test]
fn concat_graph_two_ranges() {
    let mut s = spec(&[(0.0, 1.0), (2.0, 3.0)]);
    s.speed = 1.5;
    s.sources = two_sources(100, 60, true);
    s.has_manifest_tracks = true;
    let p = plan(&s).unwrap();
    assert_eq!(
        p.filter_graph.as_deref(),
        Some(
            "[0:v]trim=start=0.000000:end=1.000000,setpts=PTS-STARTPTS,setpts=PTS/1.5[v0];[0:v]trim=start=2.000000:end=3.000000,setpts=PTS-STARTPTS,setpts=PTS/1.5[v1];[0:a:0]volume=1.000000[src0];[0:a:1]volume=0.000000[src1];[src0][src1]amix=inputs=2:normalize=0,volume=1.000000,alimiter=limit=0.98:level=disabled,volume=0.800000[amixed];[amixed]asplit=2[amix0][amix1];[amix0]atrim=start=0.000000:end=1.000000,asetpts=PTS-STARTPTS,atempo=1.5[a0];[amix1]atrim=start=2.000000:end=3.000000,asetpts=PTS-STARTPTS,atempo=1.5[a1];[v0][a0][v1][a1]concat=n=2:v=1:a=1[vout][aout]"
        )
    );
    assert_eq!(p.video_map, MapRef::Label("vout".into()));
    // Speed applies per range on both streams, so the segments line up in
    // the concat (the reference sped audio up after the concat, which
    // padded every segment to its unsped audio length; `-shortest` then cut
    // off the later ranges' video).
    assert_eq!(p.audio_map, Some(MapRef::Label("aout".into())));
    assert_eq!(p.video, VideoOut::Encode);
    assert_eq!(p.audio, AudioOut::Encode { kbps: 320 });
    assert!(p.shortest);
    assert_eq!((p.seek_s, p.duration_s), (None, None));
}

#[test]
fn concat_without_manifest_splits_first_audio() {
    let p = plan(&spec(&[(0.0, 1.0), (2.0, 3.0)])).unwrap();
    let g = p.filter_graph.unwrap();
    assert!(
        g.contains("[0:a:0]asplit=2[adirect0][adirect1];[adirect0]atrim=start=0.000000:end=1.000000,asetpts=PTS-STARTPTS[a0]"),
        "{g}"
    );
    assert!(
        g.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[vout][aout]"),
        "{g}"
    );
    assert_eq!(p.audio_map, Some(MapRef::Label("aout".into())));
    let mut silent = spec(&[(0.0, 1.0), (2.0, 3.0)]);
    silent.has_audio = false;
    let p = plan(&silent).unwrap();
    assert!(
        p.filter_graph
            .unwrap()
            .ends_with("[v0][v1]concat=n=2:v=1:a=0[vout]")
    );
    assert_eq!(p.audio_map, None);
    assert_eq!(p.audio, AudioOut::None);
}

#[test]
fn single_range_stream_copy() {
    let p = plan(&spec(&[(1.0, 3.0)])).unwrap();
    assert_eq!(p.filter_graph, None);
    assert_eq!((p.seek_s, p.duration_s), (Some(1.0), Some(2.0)));
    assert_eq!(p.video, VideoOut::Copy);
    assert_eq!(p.audio, AudioOut::Copy);
    assert_eq!(p.video_map, MapRef::AllVideoOptional);
    assert_eq!(p.audio_map, Some(MapRef::AllAudioOptional));
}

#[test]
fn single_range_filtered() {
    let mut s = spec(&[(1.0, 3.0)]);
    s.crop = Some((0, 0, 640, 360));
    s.speed = 2.0;
    let p = plan(&s).unwrap();
    assert_eq!(
        p.filter_graph.as_deref(),
        Some("[0:v]crop=640:360:0:0,setpts=PTS/2[vout]")
    );
    assert_eq!((p.seek_s, p.duration_s), (Some(1.0), Some(1.0)));
    assert_eq!(p.video_map, MapRef::Label("vout".into()));
    assert_eq!(p.audio_map, Some(MapRef::AllAudioOptional));
    assert_eq!(p.audio_filter.as_deref(), Some("atempo=2"));
    assert_eq!(p.audio, AudioOut::Encode { kbps: 320 });
    assert!(!p.shortest);
}

#[test]
fn single_range_all_sources_unavailable_has_no_audio() {
    // The reference appends `None` to its command here; we drop the audio.
    let mut s = spec(&[(1.0, 3.0)]);
    s.has_manifest_tracks = true;
    s.sources = vec![MixSource {
        audio_index: Some(0),
        available: false,
        role: MixRole::Direct,
        gain_percent: 100,
        muted: false,
    }];
    let p = plan(&s).unwrap();
    assert_eq!(p.audio_map, None);
    assert_eq!(p.audio, AudioOut::None);
}

#[test]
fn empty_edit_is_an_error() {
    assert_eq!(
        plan(&spec(&[(1.0, 1.0005)])).unwrap_err(),
        "at least one non-empty segment is required"
    );
}

#[test]
fn watermark_snippets() {
    assert_eq!(
        filter_path(r"C:\fonts:x/Oswald-Bold.ttf"),
        r"C\:/fonts\:x/Oswald-Bold.ttf"
    );
    assert_eq!(filter_path("/a/b'c/favicon.ico"), r"/a/b\'c/favicon.ico");
    assert_eq!(ffmpeg_color("#abc"), "0xaabbcc");
    assert_eq!(ffmpeg_color("bad"), "0xbbaadd");
    assert_eq!(ffmpeg_color("#12345"), "0x000000");
    let style = WatermarkStyle {
        bg: ffmpeg_color("#101512"),
        accent: ffmpeg_color("#20e3a0"),
        text: ffmpeg_color("#f3f6f4"),
        divider: ffmpeg_color("#abc"),
        stats_dim: ffmpeg_color("bad"),
        font_path: r"C:\fonts:x/Oswald-Bold.ttf".into(),
        icon_path: "/a/b'c/favicon.ico".into(),
    };
    let f = filters(&style, "vbase", "vout");
    assert_eq!(
        f[0],
        r"color=c=0x101512@0.92:s=320x76:r=60,format=rgba,colorchannelmixer=aa=0.92,drawbox=x=0:y=0:w=3:h=ih:color=0x20e3a0@1:t=fill,drawbox=x=76:y=12:w=1:h=52:color=0xaabbcc@1:t=fill,drawtext=fontfile='C\:/fonts\:x/Oswald-Bold.ttf':text='CAPTURED WITH':fontcolor=0xbbaadd:fontsize=10:x=94:y=10,drawtext=fontfile='C\:/fonts\:x/Oswald-Bold.ttf':text='FTHRClips':fontcolor=0xf3f6f4:fontsize=22:x=94:y=29[fthr_watermark]"
    );
    assert_eq!(
        f[1],
        r"movie=filename='/a/b\'c/favicon.ico':loop=1,format=rgba,scale=48:48:flags=neighbor,pad=48:48:(ow-iw)/2:(oh-ih)/2:color=black@0[fthr_watermark_icon]"
    );
    assert_eq!(
        f[2],
        "[fthr_watermark][fthr_watermark_icon]overlay=x=14:y=14:format=auto:eof_action=repeat[fthr_watermark_composed]"
    );
    assert_eq!(
        f[3],
        "[vbase][fthr_watermark_composed]overlay=x='if(lt(t,0.38),W-(w+24)*(1-pow(1-t/0.38,3)),if(lt(t,2.55),W-w-24,W-w-24+(w+24)*pow(min(1,(t-2.55)/0.35),3)))':y='H-h-24':enable='between(t,0,2.9)':eof_action=pass[vout]"
    );
    // With the watermark on, a single plain range re-encodes through vbase.
    let mut s = spec(&[(1.0, 3.0)]);
    s.watermark = Some(style);
    let p = plan(&s).unwrap();
    let g = p.filter_graph.unwrap();
    assert!(g.starts_with("[0:v]null[vbase];color=c="), "{g}");
    assert_eq!(p.video, VideoOut::Encode);
}

#[test]
fn names() {
    assert_eq!(
        export_name("clip", "0123456789ab", false),
        "clip_export_0123456789ab.mp4"
    );
    assert_eq!(
        export_name("clip", "0123456789ab", true),
        "clip_export_0123456789ab.mkv"
    );
    assert_eq!(
        share_name("clip", "12-00-00", "0123abcd", false, false),
        "clip_share_12-00-00_0123abcd.mp4"
    );
    assert_eq!(
        share_name("clip", "12-00-00", "0123abcd", true, false),
        "clip_discord_12-00-00_0123abcd.mp4"
    );
    assert_eq!(
        staged_name(
            std::path::Path::new("/x/clip_export_ab.mp4"),
            "f".repeat(32).as_str()
        ),
        std::path::PathBuf::from(format!("/x/.clip_export_ab.partial.{}.mp4", "f".repeat(32)))
    );
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("a_share.mp4");
    assert_eq!(unique(&p), p);
    std::fs::write(&p, b"").unwrap();
    assert_eq!(unique(&p), tmp.path().join("a_share_2.mp4"));
    std::fs::write(tmp.path().join("a_share_2.mp4"), b"").unwrap();
    assert_eq!(unique(&p), tmp.path().join("a_share_3.mp4"));
    assert_eq!(speed_adjusted_duration(3.0, 1.5), 2.0);
    assert_eq!(speed_adjusted_duration(0.01, 1.0), 0.1);
}

#[test]
fn plan_maths() {
    let discord = ExportPreset::builtin_discord();
    let src = MediaFacts {
        size_bytes: 93_750_000,
        duration_s: 30.0,
        width: 1920,
        height: 1080,
        fps: 60.0,
        video_codec: "h264".into(),
        has_audio: true,
        audio_codec: "aac".into(),
        audio_kbps: 160,
    };
    let p = plan_export(&src, &discord, None);
    assert_eq!(p.target_bytes, 9_751_756);
    assert_eq!((p.target_width, p.target_height), (1280, 720));
    assert_eq!(p.video_kbps, 2492);
    assert_eq!(p.audio_kbps, 96);
    assert!(p.reencode_video && p.reencode_audio);
    assert_eq!(
        p.summary(),
        "1920×1080→1280×720 · 89.4 MB→≤9.3 MB · AAC 96 kbps audio"
    );
    let small = MediaFacts {
        size_bytes: 2_000_000,
        duration_s: 10.0,
        width: 1280,
        height: 720,
        fps: 30.0,
        video_codec: "h264".into(),
        has_audio: true,
        audio_codec: "aac".into(),
        audio_kbps: 96,
    };
    assert_eq!(
        plan_export(&small, &discord, None).summary(),
        "Original already fits; no re-encoding needed."
    );
    let huge = MediaFacts {
        size_bytes: 4_000_000_000,
        duration_s: 600.0,
        width: 3840,
        height: 2160,
        fps: 60.0,
        video_codec: "hevc".into(),
        has_audio: true,
        audio_codec: "opus".into(),
        audio_kbps: 320,
    };
    let tiny = ExportPreset {
        preset_id: "tiny".into(),
        name: "Tiny".into(),
        target_size_mb: 1.0,
        target_width: None,
        target_height: None,
        target_fps: None,
        video_codec: "h264".into(),
        audio_bitrate_kbps: 128,
        built_in: false,
        safety_margin: 0.93,
    };
    let p = plan_export(&huge, &tiny, None);
    assert_eq!(p.target_bytes, 975_175);
    assert_eq!((p.target_width, p.target_height), (640, 360));
    assert_eq!(p.video_kbps, 120, "the 120 kbps floor");
    assert_eq!(
        p.summary(),
        "3840×2160→640×360 · 3814.7 MB→≤0.9 MB · AAC 128 kbps audio"
    );
}

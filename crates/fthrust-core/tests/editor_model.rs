//! The editor model: timeline, snapshots, undo and the draft codec
//! (reference `ui/clip_viewer.py` `TrimSlider`, `EditorSnapshot`,
//! `_editor_snapshot_from_draft`).

use fthrust_core::clip_metadata::{ClipMetadataStore, DraftFile, EditorDraft};
use fthrust_core::editor::format;
use fthrust_core::editor::snapshot::{Effects, Snapshot, UndoStack, decode_state, encode_state};
use fthrust_core::editor::timeline::{Segment, Timeline};
use fthrust_core::editor::tools::{QuickCrop, SPEED_PRESETS, normalise_speed};
use serde_json::json;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn segs(spec: &[(f64, f64, bool)]) -> Vec<Segment> {
    spec.iter()
        .map(|&(start, end, deleted)| Segment {
            start,
            end,
            deleted,
        })
        .collect()
}

#[test]
fn split_and_delete_follow_reference() {
    let mut t = Timeline::new(10_000);
    assert!(close(t.minimum(), 0.025));
    assert!(t.split_at(0.3));
    assert_eq!(t.segments.len(), 2);
    assert_eq!(t.selected, 1);
    assert!(!t.split_at(0.3005), "inside the minimum gap of an edge");
    assert!(t.split_at(0.6));
    t.selected = 1;
    assert!(t.toggle_selected());
    assert!(t.segments[1].deleted);
    assert_eq!(t.kept_ranges(), vec![(0.0, 0.3), (0.6, 1.0)]);
    assert_eq!(t.cut_count(), 1);
    t.selected = 0;
    assert!(t.toggle_selected());
    t.selected = 2;
    assert!(!t.can_toggle_selected(), "the last active segment stays");
    assert!(!t.toggle_selected());
    assert_eq!(t.segments.iter().filter(|s| !s.deleted).count(), 1);
    t.selected = 1;
    assert!(t.toggle_selected(), "restoring is always allowed");
    assert!(!t.segments[1].deleted);
    assert!(!t.split_at(0.1), "a deleted segment cannot be split");
}

#[test]
fn trim_clamps_and_kept_ranges_clip_to_trim() {
    let mut t = Timeline::new(10_000);
    t.set_trim_start(0.95);
    assert!(close(t.trim_start, 0.95));
    t.set_trim_start(0.99);
    assert!(close(t.trim_start, 1.0 - 0.025));
    t.set_trim_end(0.0);
    assert!(close(t.trim_end, t.trim_start + 0.025));

    let mut t = Timeline::new(10_000);
    t.segments = segs(&[(0.0, 0.3, false), (0.3, 0.6, true), (0.6, 1.0, false)]);
    t.set_trim_start(0.2);
    t.set_trim_end(0.7);
    let kept = t.kept_ranges();
    assert_eq!(kept.len(), 2);
    assert!(close(kept[0].0, 0.2) && close(kept[0].1, 0.3));
    assert!(close(kept[1].0, 0.6) && close(kept[1].1, 0.7));
    assert_eq!(t.kept_ranges_ms(), vec![(2000, 3000), (6000, 7000)]);
    assert!(close(t.kept_fraction(), 0.2));
}

#[test]
fn hide_deleted_maps_display_and_back() {
    let mut t = Timeline::new(10_000);
    t.segments = segs(&[(0.0, 0.3, false), (0.3, 0.6, true), (0.6, 1.0, false)]);
    assert!(
        close(t.source_to_display(0.45), 0.45),
        "identity while shown"
    );
    t.hide_deleted = true;
    assert!((t.source_to_display(0.6) - 0.3 / 0.7).abs() < 1e-9);
    for x in [0.0, 0.1, 0.29, 0.61, 0.8, 1.0] {
        let back = t.display_to_source(t.source_to_display(x));
        assert!((back - x).abs() < 1e-9, "{x} -> {back}");
    }
    assert_eq!(
        t.segment_index_at(0.3),
        Some(0),
        "the deleted segment is skipped"
    );
    assert_eq!(
        t.segment_index_at(1.0),
        Some(2),
        "fallback to the last active end"
    );
}

#[test]
fn zoom_and_pan_limits() {
    let mut t = Timeline::new(10_000);
    assert!(close(t.max_zoom(), 8.0));
    assert!(close(Timeline::new(3_600_000).max_zoom(), 720.0));
    assert!(close(Timeline::new(20_000_000).max_zoom(), 2048.0));
    t.set_zoom(1.25, Some(0.5));
    assert!(close(t.span(), 0.8));
    // The anchor keeps its place on screen: 0.5 was at 50 % of the view.
    assert!(close((0.5 - t.view_start) / t.span(), 0.5));
    let before = t.view_start;
    t.pan(1.0);
    assert!(close(t.view_start, (before + 0.7 * 0.8).min(1.0 - 0.8)));
    t.pan(-1.0);
    t.pan(-1.0);
    assert!(close(t.view_start, 0.0));
    t.set_zoom(4.0, Some(0.0));
    t.ensure_visible(0.95, false);
    assert!(close(t.view_start, 0.75), "clamped to 1 - span");
    t.set_zoom(8.0, Some(0.0));
    t.ensure_visible(0.7, true);
    assert!(close(t.view_start, 0.7 - 0.08 * 0.125));
    t.set_zoom(99.0, None);
    assert!(close(t.zoom, 8.0));
}

#[test]
fn playhead_helpers() {
    let mut t = Timeline::new(10_000);
    t.segments = segs(&[(0.0, 0.3, false), (0.3, 0.6, true), (0.6, 1.0, false)]);
    assert_eq!(t.kept_range_at_ms(2990, 25), None, "inside the end slack");
    assert_eq!(t.kept_range_at_ms(1000, 25), Some(0));
    assert_eq!(t.kept_range_at_ms(5990, 25), Some(1), "start slack");
    assert_eq!(t.next_kept_start_ms(4000), Some(6000));
    assert_eq!(t.next_kept_start_ms(9990), None);
}

#[test]
fn short_clip_limits() {
    let mut t = Timeline::new(300);
    assert!((t.minimum() - 250.0 / 300.0).abs() < 1e-9);
    assert!(!t.can_split(0.5));
    assert!(!t.split_at(0.5));
    t.set_trim_start(0.9);
    t.set_trim_end(0.1);
    assert!(t.trim_start < t.trim_end);
    assert!(close(t.max_zoom(), 8.0));
    let mut tiny = Timeline::new(100);
    tiny.set_trim_start(0.5);
    tiny.set_trim_end(0.2);
    assert!(tiny.trim_start < tiny.trim_end);
}

fn snapshot() -> Snapshot {
    Snapshot {
        trim_start: 0.1,
        trim_end: 0.9,
        segments: segs(&[(0.0, 0.5, false), (0.5, 1.0, true)]),
        selected: 1,
        crop: Some((10, 20, 640, 360)),
        stretch: 1.5,
        effects: Effects {
            exposure: 10,
            contrast: -5,
            saturation: 0,
            temperature: 3,
            sharpness: 40,
            vignette: 2,
        },
        speed: 0.5,
        preserve_pitch: false,
    }
}

#[test]
fn undo_stack_caps_and_clears_redo() {
    let mut u = UndoStack::default();
    let mut a = snapshot();
    let first = a.clone();
    for i in 0..101 {
        let mut b = a.clone();
        b.selected = i + 2;
        assert!(u.commit(a.clone(), &b));
        a = b;
    }
    assert_eq!(u.undo.len(), 100);
    assert_ne!(u.undo[0], first, "the oldest step was dropped");
    assert!(!u.commit(a.clone(), &a), "no-op when nothing changed");
    let back = u.undo(&a).unwrap();
    assert!(u.can_redo());
    assert_eq!(u.redo(&back).unwrap(), a);
    let back = u.undo(&a).unwrap();
    let mut c = back.clone();
    c.stretch = 2.0;
    u.commit(back, &c);
    assert!(!u.can_redo());
}

#[test]
fn draft_round_trip() {
    let s = snapshot();
    let v = encode_state(&s);
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "crop_rect",
            "effects",
            "preserve_pitch",
            "segments",
            "selected_segment",
            "speed_rate",
            "stretch_ratio",
            "trim_end",
            "trim_start"
        ]
    );
    assert_eq!(v["segments"], json!([[0.0, 0.5, false], [0.5, 1.0, true]]));
    assert_eq!(v["effects"]["sharpness"], json!(40));
    assert_eq!(decode_state(&v), Some(s));
}

#[test]
fn draft_validation_matches_reference() {
    let base = encode_state(&snapshot());
    let with = |key: &str, value: serde_json::Value| {
        let mut v = base.clone();
        v[key] = value;
        v
    };
    let nan = serde_json::Value::from(f64::NAN);
    for bad in [
        with("trim_start", nan),
        with("trim_start", json!(0.9)),
        with("stretch_ratio", json!(5.0)),
        with("speed_rate", json!(3.0)),
        with("segments", json!([])),
        with("segments", json!(vec![json!([0.0, 1.0, false]); 501])),
        with("segments", json!([[0.0, 0.5, false], [0.501, 1.0, false]])),
        with("segments", json!([[0.1, 1.0, false]])),
        with("segments", json!([[0.0, 0.5, true], [0.5, 1.0, true]])),
        with("segments", json!([[0.0, 0.5], [0.5, 1.0, false]])),
        json!("not an object"),
    ] {
        assert_eq!(decode_state(&bad), None, "{bad}");
    }
    let lenient = decode_state(&with("selected_segment", json!(9))).unwrap();
    assert_eq!(lenient.selected, 1);
    assert_eq!(
        decode_state(&with("crop_rect", json!([0, 0, 1, 1])))
            .unwrap()
            .crop,
        None
    );
    let fx = decode_state(&with("effects", json!({"exposure": 500, "vignette": -3}))).unwrap();
    assert_eq!(fx.effects.exposure, 100);
    assert_eq!(fx.effects.vignette, 0);
    assert_eq!(fx.effects.contrast, 0, "missing keys default to 0");
    assert!(
        decode_state(&with("preserve_pitch", json!("yes")))
            .unwrap()
            .preserve_pitch
    );
}

#[test]
fn draft_for_changed_file_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = fthrust_core::FthrPaths::new(tmp.path());
    let clip = tmp.path().join("a.mp4");
    std::fs::write(&clip, b"12345").unwrap();
    let mut identity = fthrust_core::media_identity::file_identity(&clip).unwrap();
    let mut store = ClipMetadataStore::open(&paths).unwrap();
    store
        .set_editor_draft(
            &clip,
            EditorDraft {
                version: 1,
                file: DraftFile {
                    mtime_ns: identity.mtime_ns,
                    size: identity.size,
                },
                updated_at: 1.0,
                state: encode_state(&snapshot()),
            },
        )
        .unwrap();
    assert!(
        store
            .editor_draft_if_current(&clip, &identity)
            .unwrap()
            .is_some()
    );
    identity.size += 1;
    assert!(
        store
            .editor_draft_if_current(&clip, &identity)
            .unwrap()
            .is_none()
    );
}

#[test]
fn quick_crop_scales() {
    let saved = QuickCrop::from_setting(&json!({
        "x": 100, "y": 50, "w": 800, "h": 600, "src_w": 1920, "src_h": 1080
    }))
    .unwrap();
    assert_eq!(saved.apply(1280, 720), Some((66, 33, 533, 400)));
    assert_eq!(saved.apply(8, 8), None);
    assert_eq!(QuickCrop::from_setting(&json!(null)), None);
    assert_eq!(QuickCrop::from_setting(&saved.to_setting()), Some(saved));
}

#[test]
fn speed_presets_and_normalise() {
    assert_eq!(SPEED_PRESETS.len(), 7);
    assert_eq!(SPEED_PRESETS[0], ("0.25×  SLOW", 0.25));
    assert_eq!(SPEED_PRESETS[3], ("1×  NORMAL", 1.0));
    assert_eq!(SPEED_PRESETS[6], ("2×  FAST", 2.0));
    assert_eq!(normalise_speed(3.0), 2.0);
    assert_eq!(normalise_speed(0.1), 0.25);
    assert_eq!(normalise_speed(f64::NAN), 1.0);
    assert_eq!(normalise_speed(-1.0), 1.0);
}

#[test]
fn formatting() {
    assert_eq!(format::clock(0), "0:00");
    assert_eq!(format::clock(9_999), "0:09");
    assert_eq!(format::clock(3_725_000), "62:05");
    assert_eq!(format::range_label(0, 10_000), "0:00 → 0:10");
    assert_eq!(format::kept_label(7000, 1), "0:07 kept  ·  1 cut");
    assert!(format::kept_label(7000, 2).ends_with("2 cuts"));
    assert!(format::kept_label(7000, 0).ends_with("0 cuts"));
    let stem = "a".repeat(40);
    assert_eq!(format::title(&stem), format!("{}…", "a".repeat(36)));
    assert_eq!(format::title(&"b".repeat(38)), "b".repeat(38));
    let dir = format!("/{}", "c".repeat(49));
    assert_eq!(format::location(&dir), format!("…{}", "c".repeat(34)));
    assert_eq!(format::location("/tmp"), "/tmp");
    let when = time::macros::datetime!(2026-10-23 16:39 UTC);
    assert_eq!(format::created(when), "Oct 23, 2026  16:39");
}

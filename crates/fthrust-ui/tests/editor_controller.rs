//! The editor controller: lifecycle, debounced scrub and draft save,
//! kept-range playback, undo gestures and session capture (reference
//! `ui/clip_viewer.py`, notes sections 1–3 and 8).

#[path = "../../fthrust-media/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fthrust_core::FthrPaths;
use fthrust_core::clip_metadata::ClipMetadataStore;
use fthrust_core::editor::timeline::Segment;
use fthrust_core::settings::SettingsStore;
use fthrust_ui::editor::{Editor, Lifecycle, PlayerFactory};
use fthrust_ui::services::playback::PlaybackService;
use fthrust_ui::services::playback::audio_out::NullOutput;

struct Rig {
    _tmp: tempfile::TempDir,
    clock: Arc<AtomicU64>,
    store: ClipMetadataStore,
    settings: SettingsStore,
    home: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let paths = FthrPaths::new(&home);
        Rig {
            store: ClipMetadataStore::open(&paths).unwrap(),
            settings: SettingsStore::load(paths).unwrap(),
            clock: Arc::new(AtomicU64::new(1_000)),
            home,
            _tmp: tmp,
        }
    }

    fn clip(&self, name: &str, seconds: u32, audio: bool) -> Option<PathBuf> {
        let p = self.home.join(name);
        support::write_test_clip(&p, seconds, audio)?;
        Some(p)
    }

    fn factory(&self) -> PlayerFactory {
        let clock = self.clock.clone();
        let home = self.home.clone();
        Arc::new(move |path: &Path| {
            let c = clock.clone();
            let f: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(move || c.load(Ordering::Relaxed));
            PlaybackService::open(path, &home, Box::new(NullOutput::new(f.clone())), f)
        })
    }

    fn now(&self) -> f64 {
        self.clock.load(Ordering::Relaxed) as f64 / 1000.0
    }

    fn open(&self, clip: &Path) -> Editor {
        Editor::open(clip, &self.store, &self.settings, self.factory()).unwrap()
    }

    /// Advances the manual clock by `ms` in 10 ms steps, ticking each step.
    fn run(&mut self, e: &mut Editor, ms: u64) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(10);
            self.clock.fetch_add(step, Ordering::Relaxed);
            left -= step;
            std::thread::sleep(Duration::from_millis(2));
            let now = self.now();
            e.tick(now, &mut self.settings, &mut self.store);
        }
    }

    /// Ticks (without advancing the clock) until the player has loaded.
    fn wait_loaded(&mut self, e: &mut Editor) {
        for _ in 0..1000 {
            let now = self.now();
            e.tick(now, &mut self.settings, &mut self.store);
            if e.player_loaded() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("player never loaded: {:?} {}", e.lifecycle, e.status_text());
    }
}

#[test]
fn lifecycle_reaches_ready_and_play_queues() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    assert_eq!(e.status_text(), "Preparing clip…");
    assert_eq!(e.play_label(), "PLAY");
    e.toggle_play(r.now());
    assert_eq!(e.lifecycle, Lifecycle::PlayQueued);
    assert_eq!(e.status_text(), "Preparing · play queued");
    assert_eq!(e.play_label(), "Preparing…");
    e.toggle_play(r.now());
    assert_eq!(e.lifecycle, Lifecycle::Preparing, "a second click cancels");
    e.toggle_play(r.now());
    r.wait_loaded(&mut e);
    assert_eq!(e.lifecycle, Lifecycle::Playing);
    assert_eq!(e.play_label(), "PAUSE");
    assert_eq!(e.status_text(), "Ready");
    r.run(&mut e, 890);
    assert_eq!(e.status_text(), "Ready");
    r.run(&mut e, 20);
    assert_eq!(e.status_text(), "", "Ready clears after 900 ms");
    e.toggle_play(r.now());
    assert_eq!(e.lifecycle, Lifecycle::Paused);
    assert_eq!(e.play_label(), "PLAY");
}

#[test]
fn scrub_is_debounced() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    let now = r.now();
    e.scrub_to(0.25, now);
    r.run(&mut e, 10);
    e.scrub_to(0.5, r.now());
    r.run(&mut e, 10);
    e.scrub_to(0.75, r.now());
    let target = (0.75 * e.duration_ms() as f64) as u64;
    assert_eq!(
        e.player_position_ms(),
        Some(0),
        "nothing reached the player yet"
    );
    assert_eq!(
        e.position_ms(),
        target,
        "the label follows the pending seek"
    );
    r.run(&mut e, 40);
    assert_eq!(
        e.player_position_ms(),
        Some(0),
        "still inside 50 ms of the last scrub"
    );
    r.run(&mut e, 20);
    assert_eq!(e.player_position_ms(), Some(target));
}

#[test]
fn playback_skips_deleted_and_stops_at_edit_end() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.timeline.segments = vec![
        Segment {
            start: 0.0,
            end: 0.25,
            deleted: false,
        },
        Segment {
            start: 0.25,
            end: 0.5,
            deleted: true,
        },
        Segment {
            start: 0.5,
            end: 1.0,
            deleted: false,
        },
    ];
    e.scrub_to(0.225, r.now());
    r.run(&mut e, 60);
    e.toggle_play(r.now());
    r.run(&mut e, 300);
    let p = e.position_ms();
    assert!(
        (2000..2400).contains(&p),
        "jumped over the deleted span: {p}"
    );
    r.run(&mut e, 4000);
    assert_eq!(
        e.lifecycle,
        Lifecycle::Paused,
        "paused at the end of the edit"
    );
    assert!(
        e.position_ms() < 100,
        "rewound to the first kept start: {}",
        e.position_ms()
    );
}

#[test]
fn play_outside_kept_range_jumps_forward() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.timeline.segments = vec![
        Segment {
            start: 0.0,
            end: 0.25,
            deleted: false,
        },
        Segment {
            start: 0.25,
            end: 0.5,
            deleted: true,
        },
        Segment {
            start: 0.5,
            end: 1.0,
            deleted: false,
        },
    ];
    e.scrub_to(0.375, r.now());
    r.run(&mut e, 60);
    e.toggle_play(r.now());
    let target = e.timeline.kept_ranges_ms()[1].0;
    let deleted_start = e.timeline.kept_ranges_ms()[0].1;
    // Wait for the jump (slow CI runners apply the seek late), recording
    // every position: none may fall inside the deleted range on the way.
    let mut seen = Vec::new();
    let mut p = 0;
    for _ in 0..150 {
        r.run(&mut e, 20);
        p = e.player_position_ms().unwrap_or(0);
        seen.push(p);
        if p >= target {
            break;
        }
    }
    assert!((target..target + 300).contains(&p), "{p} from {target}");
    let scrubbed = (0.375 * e.duration_ms() as f64) as u64;
    assert!(
        seen.iter()
            .all(|&q| q <= scrubbed + 50 || q >= target || q < deleted_start),
        "played through the deleted range: {seen:?}"
    );
}

#[test]
fn trim_drag_is_one_undo_step() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.begin_gesture();
    for x in [0.1, 0.2, 0.3] {
        e.timeline.set_trim_start(x);
    }
    e.end_gesture(r.now());
    assert_eq!(e.undo.undo.len(), 1);
    e.undo(r.now());
    assert_eq!(e.timeline.trim_start, 0.0);
    e.redo(r.now());
    assert!((e.timeline.trim_start - 0.3).abs() < 1e-9);
    e.begin_gesture();
    e.end_gesture(r.now());
    assert_eq!(e.undo.undo.len(), 1, "an unchanged gesture records nothing");
}

#[test]
fn draft_saved_after_debounce_and_reloaded() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.scrub_to(0.5, r.now());
    r.run(&mut e, 60);
    assert!(e.split(r.now()));
    r.run(&mut e, 440);
    assert!(r.store.get(&clip).unwrap().editor_draft.is_none());
    r.run(&mut e, 10);
    let draft = r
        .store
        .get(&clip)
        .unwrap()
        .editor_draft
        .expect("draft saved at 450 ms");
    assert_eq!(draft.version, 1);
    assert_eq!(draft.file.size, std::fs::metadata(&clip).unwrap().len());
    e.close(&mut r.store);
    let again = r.open(&clip);
    assert_eq!(again.timeline.segments.len(), 2);
    assert!(again.undo.undo.is_empty(), "undo history is not persisted");
    assert!(
        again.timeline_label().ends_with("1 cut"),
        "{}",
        again.timeline_label()
    );
}

#[test]
fn split_failure_feedback() {
    let r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    e.timeline.playhead = 0.01;
    let now = r.now();
    assert!(!e.split(now));
    assert_eq!(e.split_feedback_until, Some(now + 1.2));
}

#[test]
fn capture_restore_round_trip() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.scrub_to(0.5, r.now());
    r.run(&mut e, 60);
    e.split(r.now());
    e.set_speed(0.5, r.now());
    e.timeline.set_zoom(2.0, Some(0.5));
    e.scrub_to(0.6, r.now());
    let state = e.capture(&mut r.store);
    let snap = e.snapshot();
    e.close(&mut r.store);
    let mut e = r.open(&clip);
    e.restore(state);
    r.wait_loaded(&mut e);
    r.run(&mut e, 60);
    assert_eq!(e.snapshot(), snap);
    assert_eq!(e.undo.undo.len(), 2);
    assert_eq!(e.timeline.zoom, 2.0);
    assert_eq!(
        e.player_position_ms(),
        Some((0.6 * e.duration_ms() as f64) as u64)
    );
    assert_eq!(
        e.lifecycle,
        Lifecycle::Paused,
        "a restored session never autoplays"
    );
    assert!(
        e.timeline_label().contains("kept"),
        "{}",
        e.timeline_label()
    );
}

#[test]
fn video_only_clip_plays() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("v.mp4", 2, false) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    assert_eq!(e.status_text(), "Ready");
    assert!(e.sources().is_empty());
    e.toggle_play(r.now());
    r.run(&mut e, 500);
    let p = e.position_ms();
    assert!((400..=600).contains(&p), "{p}");
}

#[test]
fn master_and_quick_crop_write_settings() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    e.set_master(55, &mut r.settings);
    assert_eq!(r.settings.get_i64("master_volume"), Some(55));
    let now = r.now();
    assert!(!e.save_quick_crop(&mut r.settings, now), "no crop to save");
    e.crop = Some((10, 10, 100, 50));
    assert!(e.save_quick_crop(&mut r.settings, now));
    let qc = r.settings.get("quick_crop").unwrap().clone();
    assert_eq!(qc["w"], 100);
    assert_eq!(qc["src_w"], 160, "the test clip is 160×90");
    e.crop = None;
    assert!(e.apply_quick_crop(&r.settings, r.now()));
    assert_eq!(e.crop, Some((10, 10, 100, 50)));
    assert_eq!(e.undo.undo.len(), 1);
}

#[test]
fn unopenable_clip_fails() {
    let mut r = Rig::new();
    let bad = r.home.join("bad.mp4");
    std::fs::write(&bad, b"not a video").unwrap();
    let mut e = r.open(&bad);
    for _ in 0..200 {
        let now = r.now();
        e.tick(now, &mut r.settings, &mut r.store);
        if e.lifecycle == Lifecycle::Failed {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(e.lifecycle, Lifecycle::Failed);
    assert_eq!(e.status_text(), "CLIP UNAVAILABLE");
    e.toggle_play(r.now());
    assert_eq!(
        e.lifecycle,
        Lifecycle::Failed,
        "play is ignored once failed"
    );
}

#[test]
fn filmstrip_loads_after_idle_and_cancels_on_play() {
    use fthrust_ui::editor::filmstrip::FilmstripState;
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 4, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    r.run(&mut e, 200);
    assert_eq!(
        e.filmstrip.state(),
        FilmstripState::Idle,
        "not before 300 ms idle"
    );
    e.toggle_play(r.now());
    r.run(&mut e, 500);
    assert_eq!(
        e.filmstrip.state(),
        FilmstripState::Idle,
        "never while playing"
    );
    e.toggle_play(r.now());
    r.run(&mut e, 1490);
    assert_eq!(
        e.filmstrip.state(),
        FilmstripState::Idle,
        "1500 ms after a pause"
    );
    r.run(&mut e, 20);
    assert_ne!(e.filmstrip.state(), FilmstripState::Idle);
    for _ in 0..500 {
        if e.filmstrip.state() == FilmstripState::Ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        let now = r.now();
        e.tick(now, &mut r.settings, &mut r.store);
    }
    let frames = e.filmstrip.take_ready().expect("frames decoded");
    assert_eq!(frames.len(), 24, "min(120, max(24, 24 + seconds / 60))");
    assert_eq!(frames[0].size, [180, 100]);
}

#[test]
fn filmstrip_off_when_disabled() {
    use fthrust_ui::editor::filmstrip::FilmstripState;
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    r.settings
        .set("timeline_preview_strip_enabled", serde_json::json!(false));
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    r.run(&mut e, 600);
    assert_eq!(e.filmstrip.state(), FilmstripState::Idle);
}

#[test]
fn untouched_clip_writes_no_draft() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    r.run(&mut e, 600);
    let _ = e.capture(&mut r.store);
    e.close(&mut r.store);
    assert!(
        r.store.get(&clip).unwrap().editor_draft.is_none(),
        "opening and closing an unedited clip must not use a draft slot"
    );
}

#[test]
fn quick_crop_waits_for_the_real_source_size() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    r.settings.set(
        "quick_crop",
        serde_json::json!({"x": 0, "y": 0, "w": 100, "h": 50, "src_w": 160, "src_h": 90}),
    );
    let mut e = r.open(&clip);
    assert!(
        !e.apply_quick_crop(&r.settings, r.now()),
        "not before the player loads"
    );
    assert_eq!(e.crop, None);
}

#[test]
fn oversized_draft_crop_is_clamped_on_load() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    let now = r.now();
    e.set_crop(Some((10, 10, 100_000, 100_000)), now);
    let st = e.capture(&mut r.store);
    e.close(&mut r.store);
    let mut e = r.open(&clip);
    e.restore(st);
    r.wait_loaded(&mut e);
    assert_eq!(
        e.crop,
        Some((10, 10, 150, 80)),
        "clamped into the 160×90 source"
    );
}

#[test]
fn pending_timers_ask_for_a_wake() {
    let mut r = Rig::new();
    let Some(clip) = r.clip("a.mp4", 2, true) else {
        return;
    };
    let mut e = r.open(&clip);
    r.wait_loaded(&mut e);
    r.run(&mut e, 2000);
    let now = r.now();
    assert_eq!(e.next_wake(now), None, "idle and paused: nothing due");
    e.scrub_to(0.5, now);
    let due = e.next_wake(now).expect("the scrub seek is due");
    assert!(due <= 0.051, "{due}");
}

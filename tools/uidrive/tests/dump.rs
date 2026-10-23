use std::time::{Duration, Instant};

use uidrive::dump::{self, Dump, Marker};

fn dump() -> Dump {
    Dump {
        frame: 5,
        time: 1.0,
        pixels_per_point: 1.0,
        page: "clips".into(),
        status: "CAPTURING".into(),
        markers: vec![
            Marker {
                name: "QPushButton[Record]".into(),
                x: 12.0,
                y: 8.0,
                w: 96.0,
                h: 36.0,
                text: Some("Record".into()),
            },
            Marker {
                name: "clipCard[0]".into(),
                x: 0.0,
                y: 100.0,
                w: 200.0,
                h: 150.0,
                text: Some("a.mp4".into()),
            },
            Marker {
                name: "clipCard[1]".into(),
                x: 210.0,
                y: 100.0,
                w: 200.0,
                h: 150.0,
                text: Some("b.mp4".into()),
            },
        ],
        error_bar: None,
        dialog: None,
    }
}

#[test]
fn find_prefers_exact_then_text_then_prefix() {
    let d = dump();
    assert_eq!(
        dump::find(&d, "clipCard[1]", 0).map(|m| m.name.as_str()),
        Some("clipCard[1]")
    );
    assert_eq!(
        dump::find(&d, "b.mp4", 0).map(|m| m.name.as_str()),
        Some("clipCard[1]")
    );
    assert_eq!(
        dump::find(&d, "clipCard", 1).map(|m| m.name.as_str()),
        Some("clipCard[1]")
    );
    assert_eq!(
        dump::find(&d, "clipCard", 0).map(|m| m.name.as_str()),
        Some("clipCard[0]")
    );
    assert!(dump::find(&d, "nope", 0).is_none());
    assert!(dump::find(&d, "clipCard", 5).is_none());
    assert_eq!(d.markers[0].center(), (60.0, 26.0));
}

#[test]
fn read_refuses_scaled_dumps() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ui.json");
    let mut d = dump();
    d.pixels_per_point = 2.0;
    std::fs::write(&path, serde_json::to_vec(&d).unwrap()).unwrap();
    let err = dump::read(&path).unwrap_err();
    assert!(err.contains("pixels_per_point"), "{err}");
    d.pixels_per_point = 1.0;
    std::fs::write(&path, serde_json::to_vec(&d).unwrap()).unwrap();
    assert_eq!(dump::read(&path).unwrap().status, "CAPTURING");
    assert!(dump::read(&tmp.path().join("missing.json")).is_err());
}

#[test]
fn wait_ignores_frames_before_the_action() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ui.json");
    std::fs::write(&path, serde_json::to_vec(&dump()).unwrap()).unwrap();
    // A stale frame (5) that does not satisfy the predicate never counts.
    let err = dump::wait(
        &path,
        |d| d.status == "SAVING",
        Duration::from_millis(400),
        Some(5),
        "status",
    )
    .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    // A stale frame that satisfies it counts only after the grace period.
    let t0 = Instant::now();
    let d = dump::wait(
        &path,
        |d| d.status == "CAPTURING",
        Duration::from_secs(2),
        Some(5),
        "status",
    )
    .unwrap();
    assert_eq!(d.frame, 5);
    assert!(
        t0.elapsed() >= Duration::from_millis(240),
        "{:?}",
        t0.elapsed()
    );
    // A newer frame is taken at once.
    let p2 = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let mut d = dump();
        d.frame = 6;
        d.status = "SAVING".into();
        std::fs::write(&p2, serde_json::to_vec(&d).unwrap()).unwrap();
    });
    let t0 = Instant::now();
    let d = dump::wait(
        &path,
        |d| d.status == "SAVING",
        Duration::from_secs(2),
        Some(5),
        "status",
    )
    .unwrap();
    assert_eq!(d.frame, 6);
    assert!(t0.elapsed() < Duration::from_millis(700));
}

/// A dump whose `clipCard[0]` sits at `y` (`None`: not drawn yet).
fn at(frame: u64, y: Option<f64>) -> Dump {
    let mut d = dump();
    d.frame = frame;
    d.markers.retain(|m| m.name != "clipCard[0]");
    if let Some(y) = y {
        d.markers.push(Marker {
            name: "clipCard[0]".into(),
            x: 0.0,
            y,
            w: 200.0,
            h: 150.0,
            text: None,
        });
    }
    d
}

#[test]
fn settled_waits_for_a_sliding_target_to_stop() {
    // A page slide: the target moves on every read, then rests.
    let mut frames = vec![
        at(1, None),
        at(2, Some(600.0)),
        at(3, Some(300.0)),
        at(4, Some(101.0)),
        at(5, Some(101.0)),
    ]
    .into_iter();
    let mut last = at(5, Some(101.0));
    let m = dump::settled(
        || {
            if let Some(d) = frames.next() {
                last = d;
            }
            Ok(last.clone())
        },
        "clipCard[0]",
        0,
        20,
        || {},
    )
    .unwrap();
    assert_eq!(m.y, 101.0);
}

#[test]
fn settled_returns_a_resting_target_at_once() {
    let mut reads = 0;
    let m = dump::settled(
        || {
            reads += 1;
            Ok(at(7, Some(42.0)))
        },
        "clipCard[0]",
        0,
        20,
        || {},
    )
    .unwrap();
    assert_eq!(m.y, 42.0);
    assert_eq!(reads, 2, "two agreeing reads are enough");
}

#[test]
fn settled_reports_a_missing_target() {
    let err = dump::settled(|| Ok(at(1, None)), "clipCard[0]", 0, 3, || {}).unwrap_err();
    assert!(err.contains("no marker matches"), "{err}");
}

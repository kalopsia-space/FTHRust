use std::time::Duration;

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
    let err = dump::wait(
        &path,
        |d| d.status == "CAPTURING",
        Duration::from_millis(300),
        Some(5),
        "status",
    )
    .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    let p2 = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let mut d = dump();
        d.frame = 6;
        std::fs::write(&p2, serde_json::to_vec(&d).unwrap()).unwrap();
    });
    let d = dump::wait(
        &path,
        |d| d.status == "CAPTURING",
        Duration::from_secs(2),
        Some(5),
        "status",
    )
    .unwrap();
    assert_eq!(d.frame, 6);
}

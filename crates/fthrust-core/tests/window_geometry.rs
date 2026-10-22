use fthrust_core::window_geometry::{Rect, SavedGeometry, Screen, popup_anchor, restore};
use serde_json::Value;

fn screens_from(v: &Value) -> Vec<Screen> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| Screen {
            name: s["name"].as_str().unwrap().to_string(),
            available: Rect {
                x: s["x"].as_i64().unwrap() as i32,
                y: s["y"].as_i64().unwrap() as i32,
                width: s["w"].as_u64().unwrap() as u32,
                height: s["h"].as_u64().unwrap() as u32,
            },
            primary: s["primary"].as_bool().unwrap_or(false),
        })
        .collect()
}

#[test]
fn restore_cases_match_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/window-geometry.json")).unwrap();
    for case in fixture["restore_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let saved: SavedGeometry = serde_json::from_value(case["saved"].clone()).unwrap();
        let screens = screens_from(&case["screens"]);
        let got = restore(&saved, &screens).unwrap_or_else(|| panic!("{name}: None"));
        let exp = &case["expected"];
        let rect: Rect = serde_json::from_value(exp["rect"].clone()).unwrap();
        assert_eq!(got.rect, rect, "{name}: rect");
        assert_eq!(
            got.screen_name,
            exp["screen_name"].as_str().unwrap(),
            "{name}: screen"
        );
        assert_eq!(
            got.maximized,
            exp["maximized"].as_bool().unwrap(),
            "{name}: maximized"
        );
    }
}

#[test]
fn no_screens_is_none() {
    assert!(restore(&SavedGeometry::default(), &[]).is_none());
}

#[test]
fn default_is_maximized() {
    let screens = [Screen {
        name: "A".into(),
        available: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        primary: true,
    }];
    let p = restore(&SavedGeometry::default(), &screens).unwrap();
    assert!(p.maximized);
}

#[test]
fn popup_sits_below_and_stays_on_screen() {
    let screen = Rect {
        x: 0,
        y: 0,
        width: 1000,
        height: 800,
    };
    let anchor = Rect {
        x: 100,
        y: 100,
        width: 50,
        height: 20,
    };
    assert_eq!(popup_anchor(anchor, (200, 100), screen), (100, 128));
    let edge = Rect {
        x: 950,
        y: 780,
        width: 50,
        height: 20,
    };
    assert_eq!(popup_anchor(edge, (200, 100), screen), (800, 700));
}

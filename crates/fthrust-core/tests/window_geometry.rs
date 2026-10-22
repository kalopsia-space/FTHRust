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

// Screen 1000x800 at origin; with the 8 px margin the safe area is
// x 8..992 (width 984), y 8..792 (height 784).

#[test]
fn popup_sits_below_anchor() {
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
    // x = 100; y = anchor.y + anchor.height = 120; 120+100 <= 792, no flip.
    assert_eq!(popup_anchor(anchor, (200, 100), screen), (100, 120));
}

#[test]
fn popup_flips_above_when_no_room_below() {
    let screen = Rect {
        x: 0,
        y: 0,
        width: 1000,
        height: 800,
    };
    let anchor = Rect {
        x: 100,
        y: 700,
        width: 50,
        height: 20,
    };
    // y below = 720; 720+100 = 820 > 792 so flip: y = 700 - 100 = 600.
    // Clamp y to [8, 792-100 = 692]: 600 stays. x = 100 fits (300 <= 992).
    assert_eq!(popup_anchor(anchor, (200, 100), screen), (100, 600));
}

#[test]
fn popup_clamps_horizontally() {
    let screen = Rect {
        x: 0,
        y: 0,
        width: 1000,
        height: 800,
    };
    // x = 900; 900+200 = 1100 > 992 so right-align: 900+50-200 = 750.
    // x clamp max = 992-200 = 792, so 750 stays. y = 100+20 = 120.
    let right = Rect {
        x: 900,
        y: 100,
        width: 50,
        height: 20,
    };
    assert_eq!(popup_anchor(right, (200, 100), screen), (750, 120));
    // Anchor at x = 0 is clamped up to the safe left edge 8.
    let left = Rect {
        x: 0,
        y: 100,
        width: 50,
        height: 20,
    };
    assert_eq!(popup_anchor(left, (200, 100), screen), (8, 120));
    // x = 950 overflows; right-align 950+50-200 = 800, clamped to max 792. y = 780+20
    // = 800 overflows (900 > 792): flip 780-100 = 680 <= 692.
    let corner = Rect {
        x: 950,
        y: 780,
        width: 50,
        height: 20,
    };
    assert_eq!(popup_anchor(corner, (200, 100), screen), (792, 680));
}

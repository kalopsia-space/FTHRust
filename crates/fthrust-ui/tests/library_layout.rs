//! Plan 5 Task 5: `ClipGrid._layout_metrics` and card geometry.

use fthrust_ui::library::layout::{
    CARD_BODY_H, CARD_MAX_W, CARD_MIN_W, CARD_W, GRID_SPACING, H_MARGIN, SECTION_SPACING,
    card_height, metrics,
};

#[test]
fn layout_metrics_match_reference() {
    assert_eq!(
        (
            CARD_W,
            CARD_MIN_W,
            CARD_MAX_W,
            CARD_BODY_H,
            GRID_SPACING,
            H_MARGIN,
            SECTION_SPACING
        ),
        (360.0, 300.0, 440.0, 88.0, 12.0, 48.0, 20.0)
    );
    assert_eq!(metrics(1298.0), (3, vec![409.0, 409.0, 408.0]));
    assert_eq!(metrics(1006.0), (3, vec![312.0, 311.0, 311.0]));
    assert_eq!(metrics(400.0), (1, vec![352.0]));
    assert_eq!(
        metrics(2560.0),
        (6, vec![409.0, 409.0, 409.0, 409.0, 408.0, 408.0])
    );
    assert_eq!(metrics(1290.0), (3, vec![406.0, 406.0, 406.0]));
    assert_eq!(
        metrics(100.0),
        (1, vec![300.0]),
        "never below the minimum card width"
    );
    assert_eq!(card_height(320.0), 268.0);
    assert_eq!(card_height(409.0), 318.0);
}

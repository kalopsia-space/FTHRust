//! Tests for the public app failure code registry.

use fthrust_core::error_codes::{APP_FAILURE_CODES, code_for_title, format_error_title};
use serde_json::Value;
use std::collections::HashSet;

const FIXTURE: &str = include_str!("fixtures/error-codes.json");

#[test]
fn table_matches_fixture_in_order() {
    let fixture: Vec<Value> = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture.len(), APP_FAILURE_CODES.len());
    for (entry, item) in fixture.iter().zip(APP_FAILURE_CODES.iter()) {
        assert_eq!(entry["code"], item.code);
        assert_eq!(entry["title"], item.title);
        assert_eq!(entry["category"], item.category);
        assert_eq!(entry["explanation"], item.explanation);
    }
}

#[test]
fn every_title_is_unique() {
    let titles: HashSet<&str> = APP_FAILURE_CODES.iter().map(|c| c.title).collect();
    assert_eq!(titles.len(), APP_FAILURE_CODES.len());
}

#[test]
fn format_known_title() {
    assert_eq!(
        format_error_title("CAPTURE FAILED"),
        "[Error 001] CAPTURE FAILED"
    );
    assert_eq!(
        code_for_title("CAPTURE FAILED").map(|c| c.code),
        Some("Error 001")
    );
}

#[test]
fn format_unknown_title_is_normalised() {
    assert_eq!(format_error_title("unknown thing"), "UNKNOWN THING");
    assert_eq!(format_error_title("NOT A REAL TITLE"), "NOT A REAL TITLE");
    assert!(code_for_title("NOT A REAL TITLE").is_none());
}

#[test]
fn format_normalises_case_and_whitespace() {
    assert_eq!(
        format_error_title("  capture failed "),
        "[Error 001] CAPTURE FAILED"
    );
    assert_eq!(
        code_for_title("  capture failed ").map(|c| c.code),
        Some("Error 001")
    );
}

//! Tests for export presets, parity-checked against the Python fixture.

use fthrust_core::export_presets::{ExportPreset, ExportPresetStore};
use fthrust_core::{CoreError, FthrPaths};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/export-presets.json")).unwrap()
}

fn setup() -> (tempfile::TempDir, FthrPaths, ExportPresetStore) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    std::fs::create_dir_all(p.root()).unwrap();
    let s = ExportPresetStore::new(&p);
    (tmp, p, s)
}

fn custom(id: &str, mb: f64) -> ExportPreset {
    ExportPreset {
        preset_id: id.into(),
        name: format!("Name {id}"),
        target_size_mb: mb,
        ..ExportPreset::catbox_adhoc()
    }
}

#[test]
fn builtin_discord_matches_fixture() {
    let want: ExportPreset = serde_json::from_value(fixture()["builtin_discord"].clone()).unwrap();
    assert_eq!(ExportPreset::builtin_discord(), want);
    assert_eq!(
        serde_json::to_value(ExportPreset::builtin_discord()).unwrap(),
        fixture()["builtin_discord"]
    );
}

#[test]
fn catbox_adhoc_matches_fixture() {
    let want: ExportPreset = serde_json::from_value(fixture()["catbox_adhoc"].clone()).unwrap();
    assert_eq!(ExportPreset::catbox_adhoc(), want);
}

#[test]
fn validation_cases_match_fixture() {
    for case in fixture()["validation_cases"].as_array().unwrap() {
        let input = &case[0];
        let accepted = case[1].as_bool().unwrap();
        let p: ExportPreset = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(p.validate().is_ok(), accepted, "{input}");
    }
}

#[test]
fn slug_matches_python() {
    assert_eq!(ExportPreset::slug("Hello, World!  2x"), "hello-world-2x");
    assert_eq!(ExportPreset::slug("!!!"), "preset");
    assert_eq!(ExportPreset::slug(&"a".repeat(100)).len(), 64);
}

#[test]
fn discord_never_written_to_file() {
    let (_t, p, s) = setup();
    assert!(matches!(
        s.save(&ExportPreset::builtin_discord()),
        Err(CoreError::Invalid(_))
    ));
    let mut fake = custom("discord", 5.0);
    fake.built_in = false;
    assert!(matches!(s.save(&fake), Err(CoreError::Invalid(_))));
    let mut b = custom("x", 5.0);
    b.built_in = true;
    assert!(matches!(s.save(&b), Err(CoreError::Invalid(_))));
    s.save(&custom("mine", 5.0)).unwrap();
    let text = std::fs::read_to_string(p.export_presets_file()).unwrap();
    assert!(!text.contains("\"discord\""));
    let ids: Vec<_> = s.all().unwrap().into_iter().map(|x| x.preset_id).collect();
    assert_eq!(ids, ["discord", "mine"]);
}

#[test]
fn duplicate_id_skipped_on_read() {
    let (_t, p, s) = setup();
    let a = serde_json::to_value(custom("a", 5.0)).unwrap();
    let a2 = serde_json::to_value(custom("a", 7.0)).unwrap();
    let bad = json!({"name": "Bad", "preset_id": "bad", "target_size_mb": 0.1});
    let dis = serde_json::to_value(custom("discord", 5.0)).unwrap();
    std::fs::write(
        p.export_presets_file(),
        json!([a, a2, bad, dis, 3]).to_string(),
    )
    .unwrap();
    let got = s.custom().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target_size_mb, 5.0);
}

#[test]
fn save_replaces_by_id() {
    let (_t, _p, s) = setup();
    s.save(&custom("a", 5.0)).unwrap();
    s.save(&custom("b", 6.0)).unwrap();
    s.save(&custom("a", 9.0)).unwrap();
    let c = s.custom().unwrap();
    assert_eq!(
        c.iter().map(|x| x.preset_id.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(s.get("a").unwrap().unwrap().target_size_mb, 9.0);
    assert!(s.delete("a").unwrap());
    assert!(!s.delete("a").unwrap());
    assert!(!s.delete("discord").unwrap());
}

#[test]
fn width_without_height_rejected() {
    let (_t, _p, s) = setup();
    let mut p = custom("w", 5.0);
    p.target_width = Some(640);
    assert!(p.validate().is_err());
    assert!(matches!(s.save(&p), Err(CoreError::Invalid(_))));
}

#[test]
fn integer_fps_in_existing_file_accepted() {
    let (_t, p, s) = setup();
    let mut v = serde_json::to_value(custom("a", 5.0)).unwrap();
    v["target_fps"] = json!(60);
    std::fs::write(p.export_presets_file(), json!([v]).to_string()).unwrap();
    assert_eq!(s.custom().unwrap()[0].target_fps, Some(60.0));
}

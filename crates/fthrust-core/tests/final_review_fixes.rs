//! Regression tests for the findings of the plan-1 whole-branch review.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

use fthrust_core::FthrPaths;
use fthrust_core::diagnostics::redact::redact_path;
use fthrust_core::diagnostics::session::{DiagnosticsSession, bounded_tail};
use fthrust_core::export_presets::{ExportPreset, ExportPresetStore};
use fthrust_core::hotkeys::HotkeyBindings;
use fthrust_core::presets::PresetsStore;
use fthrust_core::settings::{SettingsStore, defaults};
use fthrust_core::theme::ThemeStore;
use serde_json::{Value, json};

fn paths() -> (tempfile::TempDir, FthrPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let p = FthrPaths::new(tmp.path());
    fs::create_dir_all(p.root()).unwrap();
    (tmp, p)
}

#[test]
fn settings_io_error_uses_defaults_without_quarantine() {
    if nix::unistd::geteuid().is_root() {
        return;
    }
    let (_tmp, p) = paths();
    let file = p.settings_file();
    fs::write(&file, r#"{"clip_length": 45}"#).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let store = SettingsStore::load(p.clone()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        store.data(),
        &defaults(&p),
        "unreadable file falls back to defaults"
    );
    assert!(
        file.exists(),
        "a transient read error must not rename the user's settings"
    );
    assert!(!p.root().join("settings.json.corrupt").exists());
    assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"clip_length": 45}"#);
}

fn theme_zip(
    dir: &std::path::Path,
    theme_json: &str,
    extra: &[(&str, &[u8])],
) -> std::path::PathBuf {
    let path = dir.join("theme.zip");
    let file = fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("theme.json", opts).unwrap();
    zip.write_all(theme_json.as_bytes()).unwrap();
    for (name, body) in extra {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn theme_import_sanitises_colours() {
    let (tmp, p) = paths();
    let mut store = ThemeStore::load(p.clone()).unwrap();
    let archive = theme_zip(
        tmp.path(),
        r##"{"colors": {"ACCENT": "green", "BG": "#aabbccdd", "TEXT": "#123456", "NOT_A_TOKEN": "#000000"},
             "capture_card": {"CAPTURE_CARD_BG": "nope"}}"##,
        &[],
    );
    store.import_zip(&archive).unwrap();
    let defaults = fthrust_core::theme::default_colors();
    assert_eq!(store.color("ACCENT"), defaults["ACCENT"]);
    assert_eq!(store.color("BG"), defaults["BG"]);
    assert_eq!(store.color("TEXT"), "#123456");
    assert!(!store.data.colors.contains_key("NOT_A_TOKEN"));
    assert_eq!(
        store.capture_card_color("CAPTURE_CARD_BG"),
        fthrust_core::theme::default_capture_card_colors()["CAPTURE_CARD_BG"]
    );
    let on_disk: Value =
        serde_json::from_str(&fs::read_to_string(p.theme_file()).unwrap()).unwrap();
    assert_eq!(on_disk["colors"]["ACCENT"], defaults["ACCENT"]);
}

#[test]
fn theme_import_rejects_oversized_entry() {
    let (tmp, p) = paths();
    let mut store = ThemeStore::load(p.clone()).unwrap();
    fs::create_dir_all(p.theme_icons_dir()).unwrap();
    let keep = p.theme_icons_dir().join("keep.svg");
    fs::write(&keep, "<svg/>").unwrap();
    store.set_custom_icon("play.png", &keep).unwrap();
    let big = vec![0u8; 65 * 1024 * 1024];
    let archive = theme_zip(
        tmp.path(),
        r#"{"colors": {}, "icons": {"play.png": "big.png"}}"#,
        &[("icons/big.png", &big)],
    );
    let err = store.import_zip(&archive).unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");
    assert_eq!(
        store.custom_icon_path("play.png"),
        Some(p.theme_icons_dir().join("play.svg"))
    );
    assert!(
        p.theme_icons_dir().join("play.svg").exists(),
        "nothing on disk changes"
    );
}

#[test]
fn export_presets_preserve_unparsed_entries_on_rewrite() {
    let (_tmp, p) = paths();
    let odd = json!({
        "preset_id": "odd", "name": "Odd", "target_size_mb": 5.0, "target_width": null,
        "target_height": null, "target_fps": null, "video_codec": "h264",
        "audio_bitrate_kbps": 128.0, "built_in": false, "safety_margin": 0.9, "future_key": true
    });
    let valid = ExportPreset {
        preset_id: "mine".into(),
        name: "Mine".into(),
        target_size_mb: 8.0,
        target_width: None,
        target_height: None,
        target_fps: None,
        video_codec: "h264".into(),
        audio_bitrate_kbps: 96,
        built_in: false,
        safety_margin: 0.9,
    };
    fs::write(
        p.export_presets_file(),
        serde_json::to_string_pretty(&json!([serde_json::to_value(&valid).unwrap(), odd])).unwrap(),
    )
    .unwrap();
    let store = ExportPresetStore::new(&p);
    assert_eq!(
        store.custom().unwrap().len(),
        1,
        "the odd entry is not offered"
    );
    let other = ExportPreset {
        preset_id: "other".into(),
        name: "Other".into(),
        ..valid.clone()
    };
    store.save(&other).unwrap();
    let on_disk: Value =
        serde_json::from_str(&fs::read_to_string(p.export_presets_file()).unwrap()).unwrap();
    let ids: Vec<&str> = on_disk
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["preset_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"odd"),
        "unparsed entries survive a rewrite: {ids:?}"
    );
    assert!(ids.contains(&"mine") && ids.contains(&"other"));
    let kept = on_disk
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["preset_id"] == "odd")
        .unwrap();
    assert_eq!(kept, &odd, "preserved byte-for-byte as a value");
    assert!(store.delete("mine").unwrap());
    let after: Value =
        serde_json::from_str(&fs::read_to_string(p.export_presets_file()).unwrap()).unwrap();
    assert!(
        after
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["preset_id"] == "odd")
    );
}

#[test]
fn presets_corrupt_file_is_quarantined_before_write() {
    let (_tmp, p) = paths();
    fs::write(p.presets_file(), "{bad json").unwrap();
    let store = PresetsStore::new(&p);
    assert!(store.list_names().unwrap().is_empty());
    let mut values = serde_json::Map::new();
    values.insert("clip_length".into(), json!(60));
    store.save("x", &values).unwrap();
    let corrupt = p.root().join("presets.json.corrupt");
    assert_eq!(fs::read_to_string(&corrupt).unwrap(), "{bad json");
    assert_eq!(store.list_names().unwrap(), vec!["x".to_string()]);
}

#[test]
fn hotkeys_non_utf8_file_loads_defaults() {
    let (_tmp, p) = paths();
    fs::write(p.hotkeys_file(), [0xff, 0xfe, 0x7b]).unwrap();
    let (bindings, needs_resave) = HotkeyBindings::load(&p).unwrap();
    assert_eq!(bindings.get("save_clip"), Some("F9"));
    assert!(!needs_resave);
    assert!(
        p.hotkeys_file().exists(),
        "the file is left for the user to inspect"
    );
}

#[test]
fn redact_path_replaces_home_in_events_and_exports() {
    let home = std::env::var("HOME").unwrap();
    let text = format!(
        "wrote {home}/Videos/FTHR Clips/a.mp4 and {}/x",
        home.to_uppercase()
    );
    let out = redact_path(&text);
    assert!(!out.contains(&home), "{out}");
    assert_eq!(
        out,
        "wrote %USERPROFILE%/Videos/FTHR Clips/a.mp4 and %USERPROFILE%/x"
    );
    assert_eq!(
        redact_path("token=abc at /nowhere"),
        "token=<redacted> at /nowhere"
    );

    let (_tmp, p) = paths();
    let session = DiagnosticsSession::start(&p, "2.0.0").unwrap();
    session.event(
        "library",
        "scan",
        json!({ "root": format!("{home}/Videos") }),
    );
    let events = fs::read_to_string(session.dir().join("events.jsonl")).unwrap();
    assert!(!events.contains(&home), "{events}");
    assert!(events.contains("%USERPROFILE%/Videos"));

    fs::create_dir_all(p.logs_dir()).unwrap();
    fs::write(p.app_log(), format!("opened {home}/Videos/clip.mp4\n")).unwrap();
    let tail = bounded_tail(&p.app_log(), 1024 * 1024);
    assert_eq!(tail, "opened %USERPROFILE%/Videos/clip.mp4\n");
}

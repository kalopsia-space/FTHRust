//! The upload settings widget and the consent dialogs
//! (notes-uploader-reference 4, 5).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fthrust_core::upload::CATBOX_LEGAL_VERSION;
use fthrust_core::upload::settings::UploaderSettings;
use fthrust_ui::settings::upload::{self as up, Choice, PROVIDERS, SFTP_NOTE, StatusKind};
use fthrust_ui::state::{AppState, DialogKind, DialogResult};
use serde_json::{Value, json};

fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("uploader");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

struct Rig {
    dir: tempfile::TempDir,
    state: AppState,
}

/// `answers` maps an action to the reply line.
fn rig(extra: Value, answers: &[(&str, &str)]) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let mut state = AppState::for_tests(dir.path(), json!({}));
    for (k, v) in extra.as_object().unwrap() {
        state.uploader_settings.set(k, v.clone());
    }
    state.uploader_settings.save(&state.paths).unwrap();
    let mut body = String::from(
        "read -r line; echo \"$line\" >> \"$(dirname \"$0\")/requests.jsonl\"\ncase \"$line\" in\n",
    );
    for (action, reply) in answers {
        body.push_str(&format!(
            "  *'\"action\":\"{action}\"'*) echo '{reply}';;\n"
        ));
    }
    body.push_str("  *) echo '{\"ok\":false,\"message\":\"unexpected\"}';;\nesac");
    state.uploader_exe = Some(script(dir.path(), &body));
    Rig { dir, state }
}

fn enabled_rig(extra: Value, answers: &[(&str, &str)]) -> Rig {
    let mut e = json!({
        "upload_enabled": true,
        "catbox_legal_accepted_version": CATBOX_LEGAL_VERSION,
    });
    for (k, v) in extra.as_object().unwrap() {
        e[k] = v.clone();
    }
    rig(e, answers)
}

impl Rig {
    fn saved(&self) -> UploaderSettings {
        UploaderSettings::load(&self.state.paths)
    }

    fn modal(&self) -> Option<DialogKind> {
        self.state.dialogs.modal.back().cloned()
    }

    fn answer(&mut self, r: DialogResult, now: f64) {
        self.state.dialogs.modal.pop_back();
        self.state.dialogs.results.push(r);
        up::service(&mut self.state, now);
    }

    fn settle(&mut self) {
        let start = Instant::now();
        let mut now = 10.0;
        while up::busy(&self.state) {
            up::service(&mut self.state, now);
            assert!(start.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(20));
            now += 0.02;
        }
        up::service(&mut self.state, now);
    }

    fn status(&mut self) -> (String, StatusKind) {
        let f = up::form(&mut self.state);
        (f.status.clone(), f.status_kind)
    }

    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("requests.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

#[test]
fn enable_asks_only_for_catbox() {
    let mut r = rig(json!({}), &[]);
    up::set_enabled(&mut r.state, true, 0.0);
    assert!(r.saved().enabled(), "no terms dialog first");
    // Catbox (the default provider) asks for its own policies.
    let Some(DialogKind::CatboxConsent { context }) = r.modal() else {
        panic!("{:?}", r.modal());
    };
    assert_eq!(
        fthrust_ui::dialogs::title_of(&DialogKind::CatboxConsent {
            context: context.clone()
        }),
        "Connect Catbox"
    );
    r.answer(
        DialogResult::CatboxConsent {
            accepted: true,
            context,
        },
        0.0,
    );
    let s = r.saved();
    assert!(s.enabled() && s.catbox_consent_current());

    // Cancelling the Catbox step during enable turns uploads off again.
    let mut r = rig(json!({}), &[]);
    up::set_enabled(&mut r.state, true, 0.0);
    let Some(DialogKind::CatboxConsent { context }) = r.modal() else {
        panic!()
    };
    r.answer(
        DialogResult::CatboxConsent {
            accepted: false,
            context,
        },
        0.0,
    );
    assert!(!r.saved().enabled());

    // Another provider: no dialog at all; disabling saves at once.
    let mut r = rig(json!({"upload_provider": "custom"}), &[]);
    up::set_enabled(&mut r.state, true, 0.0);
    assert!(r.state.dialogs.modal.is_empty());
    assert!(r.saved().enabled());
    up::set_enabled(&mut r.state, false, 0.0);
    assert!(!r.saved().enabled());
}

/// Terms accepted in an older version (the Qt app's) do not turn uploads
/// off.
#[test]
fn old_terms_versions_keep_uploads_on() {
    let r = enabled_rig(
        json!({"uploader_terms_accepted_version": "2026-08-24-v1",
               "uploader_privacy_accepted_version": "2026-08-24-v1"}),
        &[],
    );
    assert!(fthrust_ui::services::upload::uploader_enabled(&r.state));
}

/// The settings link opens the Terms and Privacy Policy, read only.
#[test]
fn terms_link_opens_the_documents() {
    let mut r = rig(json!({}), &[]);
    up::show_terms(&mut r.state);
    assert_eq!(r.modal(), Some(DialogKind::UploaderTerms));
    assert_eq!(
        fthrust_ui::dialogs::title_of(&DialogKind::UploaderTerms),
        "Uploader Terms & Privacy"
    );
    assert!(!r.saved().enabled(), "viewing changes nothing");
}

#[test]
fn catbox_dialog_gates_accept() {
    use fthrust_ui::dialogs::upload_consent::catbox_accept_enabled;
    assert!(!catbox_accept_enabled(false));
    assert!(catbox_accept_enabled(true));
}

#[test]
fn provider_combo_order_and_panels() {
    let labels: Vec<&str> = PROVIDERS.iter().map(|c| c.label()).collect();
    assert_eq!(labels, vec!["Catbox", "VaultScope", "Your server", "SFTP"]);
    let ids: Vec<&str> = PROVIDERS.iter().map(|c| c.id()).collect();
    assert_eq!(ids, vec!["catbox", "vaultscope", "custom", "sftp"]);
    let p = up::panels(Choice::Catbox);
    assert!(p.catbox && !p.custom && !p.sftp && !p.vaultscope && p.support);
    assert_eq!(p.open, Some("OPEN CATBOX"));
    let p = up::panels(Choice::VaultScope);
    assert!(p.sftp && p.vaultscope && !p.support);
    assert_eq!(p.open, Some("OPEN VAULTSCOPE"));
    let p = up::panels(Choice::Custom);
    assert!(p.custom && !p.sftp && p.open.is_none());
    let p = up::panels(Choice::Sftp);
    assert!(p.sftp && !p.vaultscope && p.open.is_none());
    // Stored `sftp` + VaultScope name reads back as VaultScope.
    let mut r = enabled_rig(
        json!({"upload_provider": "sftp", "sftp_provider_name": "VaultScope"}),
        &[],
    );
    assert_eq!(up::form(&mut r.state).choice, Choice::VaultScope);
    let mut r = enabled_rig(json!({"upload_provider": "own_server"}), &[]);
    assert_eq!(up::form(&mut r.state).choice, Choice::Custom);
}

#[test]
fn switch_to_catbox_without_consent_reverts_on_cancel() {
    let mut r = enabled_rig(
        json!({"upload_provider": "custom", "catbox_legal_accepted_version": ""}),
        &[],
    );
    up::select_provider(&mut r.state, Choice::Catbox);
    let Some(DialogKind::CatboxConsent { context }) = r.modal() else {
        panic!()
    };
    r.answer(
        DialogResult::CatboxConsent {
            accepted: false,
            context,
        },
        0.0,
    );
    assert_eq!(up::form(&mut r.state).choice, Choice::Custom);
    up::select_provider(&mut r.state, Choice::Catbox);
    let Some(DialogKind::CatboxConsent { context }) = r.modal() else {
        panic!()
    };
    r.answer(
        DialogResult::CatboxConsent {
            accepted: true,
            context,
        },
        0.0,
    );
    assert_eq!(up::form(&mut r.state).choice, Choice::Catbox);
    assert!(r.saved().catbox_consent_current());
    // No dialog for the others.
    up::select_provider(&mut r.state, Choice::Sftp);
    assert!(r.state.dialogs.modal.is_empty());
}

#[test]
fn save_writes_fields() {
    let mut r = enabled_rig(json!({"upload_provider": "custom"}), &[]);
    {
        let f = up::form(&mut r.state);
        f.server_url = " clips.example/upload ".into();
        f.auth_header = " Bearer x ".into();
        f.userhash = " h ".into();
        f.mode = "interval".into();
        f.interval_value = "abc".into();
        f.interval_unit = "hours".into();
        f.auto_compress = true;
        f.auto_delete = true;
    }
    up::save(&mut r.state, 10.0);
    r.settle();
    let s = r.saved();
    assert_eq!(s.str("upload_server_url"), "https://clips.example/upload");
    assert_eq!(
        up::form(&mut r.state).server_url,
        "https://clips.example/upload"
    );
    assert_eq!(s.str("upload_auth_header"), "Bearer x");
    assert_eq!(s.str("catbox_userhash"), "h");
    assert_eq!(s.str("upload_provider"), "custom");
    assert_eq!(s.str("upload_mode"), "interval");
    assert_eq!(s.get("upload_interval_value"), Some(&json!(5)));
    assert_eq!(s.str("upload_interval_unit"), "hours");
    assert!(s.flag("upload_auto_compress") && s.flag("upload_auto_delete"));
    assert_eq!(r.status(), ("Settings saved".into(), StatusKind::Neutral));
    up::service(&mut r.state, 12.0);
    assert_eq!(r.status().0, "Settings saved");
    up::service(&mut r.state, 13.1);
    assert_eq!(r.status().0, "Not tested");
    assert_eq!(
        r.state.uploader_settings.str("upload_server_url"),
        "https://clips.example/upload"
    );

    // VaultScope saves as sftp with its name, password cleared and kept out.
    let mut r = enabled_rig(
        json!({"upload_provider": "sftp"}),
        &[(
            "sftp_save",
            r#"{"ok":true,"message":"SFTP credentials updated."}"#,
        )],
    );
    {
        let f = up::form(&mut r.state);
        f.choice = Choice::VaultScope;
        f.sftp_host = " sftp.vaultscope.de ".into();
        f.sftp_port = "2222".into();
        f.sftp_username = "u".into();
        f.sftp_password = " pw ".into();
    }
    up::save(&mut r.state, 0.0);
    r.settle();
    let s = r.saved();
    assert_eq!(s.str("upload_provider"), "sftp");
    assert_eq!(s.str("sftp_provider_name"), "VaultScope");
    assert_eq!(s.str("sftp_host"), "sftp.vaultscope.de");
    assert_eq!(s.get("sftp_port"), Some(&json!("2222")));
    let text = std::fs::read_to_string(r.state.paths.uploader_settings_file()).unwrap();
    assert!(!text.contains(" pw "));
    assert_eq!(up::form(&mut r.state).sftp_password, "");
    let req = &r.requests()[0];
    assert_eq!(req["action"], json!("sftp_save"));
    assert_eq!(
        req["config"]["sftp_password"],
        json!(" pw "),
        "password sent verbatim"
    );
    assert_eq!(req["config"]["sftp_host"], json!("sftp.vaultscope.de"));
}

#[test]
fn sftp_save_failure_saves_nothing() {
    let mut r = enabled_rig(
        json!({"upload_provider": "sftp", "sftp_host": "old.example"}),
        &[(
            "sftp_save",
            r#"{"ok":false,"message":"SFTP port must be between 1 and 65535."}"#,
        )],
    );
    {
        let f = up::form(&mut r.state);
        f.sftp_host = "new.example".into();
        f.sftp_port = "0".into();
    }
    up::save(&mut r.state, 0.0);
    r.settle();
    assert_eq!(r.saved().str("sftp_host"), "old.example");
    assert_eq!(
        r.state.dialogs.messages.last(),
        Some(&DialogKind::Message {
            title: "SFTP settings".into(),
            message: "SFTP port must be between 1 and 65535.".into(),
        })
    );
}

#[test]
fn test_connection_flow() {
    let mut r = enabled_rig(json!({"upload_provider": "custom"}), &[]);
    up::form(&mut r.state).server_url = "  ".into();
    up::test_connection(&mut r.state);
    assert_eq!(r.status().0, "Enter a server URL first");
    assert!(r.requests().is_empty());

    let mut r = enabled_rig(
        json!({"upload_provider": "custom"}),
        &[(
            "test_connection",
            r#"{"ok":true,"message":"Your server reachable (HTTP 200)."}"#,
        )],
    );
    up::form(&mut r.state).server_url = "https://s.example/u".into();
    up::test_connection(&mut r.state);
    assert_eq!(r.status().0, "Testing…");
    assert!(up::form(&mut r.state).testing);
    r.settle();
    assert_eq!(
        r.status(),
        ("Your server reachable (HTTP 200).".into(), StatusKind::Ok)
    );
    assert!(!up::form(&mut r.state).testing);
    let req = &r.requests()[0];
    assert_eq!(req["config"]["upload_provider"], json!("custom"));
    assert_eq!(
        req["config"]["upload_server_url"],
        json!("https://s.example/u")
    );

    let mut r = enabled_rig(
        json!({"upload_provider": "sftp"}),
        &[("test_connection", r#"{"ok":false,"message":""}"#)],
    );
    up::form(&mut r.state).choice = Choice::VaultScope;
    up::test_connection(&mut r.state);
    r.settle();
    assert_eq!(
        r.status(),
        (
            "The upload provider did not return a reason.".into(),
            StatusKind::Error
        )
    );
    assert_eq!(r.requests()[0]["config"]["upload_provider"], json!("sftp"));
    let e = r.state.error_bar.current().unwrap();
    assert_eq!(e.title, "[Error 022] UPLOAD CONNECTION FAILED");
    assert_eq!(e.actions[0].label, "OPEN UPLOAD SETTINGS");

    // Catbox without consent asks first.
    let mut r = enabled_rig(json!({"catbox_legal_accepted_version": ""}), &[]);
    up::test_connection(&mut r.state);
    assert!(matches!(r.modal(), Some(DialogKind::CatboxConsent { .. })));
    assert!(r.requests().is_empty());
}

#[test]
fn forget_password_status() {
    let mut r = enabled_rig(
        json!({"upload_provider": "sftp"}),
        &[(
            "sftp_forget",
            r#"{"ok":true,"message":"SFTP credentials updated."}"#,
        )],
    );
    up::form(&mut r.state).sftp_password = "typed".into();
    up::forget_password(&mut r.state);
    r.settle();
    assert_eq!(r.status().0, "Saved SFTP password removed");
    assert_eq!(up::form(&mut r.state).sftp_password, "");
    let mut r = enabled_rig(
        json!({"upload_provider": "sftp"}),
        &[(
            "sftp_forget",
            r#"{"ok":false,"message":"Could not remove the saved SFTP password."}"#,
        )],
    );
    up::forget_password(&mut r.state);
    r.settle();
    assert_eq!(
        r.state.dialogs.messages.last(),
        Some(&DialogKind::Message {
            title: "SFTP password".into(),
            message: "Could not remove the saved SFTP password.".into(),
        })
    );
}

#[test]
fn vaultscope_paste() {
    let mut r = enabled_rig(json!({"upload_provider": "sftp"}), &[]);
    {
        let f = up::form(&mut r.state);
        f.choice = Choice::VaultScope;
        f.sftp_fingerprint = "SHA256:x".into();
        f.sftp_public_enabled = true;
        f.sftp_public_base_url = "https://old/".into();
        f.sftp_directory = "/x".into();
    }
    up::paste_vaultscope(
        &mut r.state,
        r#"{"providerName":"VaultScope","domain":"sftp.vaultscope.de","port":2022,"username":"me","password":"pw"}"#,
    );
    let f = up::form(&mut r.state);
    assert_eq!(
        (
            f.sftp_host.as_str(),
            f.sftp_port.as_str(),
            f.sftp_username.as_str(),
            f.sftp_password.as_str()
        ),
        ("sftp.vaultscope.de", "2022", "me", "pw")
    );
    assert_eq!(
        (
            f.sftp_directory.as_str(),
            f.sftp_fingerprint.as_str(),
            f.sftp_public_base_url.as_str()
        ),
        (".", "", "")
    );
    assert!(!f.sftp_public_enabled);
    assert_eq!(
        f.status,
        "VaultScope login filled in. Test the connection, then save settings."
    );
    up::paste_vaultscope(&mut r.state, "not json");
    assert_eq!(
        up::form(&mut r.state).sftp_username,
        "me",
        "a bad paste changes nothing"
    );
    let Some(DialogKind::Message { title, .. }) = r.state.dialogs.messages.last() else {
        panic!()
    };
    assert_eq!(title, "VaultScope login");
}

#[test]
fn interval_controls_follow_radio() {
    assert!(up::interval_enabled("interval"));
    assert!(!up::interval_enabled("manual"));
    assert!(!up::interval_enabled("immediate"));
    assert_eq!(up::INTERVAL_VALUES, [1, 2, 5, 10, 15, 30, 60]);
    assert_eq!(
        up::MODES[2].1,
        "Manual only  (right-click a clip \u{2192} Upload)"
    );
}

#[test]
fn note_mentions_keyring() {
    assert!(SFTP_NOTE.contains("Passwords are saved in your desktop keyring."));
    assert!(!SFTP_NOTE.contains("Windows"));
}

#[test]
fn widget_body_only_while_enabled() {
    let draw = |state: &mut AppState| -> Vec<String> {
        let ctx = egui::Context::default();
        fthrust_ui::fonts::install(&ctx, &state.tokens);
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(700.0);
                up::ui(ui, state, 700.0);
            });
            out.textures_delta.clear();
        }
        let _ = fthrust_ui::fidelity::take(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(700.0);
            up::ui(ui, state, 700.0);
        });
        out.textures_delta.clear();
        let names: Vec<String> = fthrust_ui::fidelity::take(&ctx)
            .into_iter()
            .map(|m| m.name)
            .collect();
        names
    };
    let mut r = rig(json!({}), &[]);
    let off = draw(&mut r.state);
    assert!(
        off.iter()
            .any(|n| n == "QCheckBox[Enable optional clip uploader]"),
        "{off:?}"
    );
    assert!(
        off.iter()
            .any(|n| n == "QPushButton[Support Catbox hosting]")
    );
    assert!(!off.iter().any(|n| n == "QPushButton[Test connection]"));
    let mut r = enabled_rig(json!({}), &[]);
    let on = draw(&mut r.state);
    for name in [
        "QPushButton[Test connection]",
        "QPushButton[Save upload settings]",
        "QLabel[Host]",
        "QLabel[Auto Upload]",
    ] {
        assert!(on.iter().any(|n| n == name), "{name} missing: {on:?}");
    }
}

#[test]
fn long_test_status_wraps_inside_the_panel() {
    let draw = |state: &mut AppState| -> egui::Rect {
        let ctx = egui::Context::default();
        fthrust_ui::fonts::install(&ctx, &state.tokens);
        let mut rect = None;
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(700.0);
                up::ui(ui, state, 700.0);
            });
            out.textures_delta.clear();
            rect = fthrust_ui::fidelity::take(&ctx)
                .into_iter()
                .find(|m| m.name == "uploadStatus")
                .map(|m| m.rect);
        }
        rect.expect("uploadStatus marker")
    };
    let mut r = enabled_rig(json!({"upload_provider": "custom"}), &[]);
    up::form(&mut r.state).status = "Your server reachable (HTTP 200).".into();
    let short = draw(&mut r.state);
    up::form(&mut r.state).status = "Your server has nothing at this URL (HTTP 404). Check the \
        server URL; FTHR Clips Server takes uploads at /upload."
        .into();
    up::form(&mut r.state).status_kind = StatusKind::Error;
    let long = draw(&mut r.state);
    assert!(long.max.x <= 700.5, "runs off the panel: {long:?}");
    assert!(
        long.height() > short.height() * 1.5,
        "not wrapped: {long:?} vs {short:?}"
    );
}

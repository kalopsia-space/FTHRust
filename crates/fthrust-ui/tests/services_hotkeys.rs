//! Plan 5 Task 13: hotkey delivery — socket server, Hyprland config,
//! GlobalShortcuts adoption, the copy-config-line button.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use egui::{Pos2, Vec2};
use fthrust_core::hotkeys::{HotkeyAction, HotkeyBindings};
use fthrust_ui::services::hotkeys::{HotkeyServer, apply_compositor_config, prepare_socket_path};
use fthrust_ui::services::portal_shortcuts::adopt_bound;
use fthrust_ui::services::{Event, Outbox};
use fthrust_ui::state::AppState;

fn outbox() -> (Outbox, std::sync::mpsc::Receiver<Event>) {
    let (tx, rx) = channel();
    (Outbox::new(tx, egui::Context::default()), rx)
}

fn wait_hotkey(rx: &std::sync::mpsc::Receiver<Event>, timeout: Duration) -> Option<HotkeyAction> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Hotkey(a)) => return Some(a),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    None
}

fn bindings() -> HotkeyBindings {
    let mut b = HotkeyBindings::defaults();
    b.sync_custom_clips(&[90]);
    b
}

#[test]
fn socket_server_dispatches_known_action_and_custom_clip() {
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    let (out, rx) = outbox();
    let server = HotkeyServer::start(&runtime, bindings(), out).unwrap();
    assert_eq!(server.path(), runtime.join("hotkey.sock"));
    let mode = std::fs::metadata(server.path()).unwrap().permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o600
    );
    let mut c = UnixStream::connect(server.path()).unwrap();
    c.write_all(b"save_clip").unwrap();
    drop(c);
    assert_eq!(
        wait_hotkey(&rx, Duration::from_secs(2)),
        Some(HotkeyAction::SaveClip)
    );
    let mut c = UnixStream::connect(server.path()).unwrap();
    c.write_all(b"save_custom_clip_90\n").unwrap();
    drop(c);
    assert_eq!(
        wait_hotkey(&rx, Duration::from_secs(2)),
        Some(HotkeyAction::SaveCustomClip(90))
    );
    let mut c = UnixStream::connect(server.path()).unwrap();
    c.write_all(b"save_custom_clip_15").unwrap();
    drop(c);
    assert_eq!(
        wait_hotkey(&rx, Duration::from_millis(400)),
        None,
        "a custom length that is not bound is ignored"
    );
    let mut c = UnixStream::connect(server.path()).unwrap();
    c.write_all(b"format_disk").unwrap();
    drop(c);
    assert_eq!(wait_hotkey(&rx, Duration::from_millis(400)), None);
    server.close();
    assert!(
        !runtime.join("hotkey.sock").exists(),
        "closed servers unlink their socket"
    );
}

#[test]
fn socket_server_drops_silent_and_junk_clients_within_500ms() {
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    let (out, rx) = outbox();
    let server = HotkeyServer::start(&runtime, bindings(), out).unwrap();
    let _silent = UnixStream::connect(server.path()).unwrap();
    let mut junk = UnixStream::connect(server.path()).unwrap();
    junk.write_all(&[b'x'; 300]).unwrap();
    let started = Instant::now();
    let mut c = UnixStream::connect(server.path()).unwrap();
    c.write_all(b"save_screenshot").unwrap();
    drop(c);
    assert_eq!(
        wait_hotkey(&rx, Duration::from_secs(2)),
        Some(HotkeyAction::SaveScreenshot)
    );
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "a silent client does not block others: {:?}",
        started.elapsed()
    );
    server.close();
}

#[test]
fn socket_server_refuses_symlink_path() {
    let d = tempfile::tempdir().unwrap();
    let runtime = d.path().join("rt");
    std::fs::create_dir_all(&runtime).unwrap();
    std::os::unix::fs::symlink("/dev/null", runtime.join("hotkey.sock")).unwrap();
    let err = prepare_socket_path(&runtime.join("hotkey.sock")).unwrap_err();
    assert!(err.contains("symlink"), "{err}");
    let (out, _rx) = outbox();
    assert!(HotkeyServer::start(&runtime, bindings(), out).is_err());
    // A plain file is refused; a stale socket of ours is removed.
    let plain = d.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("hotkey.sock"), b"x").unwrap();
    assert!(
        prepare_socket_path(&plain.join("hotkey.sock"))
            .unwrap_err()
            .contains("not a socket")
    );
    let stale = d.path().join("stale");
    std::fs::create_dir_all(&stale).unwrap();
    drop(std::os::unix::net::UnixListener::bind(stale.join("hotkey.sock")).unwrap());
    assert!(prepare_socket_path(&stale.join("hotkey.sock")).is_ok());
    assert!(!stale.join("hotkey.sock").exists());
}

#[test]
fn compositor_config_written_on_start_and_change() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    std::fs::create_dir_all(home.join(".config/hypr")).unwrap();
    std::fs::write(
        home.join(".config/hypr/hyprland.conf"),
        "monitor=,preferred,auto,1\n",
    )
    .unwrap();
    let bin = d.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = d.path().join("hyprctl.log");
    std::fs::write(
        bin.join("hyprctl"),
        format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(
        bin.join("hyprctl"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let mut b = bindings();
    let exe = Path::new("/opt/fthrust/bin/fthrust");
    let sock = Path::new("/run/user/1000/fthrust/hotkey.sock");
    let a = apply_compositor_config(
        &home,
        &b,
        exe,
        sock,
        "hyprland",
        true,
        Some(&bin.join("hyprctl")),
    )
    .unwrap();
    assert!(a.changed && a.reloaded);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "reload");
    assert!(
        std::fs::read_to_string(home.join(".config/hypr/fthr-hotkeys.conf"))
            .unwrap()
            .contains("--hotkey save_clip")
    );
    assert!(
        std::fs::read_to_string(home.join(".config/hypr/hyprland.conf"))
            .unwrap()
            .contains("source =")
    );
    let a = apply_compositor_config(
        &home,
        &b,
        exe,
        sock,
        "hyprland",
        true,
        Some(&bin.join("hyprctl")),
    )
    .unwrap();
    assert!(!a.changed && !a.reloaded, "unchanged config: no reload");
    b.set_keyboard("save_clip", "F5");
    let a = apply_compositor_config(
        &home,
        &b,
        exe,
        sock,
        "hyprland",
        true,
        Some(&bin.join("hyprctl")),
    )
    .unwrap();
    assert!(a.changed && a.reloaded);
    // Not Hyprland: nothing happens.
    let a = apply_compositor_config(
        &home,
        &b,
        exe,
        sock,
        "kwin",
        true,
        Some(&bin.join("hyprctl")),
    )
    .unwrap();
    assert!(!a.changed && !a.reloaded);
    // No instance signature: written but not reloaded.
    b.set_keyboard("save_clip", "F6");
    let a = apply_compositor_config(
        &home,
        &b,
        exe,
        sock,
        "hyprland",
        false,
        Some(&bin.join("hyprctl")),
    )
    .unwrap();
    assert!(a.changed && !a.reloaded);
}

#[test]
fn portal_adopts_desktop_trigger_and_warns() {
    let mut b = bindings();
    let mut requested = BTreeMap::new();
    requested.insert("save_clip".to_string(), "F9".to_string());
    requested.insert("save_screenshot".to_string(), "F12".to_string());
    let cleared = BTreeSet::new();
    let bound = vec![
        ("save_clip".to_string(), "F10, Alt+S".to_string()),
        ("save_screenshot".to_string(), "F12".to_string()),
        ("unknown_action".to_string(), "F1".to_string()),
    ];
    let (changed, kept) = adopt_bound(&mut b, &requested, &cleared, &bound);
    assert!(changed);
    assert_eq!(
        b.get("save_clip"),
        Some("F10"),
        "the desktop's first alternative is adopted"
    );
    assert_eq!(b.get("save_screenshot"), Some("F12"));
    assert_eq!(kept, vec!["Save clip: F10, Alt+S".to_string()]);
    // Cleared actions are not adopted back.
    let mut cleared = BTreeSet::new();
    cleared.insert("save_clip".to_string());
    b.set_keyboard("save_clip", "");
    let (changed, kept) = adopt_bound(&mut b, &requested, &cleared, &bound);
    assert!(!changed && kept.is_empty());
    assert_eq!(b.get("save_clip"), Some(""));
    let warning =
        fthrust_ui::services::portal_shortcuts::kept_warning(
            &["Save clip: F10, Alt+S".to_string()],
        );
    assert_eq!(
        warning,
        "Your desktop manages these keys and kept Save clip: F10, Alt+S. Change them in your desktop shortcut settings."
    );
}

#[test]
fn copy_config_line_button_only_on_hyprland() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = AppState::for_tests(tmp.path(), serde_json::json!({}));
    s.tokens.reduce_motion = true;
    s.compositor = "hyprland".into();
    let tokens = s.tokens.clone();
    let mut h = egui_kittest::Harness::builder()
        .with_size(Vec2::new(600.0, 700.0))
        .with_pixels_per_point(1.0)
        .with_context_setup({
            let tokens = tokens.clone();
            move |ctx| {
                fthrust_ui::fonts::install(ctx, &tokens);
                fthrust_ui::tokens::install_style(ctx, &tokens);
            }
        })
        .build_ui(|ui| {
            let t = s.tokens.clone();
            fthrust_ui::widgets::popup::draw_frame(ui, &t, "hotkeys", Pos2::ZERO, 580.0, |ui| {
                fthrust_ui::popups::hotkeys::content(ui, &mut s)
            });
        });
    h.run_steps(2);
    fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    let copy = markers
        .iter()
        .find(|m| m.name == "QPushButton[COPY LINE]")
        .expect("copy button on Hyprland")
        .rect;
    h.hover_at(copy.center());
    h.step();
    h.drag_at(copy.center());
    h.step();
    h.drop_at(copy.center());
    h.run_steps(2);
    drop(h);
    assert_eq!(
        s.last_copied_bind_line.as_deref(),
        Some(format!("bind = , F9, exec, \"{}\" --hotkey save_clip", s.client_exe).as_str())
    );
    // Elsewhere: no button.
    s.compositor = "kwin".into();
    let mut h = egui_kittest::Harness::builder()
        .with_size(Vec2::new(600.0, 700.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(move |ctx| {
            fthrust_ui::fonts::install(ctx, &tokens);
            fthrust_ui::tokens::install_style(ctx, &tokens);
        })
        .build_ui(|ui| {
            let t = s.tokens.clone();
            fthrust_ui::widgets::popup::draw_frame(ui, &t, "hotkeys", Pos2::ZERO, 580.0, |ui| {
                fthrust_ui::popups::hotkeys::content(ui, &mut s)
            });
        });
    h.run_steps(2);
    fthrust_ui::fidelity::take(&h.ctx);
    h.step();
    let markers = fthrust_ui::fidelity::take(&h.ctx);
    assert!(markers.iter().all(|m| m.name != "QPushButton[COPY LINE]"));
}

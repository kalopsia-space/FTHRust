//! Plan 5 Task 2: Hyprland managed hotkey config (`reference/tests/test_hyprland_binds.py`).

use std::path::Path;

use fthrust_core::hotkeys::HotkeyBindings;
use fthrust_core::hotkeys::hyprland::{
    apply, bind_actions, bind_line, ensure_source_line, setup_instructions, socket_command,
    to_hyprland_bind, write_lua_block, write_managed_conf,
};

const SOCK: &str = "/run/user/1000/fthrust/hotkey.sock";
const EXE: &str = "/opt/fthrust/bin/fthrust";

fn bindings(pairs: &[(&str, &str)]) -> HotkeyBindings {
    let mut b = HotkeyBindings::defaults();
    for (k, v) in pairs {
        b.set_keyboard(k, v);
    }
    b
}

#[test]
fn bind_translation_matches_reference() {
    assert_eq!(
        to_hyprland_bind("Ctrl+Shift+S"),
        ("CTRL SHIFT".to_string(), "S".to_string())
    );
    assert_eq!(to_hyprland_bind("F9"), (String::new(), "F9".to_string()));
    assert_eq!(
        to_hyprland_bind("Alt+F4"),
        ("ALT".to_string(), "F4".to_string())
    );
    assert_eq!(
        to_hyprland_bind("ctrl+shift+s"),
        ("CTRL SHIFT".to_string(), "S".to_string())
    );
    // Named keys become xkb keysym names, which is what Hyprland parses.
    assert_eq!(
        to_hyprland_bind("Win+Page Up"),
        ("WIN".to_string(), "Prior".to_string())
    );
    assert_eq!(
        to_hyprland_bind("Esc"),
        (String::new(), "Escape".to_string())
    );
    assert_eq!(
        to_hyprland_bind("Ctrl+Enter"),
        ("CTRL".to_string(), "Return".to_string())
    );
    assert_eq!(
        to_hyprland_bind("Scroll Lock"),
        (String::new(), "Scroll_Lock".to_string())
    );
    assert_eq!(to_hyprland_bind(""), (String::new(), String::new()));
    assert_eq!(
        socket_command("save_clip", Path::new(EXE), Path::new(SOCK)),
        format!("\"{EXE}\" --hotkey save_clip")
    );
    assert_eq!(
        bind_line("save_clip", "Ctrl+Shift+S", Path::new(EXE), Path::new(SOCK)),
        format!("bind = CTRL SHIFT, S, exec, \"{EXE}\" --hotkey save_clip")
    );
}

#[test]
fn bind_actions_order_and_filtering() {
    let mut b = bindings(&[("stop_recording", "F10"), ("dismiss_game_detection", "F8")]);
    b.set_keyboard("confirm_game_detection", "");
    b.sync_custom_clips(&[90, 15]);
    b.set_keyboard("save_custom_clip_90", "F6");
    assert_eq!(
        bind_actions(&b),
        vec![
            "save_clip",
            "save_screenshot",
            "stop_recording",
            "dismiss_game_detection",
            "save_custom_clip_90"
        ]
    );
}

#[test]
fn managed_conf_written_once_and_unchanged_returns_false() {
    let home = tempfile::tempdir().unwrap();
    let mut b = bindings(&[
        ("save_screenshot", ""),
        ("confirm_game_detection", "F7"),
        ("dismiss_game_detection", "F8"),
    ]);
    assert!(write_managed_conf(home.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    let conf = home.path().join(".config/hypr/fthr-hotkeys.conf");
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(text.starts_with("# FTHR Clips hotkeys — auto-generated, do not edit manually.\n# Change hotkeys inside FTHR Clips → Hotkeys menu.\n"));
    assert!(text.contains("confirm_game_detection") && text.contains("dismiss_game_detection"));
    assert!(!text.contains("save_screenshot"));
    assert!(!text.contains("bind = , ,"));
    assert!(text.ends_with('\n'));
    assert!(
        !write_managed_conf(home.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap(),
        "unchanged content"
    );
    b.set_keyboard("save_clip", "F5");
    assert!(write_managed_conf(home.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    assert!(
        !home
            .path()
            .join(".config/hypr/fthr-hotkeys.conf.tmp")
            .exists()
    );
}

#[test]
fn source_line_appended_once_and_skipped_when_user_has_socket_binds() {
    let home = tempfile::tempdir().unwrap();
    let hypr = home.path().join(".config/hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    // Missing hyprland.conf: nothing is created.
    ensure_source_line(home.path()).unwrap();
    assert!(!hypr.join("hyprland.conf").exists());
    let conf = hypr.join("hyprland.conf");
    std::fs::write(&conf, "monitor=,preferred,auto,1\n").unwrap();
    ensure_source_line(home.path()).unwrap();
    ensure_source_line(home.path()).unwrap();
    let text = std::fs::read_to_string(&conf).unwrap();
    assert_eq!(text.matches("source =").count(), 1);
    assert!(text.ends_with(&format!(
        "# FTHR Clips hotkeys (auto-added)\nsource = {}\n",
        hypr.join("fthr-hotkeys.conf").display()
    )));
    let user = home.path().join("user");
    std::fs::create_dir_all(user.join(".config/hypr")).unwrap();
    std::fs::write(
        user.join(".config/hypr/hyprland.conf"),
        format!("bind = , F9, exec, echo -n save_clip | nc -U {SOCK}\n"),
    )
    .unwrap();
    ensure_source_line(&user).unwrap();
    assert!(
        !std::fs::read_to_string(user.join(".config/hypr/hyprland.conf"))
            .unwrap()
            .contains("source =")
    );
}

#[test]
fn lua_block_replaced_in_place_and_incomplete_block_refused() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("custom/keybinds.lua");
    let b = bindings(&[
        ("confirm_game_detection", "F7"),
        ("save_clip", "Ctrl+Shift+S"),
    ]);
    assert!(write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("-- FTHR Clips hotkeys begin (managed)")
            && text
                .trim_end()
                .ends_with("-- FTHR Clips hotkeys end (managed)")
    );
    assert!(text.contains(&format!("hl.bind(\"CTRL + SHIFT + S\", hl.dsp.exec_cmd(\"\\\"{EXE}\\\" --hotkey save_clip\"), {{ description = \"FTHR Clips: save_clip\" }})")));
    assert!(text.contains("confirm_game_detection"));
    assert!(!write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    // User content before the block survives a rewrite.
    std::fs::write(&path, format!("local hl = require('hl')\n\n{text}")).unwrap();
    let mut b2 = b.clone();
    b2.set_keyboard("save_clip", "F5");
    assert!(write_lua_block(&path, &b2, Path::new(EXE), Path::new(SOCK)).unwrap());
    let text2 = std::fs::read_to_string(&path).unwrap();
    assert!(text2.starts_with("local hl = require('hl')\n"));
    assert_eq!(text2.matches("begin (managed)").count(), 1);
    assert!(text2.contains("hl.bind(\"F5\""));
    std::fs::write(
        &path,
        "-- FTHR Clips hotkeys begin (managed)\nhl.bind(...)\n",
    )
    .unwrap();
    assert!(
        write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).is_err(),
        "incomplete block refused"
    );
}

#[test]
fn apply_prefers_lua_when_present() {
    let home = tempfile::tempdir().unwrap();
    let b = bindings(&[]);
    let a = apply(home.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap();
    assert!(a.changed && !a.lua);
    assert!(home.path().join(".config/hypr/fthr-hotkeys.conf").exists());
    let home2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home2.path().join(".config/hypr")).unwrap();
    std::fs::write(home2.path().join(".config/hypr/hyprland.lua"), "").unwrap();
    let a2 = apply(home2.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap();
    assert!(a2.changed && a2.lua);
    assert!(
        home2
            .path()
            .join(".config/hypr/custom/keybinds.lua")
            .exists()
    );
    assert!(!home2.path().join(".config/hypr/fthr-hotkeys.conf").exists());
}

#[test]
fn setup_instructions_use_the_client_command() {
    let s = setup_instructions("kwin", Path::new(EXE), Path::new(SOCK));
    assert!(s.starts_with(
        "KDE: System Settings -> Keyboard -> Shortcuts -> Add New -> Command or Script\n\n"
    ));
    assert!(s.contains(&format!(
        "\"{EXE}\" --hotkey save_clip\n\"{EXE}\" --hotkey save_screenshot"
    )));
    let g = setup_instructions("gnome", Path::new(EXE), Path::new(SOCK));
    assert!(g.starts_with("GNOME: Settings → Keyboard → Custom Shortcuts\n\n"));
    assert!(g.contains(&format!("bash -c '\"{EXE}\" --hotkey save_clip'")));
    assert!(
        setup_instructions("x11", Path::new(EXE), Path::new(SOCK))
            .starts_with("Bind your preferred keys to these commands:\n\n")
    );
    assert!(!s.contains("nc -U"));
}

#[test]
fn source_line_is_added_next_to_reference_era_manual_binds() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".config/hypr");
    std::fs::create_dir_all(&dir).unwrap();
    let conf = dir.join("hyprland.conf");
    std::fs::write(
        &conf,
        "bind = , F9, exec, echo save_clip | nc -U /run/user/1000/fthr/hotkey.sock\n",
    )
    .unwrap();
    ensure_source_line(home.path()).unwrap();
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(
        text.contains("fthr-hotkeys.conf"),
        "the old socket bind does not count as ours: {text}"
    );
    ensure_source_line(home.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(&conf)
            .unwrap()
            .matches("fthr-hotkeys.conf")
            .count(),
        1
    );
}

// A keybind file the app cannot read is the user's file, not an empty one.

fn lua_path(dir: &Path) -> std::path::PathBuf {
    dir.join("custom/keybinds.lua")
}

fn write_lua(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn lua_leftovers(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn lua_file_that_is_not_utf8_is_never_rewritten() {
    let d = tempfile::tempdir().unwrap();
    let path = lua_path(d.path());
    // A latin-1 comment in an otherwise valid user config.
    let original = b"-- Mod\xe9 keys\nhl.bind(\"SUPER + Q\", hl.dsp.window.close())\n".to_vec();
    write_lua(&path, &original);
    let b = bindings(&[]);
    let err = write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), original, "left untouched");
    assert_eq!(lua_leftovers(&path), ["keybinds.lua"], "no temp or backup");
}

#[test]
fn lua_file_that_cannot_be_read_is_never_rewritten() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let path = lua_path(d.path());
    let original = b"hl.bind(\"SUPER + Q\", hl.dsp.window.close())\n".to_vec();
    write_lua(&path, &original);
    // Write-only: unreadable, but the directory still lets a rewrite through,
    // which is exactly what used to wipe it.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read(&path).is_ok() {
        return; // running as root: the mode does not bind us
    }
    let b = bindings(&[]);
    let r = write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(r.is_err(), "an unreadable file aborts the write");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(lua_leftovers(&path), ["keybinds.lua"]);
}

#[test]
fn lua_rewrite_goes_through_a_temp_file_and_keeps_a_backup() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let path = lua_path(d.path());
    let b = bindings(&[("save_clip", "Ctrl+Shift+S")]);
    let user = "local hl = require('hl')\nhl.bind(\"SUPER + Q\", hl.dsp.window.close())\n";
    write_lua(&path, user.as_bytes());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.starts_with(user.trim_end()) && first.contains("hotkeys begin (managed)"));
    // The backup holds what the file said before this write.
    let bak = path.with_file_name("keybinds.lua.bak");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), user);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640,
        "the file keeps its mode"
    );
    // A second change backs up the first version; an identical write backs up nothing.
    let b2 = bindings(&[("save_clip", "F5")]);
    assert!(write_lua_block(&path, &b2, Path::new(EXE), Path::new(SOCK)).unwrap());
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), first);
    assert!(!write_lua_block(&path, &b2, Path::new(EXE), Path::new(SOCK)).unwrap());
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), first);
    assert_eq!(
        lua_leftovers(&path),
        ["keybinds.lua", "keybinds.lua.bak"],
        "no temporary file is left behind"
    );
}

#[test]
fn lua_rewrite_follows_a_symlink_instead_of_replacing_it() {
    let d = tempfile::tempdir().unwrap();
    let dotfiles = d.path().join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let real = dotfiles.join("keybinds.lua");
    std::fs::write(&real, "-- mine\n").unwrap();
    let path = lua_path(&d.path().join("hypr"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &path).unwrap();
    let b = bindings(&[]);
    assert!(write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the dotfile link survives"
    );
    let text = std::fs::read_to_string(&real).unwrap();
    assert!(text.starts_with("-- mine") && text.contains("hotkeys begin (managed)"));
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("keybinds.lua.bak")).unwrap(),
        "-- mine\n"
    );
}

#[test]
fn a_new_lua_file_needs_no_backup() {
    let d = tempfile::tempdir().unwrap();
    let path = lua_path(d.path());
    let b = bindings(&[]);
    assert!(write_lua_block(&path, &b, Path::new(EXE), Path::new(SOCK)).unwrap());
    assert_eq!(lua_leftovers(&path), ["keybinds.lua"]);
}

#[test]
fn source_line_never_touches_a_hyprland_conf_it_cannot_read() {
    // The `.conf` path only ever appends to hyprland.conf, and gives up on a
    // file it cannot read as text, so it cannot wipe it.
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".config/hypr");
    std::fs::create_dir_all(&dir).unwrap();
    let original = b"# Mod\xe9\nmonitor=,preferred,auto,1\n".to_vec();
    std::fs::write(dir.join("hyprland.conf"), &original).unwrap();
    ensure_source_line(home.path()).unwrap();
    assert_eq!(std::fs::read(dir.join("hyprland.conf")).unwrap(), original);
}

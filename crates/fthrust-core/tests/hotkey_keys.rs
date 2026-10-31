//! Every key egui can report becomes a name Hyprland and the portal parse.
//!
//! The hotkey popup stores egui's key names (`PageUp`, `Equals`, `Backtick`,
//! `Quote`, the brackets, `PrintScreen`...). The Hyprland `.conf` writer, the
//! Lua writer and the GlobalShortcuts trigger all need xkb keysym names.
//!
//! Parsing is not firing: a compositor binds a key by what it types with no
//! modifier held. Shifted punctuation (`Exclamationmark`, `Questionmark`,
//! `Colon`, `Pipe`, the curly brackets, `Plus`) therefore has a keysym here
//! (`exclam`...) that only matches when it is typed without Shift, so the
//! popup records it as the key it sits on when Shift is held
//! ([`unshifted_key_name`], tested at the end of this file).

use std::collections::BTreeSet;
use std::path::Path;

use egui::Key;
use fthrust_core::hotkeys::hyprland::{
    bind_actions, bind_line, to_hyprland_bind, write_lua_block, write_managed_conf,
};
use fthrust_core::hotkeys::{HotkeyBindings, to_portal_trigger, unshifted_key_name};

const SOCK: &str = "/run/user/1000/fthrust/hotkey.sock";
const EXE: &str = "/opt/fthrust/bin/fthrust";

/// The xkb keysym name of each egui key, written out by hand from
/// `xkbcommon-keysyms.h` (letters in lower case; every name was also checked
/// against `xkb_keysym_from_name` of libxkbcommon). `None`: the key has no
/// keysym. The match is exhaustive on purpose: a key added by an egui upgrade
/// stops this file compiling until it has a name.
fn keysym(key: Key) -> Option<&'static str> {
    Some(match key {
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::ArrowUp => "Up",
        Key::Escape => "Escape",
        Key::Tab => "Tab",
        Key::Backspace => "BackSpace",
        Key::Enter => "Return",
        Key::Space => "space",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "Prior",
        Key::PageDown => "Next",
        Key::Copy => "XF86Copy",
        Key::Cut => "XF86Cut",
        Key::Paste => "XF86Paste",
        Key::Colon => "colon",
        Key::Comma => "comma",
        Key::Backslash => "backslash",
        Key::Slash => "slash",
        Key::Pipe => "bar",
        Key::Questionmark => "question",
        Key::Exclamationmark => "exclam",
        Key::OpenBracket => "bracketleft",
        Key::CloseBracket => "bracketright",
        Key::OpenCurlyBracket => "braceleft",
        Key::CloseCurlyBracket => "braceright",
        Key::Backtick => "grave",
        Key::Minus => "minus",
        Key::Period => "period",
        Key::Plus => "plus",
        Key::Equals => "equal",
        Key::Semicolon => "semicolon",
        Key::Quote => "apostrophe",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::A => "a",
        Key::B => "b",
        Key::C => "c",
        Key::D => "d",
        Key::E => "e",
        Key::F => "f",
        Key::G => "g",
        Key::H => "h",
        Key::I => "i",
        Key::J => "j",
        Key::K => "k",
        Key::L => "l",
        Key::M => "m",
        Key::N => "n",
        Key::O => "o",
        Key::P => "p",
        Key::Q => "q",
        Key::R => "r",
        Key::S => "s",
        Key::T => "t",
        Key::U => "u",
        Key::V => "v",
        Key::W => "w",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::F13 => "F13",
        Key::F14 => "F14",
        Key::F15 => "F15",
        Key::F16 => "F16",
        Key::F17 => "F17",
        Key::F18 => "F18",
        Key::F19 => "F19",
        Key::F20 => "F20",
        Key::F21 => "F21",
        Key::F22 => "F22",
        Key::F23 => "F23",
        Key::F24 => "F24",
        Key::F25 => "F25",
        Key::F26 => "F26",
        Key::F27 => "F27",
        Key::F28 => "F28",
        Key::F29 => "F29",
        Key::F30 => "F30",
        Key::F31 => "F31",
        Key::F32 => "F32",
        Key::F33 => "F33",
        Key::F34 => "F34",
        Key::F35 => "F35",
        Key::BrowserBack => "XF86Back",
        Key::BrowserForward => "XF86Forward",
        Key::BrowserRefresh => "XF86Refresh",
        Key::BrowserSearch => "XF86Search",
        Key::BrowserHome => "XF86HomePage",
        Key::BrowserFavorites => "XF86Favorites",
        Key::BrowserStop => "XF86Stop",
        Key::MediaPlayPause => "XF86AudioPlay",
        Key::MediaTrackNext => "XF86AudioNext",
        Key::MediaTrackPrevious => "XF86AudioPrev",
        Key::MediaStop => "XF86AudioStop",
        Key::AudioVolumeMute => "XF86AudioMute",
        Key::AudioVolumeDown => "XF86AudioLowerVolume",
        Key::AudioVolumeUp => "XF86AudioRaiseVolume",
        Key::LaunchMail => "XF86Mail",
        Key::LaunchApp1 => "XF86MyComputer",
        Key::LaunchApp2 => "XF86Calculator",
        Key::ShiftLeft => "Shift_L",
        Key::ShiftRight => "Shift_R",
        Key::ControlLeft => "Control_L",
        Key::ControlRight => "Control_R",
        Key::AltLeft => "Alt_L",
        Key::AltRight => "Alt_R",
        Key::SuperLeft => "Super_L",
        Key::SuperRight => "Super_R",
        Key::IntlBackslash => "less",
        Key::CapsLock => "Caps_Lock",
        Key::NumLock => "Num_Lock",
        Key::ScrollLock => "Scroll_Lock",
        Key::PrintScreen => "Print",
        Key::Pause => "Pause",
        Key::Menu => "Menu",
        Key::Fn => return None,
        Key::Eject => "XF86Eject",
        Key::Help => "Help",
        Key::Power => "XF86PowerOff",
        Key::Sleep => "XF86Sleep",
        Key::Clear => "Clear",
    })
}

/// What the Hyprland writers print for a keysym: letters upper-cased, as the
/// reference did.
fn hyprland_spelling(sym: &str) -> String {
    if sym.chars().count() == 1 {
        sym.to_uppercase()
    } else {
        sym.to_owned()
    }
}

/// The chord the hotkey popup builds for `key` with Ctrl and Shift held.
fn chord(key: Key) -> String {
    format!("Ctrl+Shift+{}", key.name())
}

/// One custom-clip action per egui key, each bound to its key: a single
/// `.conf` and a single Lua file then show what every key turns into.
fn all_keys_bound() -> (HotkeyBindings, Vec<(Key, String)>) {
    let mut b = HotkeyBindings::defaults();
    let lengths: Vec<u32> = (0..Key::ALL.len() as u32).map(|i| 5 + i).collect();
    b.sync_custom_clips(&lengths);
    let mut actions = Vec::new();
    for (key, n) in Key::ALL.iter().zip(&lengths) {
        let action = format!("save_custom_clip_{n}");
        b.set_keyboard(&action, &chord(*key));
        actions.push((*key, action));
    }
    // The fixed actions keep their defaults out of the way.
    for fixed in [
        "save_clip",
        "save_screenshot",
        "confirm_game_detection",
        "dismiss_game_detection",
    ] {
        b.set_keyboard(fixed, "");
    }
    (b, actions)
}

#[test]
fn every_egui_key_has_a_verdict() {
    let without: Vec<_> = Key::ALL.iter().filter(|k| keysym(**k).is_none()).collect();
    assert_eq!(without, [&Key::Fn], "only Fn has no keysym");
}

#[test]
fn hyprland_bind_and_portal_trigger_use_the_keysym() {
    for &key in Key::ALL {
        let mut b = HotkeyBindings::defaults();
        b.set_keyboard("save_clip", &chord(key));
        let stored = b.get("save_clip").unwrap().to_owned();
        let (mods, hypr) = to_hyprland_bind(&stored);
        let portal = to_portal_trigger(&stored);
        match keysym(key) {
            Some(sym) => {
                assert_eq!(mods, "CTRL SHIFT", "{key:?} stored as {stored}");
                assert_eq!(hypr, hyprland_spelling(sym), "{key:?} stored as {stored}");
                assert_eq!(
                    portal,
                    format!("CTRL+SHIFT+{sym}"),
                    "{key:?} stored as {stored}"
                );
            }
            None => {
                // Nothing to translate to: the portal trigger keeps the name.
                assert_eq!(portal, format!("CTRL+SHIFT+{}", key.name()));
            }
        }
    }
}

#[test]
fn the_conf_and_the_lua_file_bind_every_key_that_has_a_keysym() {
    let (b, actions) = all_keys_bound();
    let bound: BTreeSet<String> = bind_actions(&b).into_iter().collect();

    let home = tempfile::tempdir().unwrap();
    write_managed_conf(home.path(), &b, Path::new(EXE), Path::new(SOCK)).unwrap();
    let conf = std::fs::read_to_string(home.path().join(".config/hypr/fthr-hotkeys.conf")).unwrap();
    let lua_path = home.path().join("keybinds.lua");
    write_lua_block(&lua_path, &b, Path::new(EXE), Path::new(SOCK)).unwrap();
    let lua = std::fs::read_to_string(&lua_path).unwrap();

    for (key, action) in &actions {
        let command = format!("\"{EXE}\" --hotkey {action}");
        match keysym(*key) {
            Some(sym) => {
                let hypr = hyprland_spelling(sym);
                assert!(bound.contains(action), "{key:?} is bound");
                assert!(
                    conf.lines()
                        .any(|l| l == format!("bind = CTRL SHIFT, {hypr}, exec, {command}")),
                    "{key:?} in the .conf: {conf}"
                );
                let lua_bind = format!("hl.bind(\"CTRL + SHIFT + {hypr}\", ");
                assert!(
                    lua.lines()
                        .any(|l| l.starts_with(&lua_bind) && l.contains(action.as_str())),
                    "{key:?} in the Lua file"
                );
            }
            None => {
                assert!(
                    !bound.contains(action),
                    "{key:?} has no keysym and is left out"
                );
                assert!(
                    !conf.contains(action.as_str()),
                    "{key:?} is not in the .conf"
                );
                assert!(
                    !lua.contains(action.as_str()),
                    "{key:?} is not in the Lua file"
                );
            }
        }
    }
}

#[test]
fn the_display_names_of_the_reference_and_the_keysyms_themselves_still_work() {
    for (typed, sym) in [
        ("Page Up", "Prior"),
        ("PageUp", "Prior"),
        ("Pageup", "Prior"),
        ("page_up", "Prior"),
        ("PgUp", "Prior"),
        ("Prior", "Prior"),
        ("Page Down", "Next"),
        ("Esc", "Escape"),
        ("Enter", "Return"),
        ("Backspace", "BackSpace"),
        ("Scroll Lock", "Scroll_Lock"),
        ("Num Lock", "Num_Lock"),
        ("Print Screen", "Print"),
        ("Equal", "equal"),
        ("grave", "grave"),
        ("F13", "F13"),
        ("f35", "F35"),
    ] {
        assert_eq!(to_hyprland_bind(typed).1, sym, "{typed}");
        assert_eq!(to_portal_trigger(typed), sym, "{typed}");
    }
    // Punctuation typed as the character itself.
    for (typed, sym) in [
        ("=", "equal"),
        ("-", "minus"),
        ("[", "bracketleft"),
        (";", "semicolon"),
        ("`", "grave"),
        ("/", "slash"),
    ] {
        assert_eq!(to_hyprland_bind(typed).1, sym, "{typed}");
        assert_eq!(
            to_portal_trigger(&format!("Ctrl+{typed}")),
            format!("CTRL+{sym}")
        );
    }
    // Letters and digits: upper case for Hyprland, lower case for the portal.
    assert_eq!(to_hyprland_bind("Ctrl+s").1, "S");
    assert_eq!(to_portal_trigger("Ctrl+S"), "CTRL+s");
    assert_eq!(to_hyprland_bind("Alt+7").1, "7");
    assert_eq!(to_portal_trigger("Alt+7"), "ALT+7");
}

/// The key each shifted symbol egui reports sits on in a US layout.
fn expected_base(key: Key) -> Option<Key> {
    Some(match key {
        Key::Exclamationmark => Key::Num1,
        Key::Questionmark => Key::Slash,
        Key::Colon => Key::Semicolon,
        Key::Pipe => Key::Backslash,
        Key::OpenCurlyBracket => Key::OpenBracket,
        Key::CloseCurlyBracket => Key::CloseBracket,
        Key::Plus => Key::Equals,
        _ => return None,
    })
}

#[test]
fn shifted_punctuation_is_recorded_as_the_key_it_is_typed_on() {
    for &key in Key::ALL {
        let recorded = unshifted_key_name(key.name());
        match expected_base(key) {
            Some(base) => {
                let name = recorded.unwrap_or_else(|| panic!("{key:?} has a base key"));
                // egui itself accepts the recorded name, as the key it should be.
                assert_eq!(Key::from_name(name), Some(base), "{key:?} -> {name}");
                // And what the compositor is told is the unshifted key.
                let mut b = HotkeyBindings::defaults();
                b.set_keyboard("save_clip", &format!("Ctrl+Shift+{name}"));
                let stored = b.get("save_clip").unwrap().to_owned();
                let (mods, hypr) = to_hyprland_bind(&stored);
                assert_eq!(mods, "CTRL SHIFT");
                let shifted = to_hyprland_bind(&format!("Ctrl+Shift+{}", key.name())).1;
                assert_ne!(
                    hypr, shifted,
                    "{key:?}: the base key is not the shifted keysym"
                );
            }
            None => assert_eq!(recorded, None, "{key:?} is recorded as it is"),
        }
    }
    // The spellings the popup stores after normalising.
    assert_eq!(unshifted_key_name("Questionmark"), Some("Slash"));
    assert_eq!(unshifted_key_name("Exclamationmark"), Some("1"));
    assert_eq!(unshifted_key_name("Opencurlybracket"), Some("OpenBracket"));
    // Letters, digits and the unshifted punctuation stay as they are.
    for name in ["S", "1", "Slash", "Equals", "Semicolon", "F9", "PageUp"] {
        assert_eq!(unshifted_key_name(name), None, "{name}");
    }
}

#[test]
fn a_key_nothing_knows_is_left_out_of_the_compositor_config() {
    let mut b = HotkeyBindings::defaults();
    b.set_keyboard("save_clip", "Ctrl+Frobnicate");
    b.set_keyboard("save_screenshot", "Fn");
    b.set_keyboard("start_recording", "Ctrl+Shift");
    assert!(!bind_actions(&b).iter().any(|a| matches!(
        a.as_str(),
        "save_clip" | "save_screenshot" | "start_recording"
    )));
    // bind_line still prints what it is given, for the "copy bind line" button.
    assert_eq!(
        bind_line(
            "save_clip",
            "Ctrl+Frobnicate",
            Path::new(EXE),
            Path::new(SOCK)
        ),
        format!("bind = CTRL, Frobnicate, exec, \"{EXE}\" --hotkey save_clip")
    );
}

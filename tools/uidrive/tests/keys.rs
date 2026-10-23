use uidrive::keys::{char_key, parse_chord};

#[test]
fn parse_chord_examples() {
    let c = parse_chord("Ctrl+Shift+S").unwrap();
    assert!(c.mods.ctrl && c.mods.shift && !c.mods.alt && !c.mods.logo);
    assert_eq!(c.keysym, xkbcommon::xkb::keysyms::KEY_s);
    assert_eq!(c.keycode, 31);
    let f9 = parse_chord("F9").unwrap();
    assert_eq!(f9.keysym, xkbcommon::xkb::keysyms::KEY_F9);
    assert_eq!(f9.keycode, 67);
    assert_eq!(parse_chord("Escape").unwrap().keycode, 1);
    let r = parse_chord("super+Return").unwrap();
    assert!(r.mods.logo);
    assert_eq!(r.keycode, 28);
    assert_eq!(parse_chord("ctrl+a").unwrap().keycode, 30);
}

#[test]
fn unknown_key_is_an_error() {
    assert_eq!(
        parse_chord("Ctrl+Bogus").unwrap_err(),
        "unknown key 'Bogus'"
    );
    assert!(parse_chord("").is_err());
    assert!(parse_chord("Ctrl+").is_err());
}

#[test]
fn char_key_us() {
    assert_eq!(char_key('a'), Some((30, false)));
    assert_eq!(char_key('A'), Some((30, true)));
    assert_eq!(char_key('!'), Some((2, true)));
    assert_eq!(char_key(' '), Some((57, false)));
    assert_eq!(char_key('/'), Some((53, false)));
    assert_eq!(char_key('?'), Some((53, true)));
    assert_eq!(char_key('\n'), Some((28, false)));
    assert_eq!(char_key('é'), None);
}

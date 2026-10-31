//! The one table from key names to xkb keysym names.
//!
//! The Hyprland `.conf` writer, the Hyprland Lua writer and the
//! GlobalShortcuts portal trigger all need a key as the compositor spells
//! it: an xkb keysym name (`Prior`, `equal`, `grave`), not the name the
//! hotkey popup stored (egui's `PageUp`, `Equals`, `Backtick`, which
//! `normalize_keyboard_combo` title-cases to `Pageup`, `Equals`, `Backtick`)
//! or the reference's display name (`Page Up`). All three go through
//! [`keysym`].

use std::borrow::Cow;

/// Named keys: every accepted spelling (lower case, without spaces, `_` or
/// `-`) and the keysym name. Covers every key egui can report
/// (`egui::Key::ALL`, checked by a test), the reference's display names and
/// the keysym names themselves.
const NAMED: &[(&[&str], &str)] = &[
    // Editing and navigation.
    (&["escape", "esc"], "Escape"),
    (&["tab"], "Tab"),
    (&["backspace", "bksp"], "BackSpace"),
    (&["enter", "return"], "Return"),
    (&["space", "spacebar"], "space"),
    (&["insert", "ins"], "Insert"),
    (&["delete", "del"], "Delete"),
    (&["home"], "Home"),
    (&["end"], "End"),
    (&["pageup", "pgup", "prior"], "Prior"),
    (&["pagedown", "pgdown", "pgdn", "next"], "Next"),
    (&["up", "arrowup"], "Up"),
    (&["down", "arrowdown"], "Down"),
    (&["left", "arrowleft"], "Left"),
    (&["right", "arrowright"], "Right"),
    (&["copy"], "XF86Copy"),
    (&["cut"], "XF86Cut"),
    (&["paste"], "XF86Paste"),
    // Punctuation.
    (&["colon"], "colon"),
    (&["comma"], "comma"),
    (&["backslash"], "backslash"),
    (&["slash"], "slash"),
    (&["pipe", "bar"], "bar"),
    (&["questionmark", "question"], "question"),
    (&["exclamationmark", "exclam"], "exclam"),
    (&["openbracket", "bracketleft"], "bracketleft"),
    (&["closebracket", "bracketright"], "bracketright"),
    (&["opencurlybracket", "braceleft"], "braceleft"),
    (&["closecurlybracket", "braceright"], "braceright"),
    (&["backtick", "backquote", "grave"], "grave"),
    (&["minus"], "minus"),
    (&["period"], "period"),
    (&["plus"], "plus"),
    (&["equals", "equal"], "equal"),
    (&["semicolon"], "semicolon"),
    (&["quote", "apostrophe"], "apostrophe"),
    // Browser, media and launch keys.
    (&["browserback"], "XF86Back"),
    (&["browserforward"], "XF86Forward"),
    (&["browserrefresh"], "XF86Refresh"),
    (&["browsersearch"], "XF86Search"),
    (&["browserhome"], "XF86HomePage"),
    (&["browserfavorites"], "XF86Favorites"),
    (&["browserstop"], "XF86Stop"),
    (&["mediaplaypause"], "XF86AudioPlay"),
    (&["mediatracknext"], "XF86AudioNext"),
    (&["mediatrackprevious"], "XF86AudioPrev"),
    (&["mediastop"], "XF86AudioStop"),
    (&["audiovolumemute"], "XF86AudioMute"),
    (&["audiovolumedown"], "XF86AudioLowerVolume"),
    (&["audiovolumeup"], "XF86AudioRaiseVolume"),
    (&["launchmail"], "XF86Mail"),
    (&["launchapp1"], "XF86MyComputer"),
    (&["launchapp2"], "XF86Calculator"),
    // Modifier keys egui reports as keys of their own.
    (&["shiftleft"], "Shift_L"),
    (&["shiftright"], "Shift_R"),
    (&["controlleft"], "Control_L"),
    (&["controlright"], "Control_R"),
    (&["altleft"], "Alt_L"),
    (&["altright"], "Alt_R"),
    (&["superleft"], "Super_L"),
    (&["superright"], "Super_R"),
    // The ISO key next to the left Shift: `less` on the layouts that have it.
    (&["intlbackslash"], "less"),
    // Locks and system keys.
    (&["capslock"], "Caps_Lock"),
    (&["numlock"], "Num_Lock"),
    (&["scrolllock"], "Scroll_Lock"),
    (&["printscreen", "print", "prtsc", "sysrq"], "Print"),
    (&["pause", "break"], "Pause"),
    (&["menu", "contextmenu", "apps"], "Menu"),
    (&["eject"], "XF86Eject"),
    (&["help"], "Help"),
    (&["power"], "XF86PowerOff"),
    (&["sleep"], "XF86Sleep"),
    (&["clear"], "Clear"),
    // `Fn` has no keysym: the keyboard handles it itself.
];

/// Keysym names of the ASCII punctuation that can stand alone as a key.
fn punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        ' ' => "space",
        '!' => "exclam",
        '"' => "quotedbl",
        '#' => "numbersign",
        '$' => "dollar",
        '%' => "percent",
        '&' => "ampersand",
        '\'' => "apostrophe",
        '(' => "parenleft",
        ')' => "parenright",
        '*' => "asterisk",
        '+' => "plus",
        ',' => "comma",
        '-' => "minus",
        '.' => "period",
        '/' => "slash",
        ':' => "colon",
        ';' => "semicolon",
        '<' => "less",
        '=' => "equal",
        '>' => "greater",
        '?' => "question",
        '@' => "at",
        '[' => "bracketleft",
        '\\' => "backslash",
        ']' => "bracketright",
        '^' => "asciicircum",
        '_' => "underscore",
        '`' => "grave",
        '{' => "braceleft",
        '|' => "bar",
        '}' => "braceright",
        '~' => "asciitilde",
        _ => return None,
    })
}

/// Lower case without spaces, underscores and hyphens: `Page Up`, `PageUp`,
/// `Pageup` and `page_up` all become `pageup`.
fn canonical(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The xkb keysym name of the key a combo ends in, in the spelling both
/// Hyprland and the GlobalShortcuts portal parse (`xkb_keysym_from_name`).
///
/// Letters come back lower case (`s`): the Hyprland writers upper-case them
/// the way the reference did. Digits and `F1`..`F35` come back as they are.
/// `None` for a key that has no keysym (`Fn`) or that nothing here knows,
/// so a writer never emits a name the compositor would reject.
pub(crate) fn keysym(key: &str) -> Option<Cow<'static, str>> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(Cow::Owned(c.to_ascii_lowercase().to_string()));
        }
        if let Some(name) = punctuation(c) {
            return Some(Cow::Borrowed(name));
        }
        // A letter outside ASCII is left to the compositor's own lookup.
        return c
            .is_alphabetic()
            .then(|| Cow::Owned(c.to_lowercase().collect()));
    }
    let name = canonical(key);
    if let Some(n) = name
        .strip_prefix('f')
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (1..=35).contains(n))
    {
        return Some(Cow::Owned(format!("F{n}")));
    }
    NAMED
        .iter()
        .find(|(aliases, _)| aliases.contains(&name.as_str()))
        .map(|(_, keysym)| Cow::Borrowed(*keysym))
}

/// Whether two key names are the same key: the same keysym, so `PageUp`,
/// `Page Up` and `Prior` are one key; names with no keysym are compared by
/// their canonical spelling.
pub(crate) fn same_key(a: &str, b: &str) -> bool {
    match (keysym(a), keysym(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => canonical(a) == canonical(b),
        _ => false,
    }
}

/// The punctuation egui reports as a key of its own when it is typed with
/// Shift held on a US-style layout, and the key it sits on (as egui names it),
/// then the same symbols as the characters themselves, which is how the Qt
/// app wrote them to `hotkeys.json` (`Ctrl+Shift+!`). `+` is not among the
/// characters: it separates the parts of a chord.
const SHIFTED_ON: &[(&str, &str)] = &[
    ("exclamationmark", "1"),
    ("questionmark", "Slash"),
    ("colon", "Semicolon"),
    ("pipe", "Backslash"),
    ("opencurlybracket", "OpenBracket"),
    ("closecurlybracket", "CloseBracket"),
    ("plus", "Equals"),
    ("!", "1"),
    ("?", "Slash"),
    (":", "Semicolon"),
    ("|", "Backslash"),
    ("{", "OpenBracket"),
    ("}", "CloseBracket"),
];

/// The key a shifted symbol is typed on, by egui's names: `Exclamationmark`
/// (or `!`) is `1` with Shift, `Questionmark` (`?`) is `Slash`, `Colon` (`:`)
/// is `Semicolon`, `Pipe` (`|`) is `Backslash`, the curly brackets (`{`, `}`)
/// are the square ones and `Plus` is `Equals` (the US layout; other layouts
/// put these elsewhere, and `None` is returned for every key that is not one
/// of them).
///
/// A compositor binds a key by what it types with no modifier held, so
/// `Shift+Exclamationmark` (keysym `exclam`) never fires on Hyprland while
/// `Shift+1` does. The hotkey popup records the base key when Shift is held,
/// and [`bound_key`] applies the same rule to a chord stored before it did.
pub(crate) fn unshifted_base(name: &str) -> Option<&'static str> {
    let name = canonical(name);
    SHIFTED_ON
        .iter()
        .find(|(shifted, _)| *shifted == name)
        .map(|(_, base)| *base)
}

/// The key a chord is bound on: with Shift among its modifiers a shifted
/// symbol is bound on the key it is typed on ([`unshifted_base`]), so a chord
/// an earlier version stored as `Ctrl+Shift+Exclamationmark`, or the Qt app as
/// `Ctrl+Shift+!`, is written as `Ctrl+Shift+1` and fires; any other key is as
/// it was given.
pub(crate) fn bound_key(key: &str, shift: bool) -> &str {
    if shift {
        unshifted_base(key).unwrap_or(key)
    } else {
        key
    }
}

/// The key part of a combo (`Ctrl+Shift+S` → `S`).
pub(crate) fn key_part(combo: &str) -> &str {
    combo.rsplit('+').next().unwrap_or("")
}

/// Whether the combo ends in a key a compositor can bind.
pub(crate) fn is_bindable(combo: &str) -> bool {
    let key = key_part(combo);
    !key.is_empty() && keysym(key).is_some()
}

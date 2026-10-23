//! Key chords and the US keymap the virtual keyboard uses.

use xkbcommon::xkb::keysyms as ks;

/// Modifier state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
    /// Super / Logo.
    pub logo: bool,
}

/// A parsed chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    /// Modifiers.
    pub mods: Mods,
    /// The key's xkb keysym (unshifted).
    pub keysym: u32,
    /// The key's evdev keycode.
    pub keycode: u32,
}

/// `(keycode, keysym)` of a named key on the US layout.
fn named_key(name: &str) -> Option<(u32, u32)> {
    let lower = name.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f')
        && let Ok(n) = n.parse::<u32>()
        && (1..=24).contains(&n)
    {
        let keycode = match n {
            1..=10 => 58 + n,
            11 => 87,
            12 => 88,
            _ => 183 + (n - 13),
        };
        return Some((keycode, ks::KEY_F1 + (n - 1)));
    }
    let one = lower.chars().next()?;
    if lower.chars().count() == 1 {
        if one.is_ascii_lowercase() {
            let (code, _) = char_key(one)?;
            return Some((code, ks::KEY_a + (one as u32 - 'a' as u32)));
        }
        if one.is_ascii_digit() {
            let (code, _) = char_key(one)?;
            return Some((code, ks::KEY_0 + (one as u32 - '0' as u32)));
        }
    }
    Some(match lower.as_str() {
        "escape" | "esc" => (1, ks::KEY_Escape),
        "enter" | "return" => (28, ks::KEY_Return),
        "tab" => (15, ks::KEY_Tab),
        "space" => (57, ks::KEY_space),
        "backspace" => (14, ks::KEY_BackSpace),
        "delete" => (111, ks::KEY_Delete),
        "left" => (105, ks::KEY_Left),
        "right" => (106, ks::KEY_Right),
        "up" => (103, ks::KEY_Up),
        "down" => (108, ks::KEY_Down),
        "home" => (102, ks::KEY_Home),
        "end" => (107, ks::KEY_End),
        "pageup" => (104, ks::KEY_Page_Up),
        "pagedown" => (109, ks::KEY_Page_Down),
        "minus" => (12, ks::KEY_minus),
        "equal" => (13, ks::KEY_equal),
        "comma" => (51, ks::KEY_comma),
        "period" => (52, ks::KEY_period),
        "slash" => (53, ks::KEY_slash),
        _ => return None,
    })
}

/// Parses `Ctrl+Shift+S`, `F9`, `super+Return`, …
pub fn parse_chord(s: &str) -> Result<Chord, String> {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(format!("bad chord '{s}'"));
    }
    let mut mods = Mods::default();
    let key = parts[parts.len() - 1];
    for m in &parts[..parts.len() - 1] {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            "super" | "logo" | "win" => mods.logo = true,
            other => return Err(format!("unknown modifier '{other}'")),
        }
    }
    let (keycode, keysym) = named_key(key).ok_or_else(|| format!("unknown key '{key}'"))?;
    Ok(Chord {
        mods,
        keysym,
        keycode,
    })
}

/// `(evdev keycode, shift)` for a printable ASCII character on the US layout.
pub fn char_key(c: char) -> Option<(u32, bool)> {
    const ROW1: &str = "1234567890-=";
    const ROW1_SHIFT: &str = "!@#$%^&*()_+";
    const ROW2: &str = "qwertyuiop[]";
    const ROW2_SHIFT: &str = "QWERTYUIOP{}";
    const ROW3: &str = "asdfghjkl;'";
    const ROW3_SHIFT: &str = "ASDFGHJKL:\"";
    const ROW4: &str = "zxcvbnm,./";
    const ROW4_SHIFT: &str = "ZXCVBNM<>?";
    let rows: [(&str, &str, u32); 4] = [
        (ROW1, ROW1_SHIFT, 2),
        (ROW2, ROW2_SHIFT, 16),
        (ROW3, ROW3_SHIFT, 30),
        (ROW4, ROW4_SHIFT, 44),
    ];
    for (plain, shifted, base) in rows {
        if let Some(i) = plain.find(c) {
            return Some((base + i as u32, false));
        }
        if let Some(i) = shifted.find(c) {
            return Some((base + i as u32, true));
        }
    }
    Some(match c {
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        _ => return None,
    })
}

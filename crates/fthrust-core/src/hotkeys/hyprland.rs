//! Hyprland managed hotkey config (`HotkeyManager._write_hyprland_config`
//! and friends): `~/.config/hypr/fthr-hotkeys.conf` with one `bind` line
//! per bound socket action, a `source =` line appended to `hyprland.conf`
//! once, or a managed block in a Lua config. The client command is
//! `"<fthrust>" --hotkey <action>` (the reference used `echo | nc -U`).

use std::path::{Path, PathBuf};

use super::HotkeyBindings;

const SOCKET_ACTIONS: [&str; 6] = [
    "save_clip",
    "save_screenshot",
    "start_recording",
    "stop_recording",
    "confirm_game_detection",
    "dismiss_game_detection",
];
const LUA_BEGIN: &str = "-- FTHR Clips hotkeys begin (managed)";
const LUA_END: &str = "-- FTHR Clips hotkeys end (managed)";
const HEADER: &str = "# FTHR Clips hotkeys — auto-generated, do not edit manually.\n# Change hotkeys inside FTHR Clips → Hotkeys menu.\n";

/// `~/.config/hypr` under `home`.
pub fn hypr_dir(home: &Path) -> PathBuf {
    home.join(".config").join("hypr")
}

/// The managed file.
pub fn managed_conf(home: &Path) -> PathBuf {
    hypr_dir(home).join("fthr-hotkeys.conf")
}

/// `_to_hyprland_bind`: `Ctrl+Shift+S` → `("CTRL SHIFT", "S")`.
pub fn to_hyprland_bind(combo: &str) -> (String, String) {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.len() == 1 {
        if parts[0].is_empty() {
            return (String::new(), String::new());
        }
        return (String::new(), hypr_key(parts[0]));
    }
    let mods = parts[..parts.len() - 1]
        .iter()
        .map(|p| p.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ");
    let last = parts[parts.len() - 1];
    (mods, hypr_key(last))
}

/// Hyprland parses xkb keysym names: single characters upper-cased as the
/// reference did, named keys through the keysym table (`Page Up` → `Prior`).
fn hypr_key(key: &str) -> String {
    if key.chars().count() == 1 {
        return key.to_uppercase();
    }
    super::portal_key(key)
        .map(str::to_owned)
        .unwrap_or_else(|| key.to_owned())
}

/// The command a compositor bind runs: `"<client_exe>" --hotkey <action>`.
/// The socket path is implied by the client (same runtime directory).
pub fn socket_command(action: &str, client_exe: &Path, _sock: &Path) -> String {
    format!("\"{}\" --hotkey {action}", client_exe.display())
}

/// One `bind = MODS, KEY, exec, <command>` line.
pub fn bind_line(action: &str, combo: &str, client_exe: &Path, sock: &Path) -> String {
    let (mods, key) = to_hyprland_bind(combo);
    format!(
        "bind = {mods}, {key}, exec, {}",
        socket_command(action, client_exe, sock)
    )
}

/// `_hyprland_bind_actions`: the six socket actions then the custom clips
/// by length, keeping only those with a key.
pub fn bind_actions(bindings: &HotkeyBindings) -> Vec<String> {
    let mut actions: Vec<String> = SOCKET_ACTIONS.iter().map(|s| s.to_string()).collect();
    for n in bindings.custom_clip_lengths() {
        actions.push(super::HotkeyAction::SaveCustomClip(n).id());
    }
    actions
        .into_iter()
        .filter(|a| bindings.get(a).is_some_and(|k| !k.is_empty()))
        .collect()
}

fn conf_text(bindings: &HotkeyBindings, client_exe: &Path, sock: &Path) -> String {
    let mut text = HEADER.to_owned();
    for action in bind_actions(bindings) {
        let combo = bindings.get(&action).unwrap_or("");
        text.push_str(&bind_line(&action, combo, client_exe, sock));
        text.push('\n');
    }
    text
}

/// Writes the managed file atomically; `Ok(true)` when its content changed.
pub fn write_managed_conf(
    home: &Path,
    bindings: &HotkeyBindings,
    client_exe: &Path,
    sock: &Path,
) -> std::io::Result<bool> {
    let path = managed_conf(home);
    let text = conf_text(bindings, client_exe, sock);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("conf.tmp");
    let result = std::fs::write(&tmp, &text).and_then(|()| std::fs::rename(&tmp, &path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

/// Appends `source = <managed conf>` to `hyprland.conf` once; a missing
/// file is never created and a file that already binds the socket is
/// left alone.
pub fn ensure_source_line(home: &Path) -> std::io::Result<()> {
    let conf = hypr_dir(home).join("hyprland.conf");
    let Ok(content) = std::fs::read_to_string(&conf) else {
        return Ok(());
    };
    // Only our own managed file or a bind to our own socket counts; the
    // reference's `nc -U …/fthr/hotkey.sock` binds point at nothing now.
    if content.contains("fthr-hotkeys.conf") || content.contains("fthrust/hotkey.sock") {
        return Ok(());
    }
    let line = format!(
        "\n# FTHR Clips hotkeys (auto-added)\nsource = {}\n",
        managed_conf(home).display()
    );
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(&conf)?;
    f.write_all(line.as_bytes())
}

/// Writes the managed block into a Lua keybind file; `Ok(true)` when the
/// file changed. An incomplete block (begin without end) is refused.
pub fn write_lua_block(
    path: &Path,
    bindings: &HotkeyBindings,
    client_exe: &Path,
    sock: &Path,
) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let mut existing = original.clone();
    if let Some(begin) = existing.find(LUA_BEGIN) {
        let Some(end_rel) = existing[begin..].find(LUA_END) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "incomplete FTHR Lua hotkey block; refusing to edit user config",
            ));
        };
        let end = begin + end_rel + LUA_END.len();
        existing = format!("{}{}", existing[..begin].trim_end(), &existing[end..]);
    }
    let mut lines = vec![
        existing.trim_end().to_owned(),
        String::new(),
        LUA_BEGIN.to_owned(),
    ];
    for action in bind_actions(bindings) {
        let combo = bindings.get(&action).unwrap_or("");
        let lua_combo = combo
            .split('+')
            .map(|p| p.to_uppercase())
            .collect::<Vec<_>>()
            .join(" + ");
        let command = socket_command(&action, client_exe, sock)
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        lines.push(format!(
            "hl.bind(\"{lua_combo}\", hl.dsp.exec_cmd(\"{command}\"), {{ description = \"FTHR Clips: {action}\" }})"
        ));
    }
    lines.push(LUA_END.to_owned());
    let text = format!("{}\n", lines.join("\n").trim_end());
    if text == original {
        return Ok(false);
    }
    std::fs::write(path, text)?;
    Ok(true)
}

/// What [`apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// A file changed (the caller reloads Hyprland).
    pub changed: bool,
    /// The Lua variant was used.
    pub lua: bool,
}

/// `_apply_compositor_config` (Hyprland branch): the Lua block when a Lua
/// config exists, else the managed conf plus the source line.
pub fn apply(
    home: &Path,
    bindings: &HotkeyBindings,
    client_exe: &Path,
    sock: &Path,
) -> std::io::Result<Applied> {
    let dir = hypr_dir(home);
    let custom_lua = dir.join("custom").join("keybinds.lua");
    if custom_lua.exists() || dir.join("hyprland.lua").exists() {
        let changed = write_lua_block(&custom_lua, bindings, client_exe, sock)?;
        return Ok(Applied { changed, lua: true });
    }
    let changed = write_managed_conf(home, bindings, client_exe, sock)?;
    ensure_source_line(home)?;
    Ok(Applied {
        changed,
        lua: false,
    })
}

/// `HotkeyManager.setup_instructions`: the per-desktop heading and the
/// commands to bind by hand.
pub fn setup_instructions(compositor: &str, client_exe: &Path, sock: &Path) -> String {
    let commands: Vec<String> = ["save_clip", "save_screenshot"]
        .iter()
        .map(|a| socket_command(a, client_exe, sock))
        .collect();
    let (heading, rendered): (&str, Vec<String>) = match compositor {
        "kwin" => (
            "KDE: System Settings -> Keyboard -> Shortcuts -> Add New -> Command or Script",
            commands,
        ),
        "gnome" => (
            "GNOME: Settings → Keyboard → Custom Shortcuts",
            commands.iter().map(|c| format!("bash -c '{c}'")).collect(),
        ),
        _ => ("Bind your preferred keys to these commands:", commands),
    };
    format!("{heading}\n\n{}", rendered.join("\n"))
}

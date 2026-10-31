//! Hyprland managed hotkey config (`HotkeyManager._write_hyprland_config`
//! and friends): `~/.config/hypr/fthr-hotkeys.conf` with one `bind` line
//! per bound socket action, a `source =` line appended to `hyprland.conf`
//! once, or a managed block in a Lua config. The client command is
//! `"<fthrust>" --hotkey <action>` (the reference used `echo | nc -U`).

use std::path::{Path, PathBuf};

use super::{HotkeyBindings, keys};
use crate::fsutil;

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

/// `_to_hyprland_bind`: `Ctrl+Shift+S` → `("CTRL SHIFT", "S")`. With Shift
/// held a shifted symbol is bound on the key it is typed on, which is what
/// Hyprland matches: `Ctrl+Shift+Exclamationmark` → `("CTRL SHIFT", "1")`.
pub fn to_hyprland_bind(combo: &str) -> (String, String) {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.len() == 1 {
        if parts[0].is_empty() {
            return (String::new(), String::new());
        }
        return (String::new(), hypr_key(parts[0]));
    }
    let modifiers = &parts[..parts.len() - 1];
    let mods = modifiers
        .iter()
        .map(|p| p.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ");
    let shift = modifiers
        .iter()
        .any(|p| p.trim().eq_ignore_ascii_case("shift"));
    let last = keys::bound_key(parts[parts.len() - 1], shift);
    (mods, hypr_key(last))
}

/// Hyprland parses xkb keysym names: letters upper-cased as the reference
/// did, every other key through the keysym table the portal trigger shares
/// (`Page Up`, `PageUp` and `Pageup` → `Prior`, `Equals` → `equal`). A key
/// with no keysym is passed through as it was typed.
fn hypr_key(key: &str) -> String {
    match keys::keysym(key) {
        Some(k) if k.chars().count() == 1 => k.to_uppercase(),
        Some(k) => k.into_owned(),
        None => key.to_owned(),
    }
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
/// by length, keeping only those with a key. A key with no xkb keysym (`Fn`,
/// or a name nothing knows) is left out and logged: Hyprland would reject the
/// line, and in a Lua config a bad bind can take the rest of the file with it.
pub fn bind_actions(bindings: &HotkeyBindings) -> Vec<String> {
    let mut actions: Vec<String> = SOCKET_ACTIONS.iter().map(|s| s.to_string()).collect();
    for n in bindings.custom_clip_lengths() {
        actions.push(super::HotkeyAction::SaveCustomClip(n).id());
    }
    actions
        .into_iter()
        .filter(|a| {
            let Some(combo) = bindings.get(a).filter(|k| !k.is_empty()) else {
                return false;
            };
            let bindable = keys::is_bindable(combo);
            if !bindable {
                tracing::warn!(action = %a, combo, "the key has no xkb keysym; not written to the Hyprland config");
            }
            bindable
        })
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

/// Reads a user's Lua keybind file for editing: `None` when it does not
/// exist. Any other failure to read it, and content that is not UTF-8,
/// aborts: the file is the user's, and treating it as empty would rewrite it
/// with nothing but the managed block.
fn read_lua_config(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} is not valid UTF-8; refusing to edit user config",
                    path.display()
                ),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Suffix of the copy kept of the Lua keybind file before a change. It is not
/// `.bak`: `keybinds.lua.bak` is the name a user (or their editor) gives a
/// copy of their own, and it must not be overwritten.
pub const LUA_BACKUP_SUFFIX: &str = ".fthr-bak";

/// Where the previous contents of the Lua keybind file at `path` are kept:
/// `<name>.fthr-bak` beside it.
pub fn lua_backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(LUA_BACKUP_SUFFIX);
    path.with_file_name(name)
}

/// Errors that say a file cannot be replaced by a rename although it can
/// still be written: a mount point (a bind-mounted file), a directory that is
/// not ours around a file that is, a read-only directory over a writable file.
fn rename_refused(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{PermissionDenied, ReadOnlyFilesystem, ResourceBusy};
    matches!(
        e.kind(),
        PermissionDenied | ReadOnlyFilesystem | ResourceBusy
    )
}

/// Replaces the Lua keybind file with `text` through a temporary file and a
/// rename, after saving `previous` beside it ([`lua_backup_path`]). A symlink
/// (dotfile managers) is written through, not replaced, and the file keeps
/// its mode. Nothing is replaced unless the backup was written. When the
/// rename is refused (see [`rename_refused`]) the file is rewritten in place
/// instead: the backup is already safe, and if the rewrite itself fails
/// half-way the previous contents are put back.
fn replace_lua_config(path: &Path, previous: Option<&str>, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mode = std::fs::metadata(&target)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    if let Some(previous) = previous.filter(|p| !p.is_empty()) {
        fsutil::write_atomic(&lua_backup_path(path), previous.as_bytes(), mode)?;
    }
    match fsutil::write_atomic(&target, text.as_bytes(), mode) {
        Err(e) if rename_refused(&e) && target.is_file() => {
            tracing::info!(path = %target.display(), error = %e, "writing the keybind file in place");
            std::fs::write(&target, text.as_bytes()).inspect_err(|_| {
                if let Some(previous) = previous {
                    let _ = std::fs::write(&target, previous.as_bytes());
                }
            })
        }
        other => other,
    }
}

/// Writes the managed block into a Lua keybind file; `Ok(true)` when the
/// file changed. An incomplete block (begin without end) is refused, and so
/// is a file that cannot be read or is not UTF-8 (only a missing one is
/// created). The file is replaced atomically (in place where a rename is
/// refused) and its previous contents stay in `<name>.fthr-bak`.
pub fn write_lua_block(
    path: &Path,
    bindings: &HotkeyBindings,
    client_exe: &Path,
    sock: &Path,
) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let previous = read_lua_config(path)?;
    let original = previous.clone().unwrap_or_default();
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
        // Modifiers upper-cased, the key as its keysym name.
        let (mods, key) = to_hyprland_bind(combo);
        let lua_combo = mods
            .split(' ')
            .filter(|m| !m.is_empty())
            .chain(std::iter::once(key.as_str()))
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
    replace_lua_config(path, previous.as_deref(), &text)?;
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

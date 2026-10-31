# Troubleshooting

When something goes wrong, FTHRust says so in the bar at the bottom of the window, usually with a button that takes you to the fix. This page covers the common problems, what each message means, and how to collect what's needed for a bug report.

- [Common problems](#common-problems)
- [Error codes](#error-codes)
- [Other messages](#other-messages)
- [Logs](#logs)
- [Diagnostic reports](#diagnostic-reports)

## Common problems

### The status never reaches "Capturing"

- **Is this a Wayland session?** Run `echo $WAYLAND_DISPLAY`. If it's empty, you're on X11, which FTHRust doesn't support.
- **Did you cancel the screen-sharing dialog, or stop sharing?** On desktops that use the ScreenCast portal, declining the dialog, stopping the share from the desktop, or a recovery that could only continue by asking again all stop capture. Press **RESTART ENGINE** in the error bar (or **Alt+Shift+R**), and choose a screen if the dialog comes up.
- **Is the portal installed?** You need `xdg-desktop-portal` plus a backend for your desktop: `xdg-desktop-portal-hyprland`, `-wlr`, `-kde` or `-gnome`.
- **Try another capture method.** Start the app with `FTHR_CAPTURE_BACKEND=portal` (or `wlr`, or `ext`) to see if a different one works.

### "No hardware encoder" or ENGINE COULD NOT START

Automatic encoder mode only uses GPU encoders, and won't quietly switch to your CPU.

1. Check your GPU's encoder driver: `vainfo` for AMD and Intel (from `libva-utils`), or `nvidia-smi` for NVIDIA.
2. Install the missing piece: `mesa-va-drivers` (Ubuntu) or `libva-mesa-driver` (Arch) for AMD; `intel-media-driver` for recent Intel.
3. Press **Redetect** in **Settings → Clip → Video Encoding**.
4. Still nothing? Choose **Software** as the encoder. It works on any CPU, at a performance cost.

### No sound in clips, or DESKTOP AUDIO UNAVAILABLE

- Check PipeWire is running: `systemctl --user status pipewire wireplumber`.
- Make sure *Enable audio capture* is on in **Settings → Audio**.
- If you picked a specific **Playback device to capture**, try the default instead. If the device you chose is missing, FTHRust falls back to the default output.
- The microphone isn't recorded until you choose an **Input device**.

### Hotkeys do nothing

- **Hyprland:** check that `~/.config/hypr/hyprland.conf` contains `source = ~/.config/hypr/fthr-hotkeys.conf`, and that `fthr-hotkeys.conf` lists your keys. Run `hyprctl reload`.
- **Moved the AppImage?** Binds point at the AppImage's path. Change any hotkey (and change it back) to rewrite them.
- **Another app owns the key.** Your desktop wins. Look for **DESKTOP KEPT ITS SHORTCUT** in the error bar, or pick another key.
- **Test without the keyboard:** run `<path to the AppImage> --hotkey save_clip` in a terminal. If that saves a clip, the problem is the key binding, not FTHRust.

### "FTHR Clips is already running"

Only one copy runs at a time; starting another brings the existing window forward. A copy that is still quitting (finishing a save, a recording or an export) is waited for, up to two minutes, and then the new one starts. If no window appears after that, the first copy may be stuck: `pkill -x fthrust` asks it to quit as if you had closed it, and a second `pkill -x fthrust` ends it at once. A capture engine it leaves behind is stopped by the next start.

### Clips look blocky or smeared

Your bitrate is too low for the game. Raise **Quality** in the capture popup to **High**, or use **Custom** with 50,000 kbps or more for fast games at 1440p.

### Saves are refused with "buffer is rebuilding"

Capture restarted recently (a settings change, a recording at different quality, or recovery from an error) and the buffer is still refilling. Wait as long as your clip length and try again.

## Error codes

Errors in the bottom bar are shown as `[Error NNN] TITLE`. Quote the number when you report a bug.

| Code | Title | Usually means |
|---|---|---|
| 001 | CAPTURE FAILED | The compositor or portal stopped sending frames. |
| 002 | CAPTURE UNAVAILABLE | No capture method worked. See [above](#the-status-never-reaches-capturing). |
| 003 | ENGINE NOT FOUND | `fthrust-engine` is missing from next to the app. Re-download the AppImage. |
| 004 | ENGINE NOT RESPONDING | The engine stopped answering. FTHRust offers **RESTART ENGINE**. |
| 005 | ENGINE COULD NOT START | Often no usable encoder. See [above](#no-hardware-encoder-or-engine-could-not-start). |
| 006 | ENGINE STOPPED | The engine exited unexpectedly or stopped answering. FTHRust restarts it on its own up to 3 times in 10 minutes; after that, clips can't be saved until you press **RESTART ENGINE**. |
| 007 | MANUAL RECORDING FAILED | A recording stopped with an error, for example when writing fell more than 8 seconds behind capture. |
| 008 | RECORDING COULD NOT START | Often too little disk space (512 MB is needed). |
| 009 | RECORDING COULD NOT STOP | The recording didn't finish cleanly. |
| 010 | RECORDING FOLDER UNAVAILABLE | The recording folder is missing or not writable. |
| 011 | RECORDING PATH TOO LONG | The folder path plus file name is over 255 characters. |
| 012 | NOT ENOUGH DISK SPACE | Free some space, or move your clips folder. |
| 013 | CLIP NOT SAVED | The save was refused (paused, rebuilding, or empty buffer). |
| 014 | CLIP SAVE FAILED | Writing the clip failed. |
| 015 | CLIP WAS NOT SAVED | The save didn't complete. |
| 016 | CLIP PATH TOO LONG | Shorten the clips folder path or the game name. |
| 017 | CLIP FINALIZATION FAILED | The clip was written but the last step of finishing it failed. |
| 018 | EXPORT FAILED | An export or share couldn't start or finish. |
| 019 | SHARE FAILED | A share couldn't be made. |
| 020 | UPLOAD FAILED | See the reason in the bar. **RETRY NOW** tries again. |
| 021 | UPLOAD NOT CONFIGURED | Uploading isn't set up yet. |
| 022 | UPLOAD CONNECTION FAILED | **Test connection** failed. **OPEN UPLOAD SETTINGS** takes you there. |
| 023 | COMPRESSION FAILED | A clip too big for Catbox couldn't be compressed. |

## Other messages

| Message | Means |
|---|---|
| **DESKTOP AUDIO UNAVAILABLE** | PipeWire couldn't be reached. Clips are saved without desktop audio. |
| **MIC NOT FOUND** | The chosen microphone is gone. Pick another in **OPEN AUDIO SETTINGS**. |
| **CLIP SAVED WITHOUT AUDIO** | The clip is fine, but an audio source wasn't capturing at the time. |
| **CAPTURE DEGRADED**, **CAPTURE STALLED**, **RECOVERING CAPTURE** | New frames stopped arriving, or capture failed and the engine is restarting it. |
| **CAPTURE DEGRADED — DARK / UNIFORM** | The picture has been black or one flat colour for 12 seconds. Expected in a dark scene or on a loading screen; the engine restarts capture once in case the feed is broken (not on the portal). |
| **SETTINGS APPLY TIMED OUT** | Capture didn't come back within 30 seconds of a settings change. Press **RESTART ENGINE**. |
| **GAME CROP ADJUSTED** | The engine had to change a game's crop to make it fit the captured frame. **EDIT GAME CROP** lets you set it yourself. |
| **DESKTOP KEPT ITS SHORTCUT** | Your desktop bound a different key than you asked for. |
| **STOP RECORDING FIRST**, **WAIT FOR CLIP SAVE** | That action has to wait for the recording or save in progress. |
| **CAMERA UNAVAILABLE**, **CAMERA OVERLAY FAILED** | The webcam overlay couldn't be used. Clips save without it. |
| **LIMITED FEATURE SUPPORT** | Your desktop doesn't share its window list, so game detection and focus pause are off. Capture and hotkeys are unaffected. See [Desktop support](install.md#desktop-support). |
| **STARTUP PROBLEM** | FTHRust couldn't read or write one of its own files while starting (`settings.json`, `theme.json`, the theme folders), for example on a full or read-only disk, and opened anyway with defaults. The message names the file, and shows even if you turned error notifications off. A file that couldn't be read is left as it is, so that session's changes aren't saved; a damaged one is kept as `<name>.corrupt`. The same notice appears if FTHRust was killed in the middle of a theme import and couldn't undo it: it names the `.import-…` folder under `~/.fthr/theme/` that keeps your previous icons, sounds and fonts, and tries again at the next start. |

## Logs

- **The app** logs to `~/.fthr/logs/fthr.log`, rotated at 2 MB with two older copies kept, and to the terminal if you started it from one. It records FTHRust's own messages at info level and warnings from everything else; add `RUST_LOG=debug` for more detail:

  ```sh
  RUST_LOG=debug ./FTHRust-*-x86_64.AppImage
  ```

  Passwords, tokens, cookies and similar are replaced with `<redacted>` before a line is written.

- **Crashes**: a panic is written, with its backtrace, to `~/.fthr/logs/fthr-crash.log`.

- **The engine's** log is written to `~/.fthr/logs/engine.log`, rotated at 2 MB. `FTHRUST_LOG=debug` makes it more detailed.
- **FFmpeg's** messages go into both logs under `fthrust_media::ffmpeg`, warnings and errors only. `FTHRUST_FFMPEG_LOG=info` (or `verbose`, `debug`, `trace`) lets more through. Each log's own filter must allow that level too: `RUST_LOG` for the app (it already passes `info`), `FTHRUST_LOG` for the engine, for example `RUST_LOG=debug FTHRUST_LOG=debug`.

If the app was closed in any way other than quitting it (a crash, a power cut, a `kill -9`), the next start notes `previous_session_unclean` in its diagnostics. Quitting normally clears that, and so does logging out or the compositor exiting: the app shuts down in order then too.

## Diagnostic reports

**Settings → General → Troubleshooting → Export diagnostic report** builds a ZIP in `~/.fthr/diagnostics/` and shows it in a dialog that you can drag straight into a bug report.

The report holds a summary of your system, FTHRust's recent activity (`events.jsonl`, one JSON record per line) and the last megabyte of each log (`ui.log` from `fthr.log`, `engine.log` from the engine's log), with paths and secrets (tokens, passwords, cookies, auth headers) redacted. It **never** includes clips, screenshots, audio, credentials or a listing of your files, and it's never uploaded anywhere by FTHRust. Open the ZIP and read it before sending if you like; it's all plain text.

If a problem started earlier than the last megabyte of a log reaches, attach the whole `~/.fthr/logs/fthr.log` or `engine.log` as well (and the terminal output if you have it).

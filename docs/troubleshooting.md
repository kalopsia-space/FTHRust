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
- **Did you cancel the screen-sharing dialog?** On desktops that use the ScreenCast portal, declining it stops capture. Press **RESTART ENGINE** in the error bar (or **Alt+Shift+R**) and choose a screen this time.
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

Only one copy runs at a time; starting another brings the existing window forward. If no window appears, the first copy may be stuck. Run `pkill fthrust` and start again.

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
| 006 | ENGINE STOPPED | The engine exited unexpectedly. Clips can't be saved until it's restarted. |
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
| **CAPTURE DEGRADED**, **CAPTURE STALLED**, **RECOVERING CAPTURE** | The engine noticed frozen or missing frames and is fixing it. |
| **SETTINGS APPLY TIMED OUT** | Capture didn't come back within 30 seconds of a settings change. Press **RESTART ENGINE**. |
| **GAME CROP ADJUSTED** | The engine had to change a game's crop to make it fit the captured frame. **EDIT GAME CROP** lets you set it yourself. |
| **DESKTOP KEPT ITS SHORTCUT** | Your desktop bound a different key than you asked for. |
| **STOP RECORDING FIRST**, **WAIT FOR CLIP SAVE** | That action has to wait for the recording or save in progress. |
| **CAMERA UNAVAILABLE**, **CAMERA OVERLAY FAILED** | The webcam overlay couldn't be used. Clips save without it. |
| **LIMITED FEATURE SUPPORT** | Your desktop doesn't share its window list, so game detection and focus pause are off. Capture and hotkeys are unaffected. See [Desktop support](install.md#desktop-support). |

## Logs

- **The app** logs to the terminal. Start it from a terminal to see what it's doing, and add `RUST_LOG=debug` for more detail:

  ```sh
  RUST_LOG=debug ./FTHRust-*-x86_64.AppImage
  ```

- **The engine's** log is written to `~/.fthr/logs/engine.log`, rotated at 2 MB. `FTHRUST_LOG=debug` makes it more detailed.

## Diagnostic reports

**Settings → General → Troubleshooting → Export diagnostic report** builds a ZIP in `~/.fthr/diagnostics/` and shows it in a dialog that you can drag straight into a bug report.

The report holds a summary of your system and FTHRust's recent activity, with paths and secrets (tokens, passwords, auth headers) redacted. It **never** includes clips, screenshots, audio, credentials or a listing of your files, and it's never uploaded anywhere by FTHRust. Open the ZIP and read it before sending if you like; it's all plain text.

While FTHRust is in alpha, please also attach `~/.fthr/logs/engine.log` (and the terminal output if you have it). The report doesn't include full logs yet.

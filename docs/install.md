# Installing

FTHRust Clips ships as a single AppImage: one file that holds the app, its capture engine, its uploader and a pinned copy of FFmpeg. There's nothing to install system-wide and nothing to uninstall but the file and its settings folder.

- [What you need](#what-you-need)
- [Running the AppImage](#running-the-appimage)
- [Your first minute](#your-first-minute)
- [Desktop support](#desktop-support)
- [Coming from FTHR Clips](#coming-from-fthr-clips)
- [Building from source](#building-from-source)
- [Uninstalling](#uninstalling)

## What you need

| | |
|---|---|
| **CPU and OS** | x86_64 Linux with glibc 2.35 or newer. Ubuntu 22.04, Fedora 36, Arch and anything newer are fine. |
| **Session** | A **Wayland** session. There is no X11 support, and the app says so at startup if `WAYLAND_DISPLAY` isn't set. |
| **Audio** | PipeWire, with a session manager such as WirePlumber. FTHRust talks to PipeWire directly, not through PulseAudio. |
| **Portals** | `xdg-desktop-portal` with a backend for your desktop. It's used for screenshots, file pickers, global shortcuts, and screen capture on desktops that need a permission dialog. |
| **GPU** | Something that can run Vulkan or OpenGL for the interface. For encoding: an NVIDIA driver (NVENC), or a VA-API driver for AMD and Intel (`mesa-va-drivers` on Ubuntu, `libva-mesa-driver` or `intel-media-driver` on Arch). |
| **FUSE** | To mount the AppImage. Most desktops already have it; see [below](#running-the-appimage) if yours doesn't. |
| **Optional** | A Secret Service keyring (GNOME Keyring, KWallet) if you want to save SFTP passwords. |

> **No hardware encoder?** Capture still works, but you have to choose it: set **Settings → Clip → Video Encoding → Encoder** to **Software**. Automatic mode only ever picks a hardware encoder and tells you if it can't find one, so the CPU never takes over quietly mid-game.

## Running the AppImage

Download `FTHRust-<version>-x86_64.AppImage` from the [releases page](https://github.com/kalopsia-space/FTHRust/releases). Each release also has a `.sha256` file if you want to check the download:

```sh
sha256sum -c FTHRust-*-x86_64.AppImage.sha256
chmod +x FTHRust-*-x86_64.AppImage
./FTHRust-*-x86_64.AppImage
```

Put the file somewhere permanent, like `~/Applications`, **before** you set up hotkeys. Hotkey bindings point at the AppImage's own path, so moving it later means rebinding.

Running `./FTHRust-…AppImage --version` prints the version and exits.

**If it says FUSE is missing**, install your distribution's FUSE package (`fuse3` on most, `libfuse2` on some older ones), or run it without mounting:

```sh
./FTHRust-*-x86_64.AppImage --appimage-extract-and-run
```

Only one copy of the app runs at a time. Launching it again just brings the existing window to the front.

## Your first minute

When the window opens, the app starts its capture engine and the toolbar status in the top right moves to **Capturing**. From that moment the replay buffer is filling.

<img src="images/library.png" alt="The main window with the capture toolbar along the top" width="860">

Along the top of the window:

- **Record** starts and stops a full recording.
- **`30s // 60 fps // Source // 25,000 kbps`** is your replay setup. Click it to change clip length and quality.
- **Source** chooses what's captured.
- **Game detection** switches into game mode when a game appears.
- **Hotkeys** shows and changes your keys.

Press **F9** to save the last 30 seconds. The clip appears at the top of the library, and the file is written to `~/Videos/FTHR Clips/Desktop/`.

On some desktops the first capture asks you to pick a screen in a system dialog. Pick your game monitor; the choice is remembered.

## Desktop support

FTHRust Clips is built and tested on **Hyprland**. It uses standard Wayland protocols and portals rather than anything Hyprland-only, so other desktops work as far as they provide those pieces. Here's what each feature relies on:

| Feature | Needs | If it's missing |
|---|---|---|
| Screen capture | `ext-image-copy-capture`, `wlr-screencopy`, **or** the ScreenCast portal | The engine tries them in that order. The portal shows a one-time "share your screen" dialog. |
| Automatic hotkeys | Hyprland (managed config) **or** the GlobalShortcuts portal | Bind `fthrust --hotkey save_clip` yourself in your desktop's shortcut settings. |
| Game detection and focus pause | `wlr-foreign-toplevel-management` | The Game detection button reads "Game detection unavailable". Capture and hotkeys are unaffected. |
| Capture card and Gary overlays | `wlr-layer-shell` | Capture cards become ordinary desktop notifications. Gary isn't shown. |

In practice:

- **Hyprland:** everything works.
- **Sway, river and other wlroots desktops:** capture, game detection and overlays work. You bind hotkeys yourself unless your portal provides GlobalShortcuts.
- **KDE Plasma:** capture works, and global shortcuts come through the portal. Game detection is unavailable.
- **GNOME:** capture works through the portal dialog, and shortcuts come through the portal on GNOME 48 and newer. Game detection and on-screen overlays are unavailable, and capture cards arrive as notifications.

Only Hyprland is tested. If you run something else, reports of what does and doesn't work are very welcome.

## Coming from FTHR Clips

FTHRust Clips uses the same `~/.fthr` folder as the original FTHR Clips Linux app, and reads its files in place. There's no import step:

- Settings, presets, export presets, hotkeys, themes and clip metadata (tags, descriptions, editor drafts) carry over.
- Your clips stay where they are. If your old settings never set a clips folder, the old default `~/FTHR_Clips` is used rather than the new `~/Videos/FTHR Clips`.
- A few old settings that no longer exist are dropped the first time FTHRust saves: the old keyboard and mouse overlays, multiband audio, auto-crop and extended clip length.

Close the original app before starting FTHRust. Running both at once isn't supported; they would fight over the same files.

## Building from source

If you'd rather build it yourself, [Development](development.md) covers the toolchain and system packages. The short version:

```sh
cargo build --release -p fthrust-ui -p fthrust-engine -p fthrust-uploader
./target/release/fthrust
```

The three binaries must sit in the same folder, because the app looks for its engine and uploader next to itself.

## Uninstalling

1. Delete the AppImage.
2. Delete `~/.fthr` for settings, logs and caches. Leave it if you might come back.
3. If you used Hyprland's managed hotkeys, delete `~/.config/hypr/fthr-hotkeys.conf` and the `source = …fthr-hotkeys.conf` line the app added to `hyprland.conf`. On Lua configs, remove the block between the `FTHR Clips hotkeys begin/end (managed)` markers in `custom/keybinds.lua`.
4. Your clips in `~/Videos/FTHR Clips` are yours. Keep or delete them as you like.

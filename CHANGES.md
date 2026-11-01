# Changes

## 2.0.0-alpha.3 (unreleased)

Fixes from a whole-project audit (`docs/superpowers/audit-2026-10-31.md` in the repository). "alpha.2" is the previous release; "the reference" is FTHR Clips linux-2.0.

### Capture

- Portal capture opens again after a recovery or a settings change; alpha.2 talked to a dead D-Bus connection on every later open (BACKEND_FAILED after a 10 s wait) and deleted the restore token, so the next start showed the screen picker.
- The screen picker only comes up for a start you asked for (starting the app, Apply, RESTART ENGINE, Record). A portal failure during capture (a resolution change, a stream error) and the restarts FTHRust makes on its own (after the engine crashed or hung, when game detection switches to or from a game, when the clips profile comes back after a recording) only restore your last choice; when that isn't possible, capture stops with SCREEN SHARING NOT RESTORED and RESTART ENGINE lets you choose a screen. The desktop's "Stop sharing" ends capture, like a dismissed picker, and forgets the saved choice, so only a start you ask for shares the screen again.
- A failed, declined or cancelled portal start closes its ScreenCast session instead of leaving it (and its screen-sharing indicator) open, and the restore token the portal issues is saved as soon as Start returns.
- A missing or unanswering portal no longer costs the restore token; only a restored session whose stream does not negotiate is retried without it (showing the picker), as in the reference, and only on a start you asked for.
- When the compositor is slow to answer at startup, the engine waits for ext-image-copy-capture or wlr-screencopy instead of falling through to the portal's screen picker.
- Zero-copy capture starts a fresh capture session, so it works from the first frame on a static screen. A compositor that never fills the dmabuf buffers falls back to shared memory after 5 seconds, and one that fails the first capture into them at once, without restarting capture; the capture surfaces are freed when capture moves to a backend that can't use them.
- A static screen right after a resolution change keeps capturing instead of timing out three times and dropping ext-image-copy-capture for wlr-screencopy, and a resolution change no longer stalls capture for 2 s waiting for buffer constraints that had already arrived.
- A picture that stays black or one flat colour for 12 seconds restarts ext-image-copy or wlr-screencopy capture once, as the docs said it would (a zero-copy capture restarts on shared memory and goes back to zero copy, which is only given up when its own picture is black where shared memory showed one); through the portal it is only flagged, because a restart would open a new session and could put the picker in front of the game.
- The content streak in the engine status counts black or flat samples in a row, as in the reference, instead of identical samples (which grew on any static desktop).
- Outputs with a flipped-90 or flipped-270 transform are captured upright; they came out rotated 180°.
- A game running at about the capture frame rate no longer shows a duplicated and then a skipped frame where a repeat raced the next real frame.
- The engine asks the compositor for a frame only when the next one is due, so a 60 fps capture of a 144 or 240 Hz monitor no longer has 58–75% of its copies thrown away.
- In zero-copy mode the once-a-second content check reads 144 pixels from the capture surface instead of downloading the whole frame (about 100 ms at 4K) on the capture thread.
- Portal frames are copied once instead of twice, outside the lock the capture thread waits on, and BGRx frames by rows instead of per pixel.
- Wayland waits no longer spin for their last millisecond (a sub-millisecond poll timeout was truncated to zero).

### Saves and recordings

- Recordings keep their audio in sync: alpha.2 dropped about 0.45% of the samples (2.7 s per 10 minutes), started the audio before the first frame, kept the AAC priming and skipped PipeWire reconnects; the audio clock now counts samples from the first keyframe and fills gaps with silence.
- Full recordings include the microphone as a second track, like clips (the reference recorded only the desktop mix).
- Stopping a recording keeps the frames still in the encoder and the audio up to the last frame, instead of cutting the last 200–300 ms of audio and any queued frames.
- A recording that fails to start no longer leaves an empty `FTHR_recording_*.mp4`, finished recordings are flushed to disk, and their decode timestamps are written as the encoder gave them.
- Starting a recording no longer blocks the engine's command loop, and the keyframe it requests can no longer slip past the recorder; a frame that fails before reaching the encoder no longer swallows a pending keyframe request either.
- Clip saves end on the newest frame the encoder delivered and wait for the audio of that moment, so a slow encoder no longer puts the audio ahead of the video and clips no longer end on 20–50 ms of silence.
- The disk replay buffer stays on disk after an encoder restart or a resolution change (it quietly moved to RAM), and its writes no longer run on the capture thread.
- A long save no longer stalls capture or the PipeWire audio callback: audio windows are copied outside the source lock, and the engine status no longer holds its lock while asking the audio sources.
- Encoding a clip's audio no longer copies the whole track first, halving peak audio memory on long saves.
- A crash inside a save or a settings change is reported instead of wedging the engine: the save worker keeps serving, and a failed Configure ends the engine with `CONFIGURE_FAILED`.
- A finished clip that can't be renamed into place is saved as `<name>_unpublished.mp4`, or kept as its `.partial`, and CLIP WAS NOT SAVED says which, instead of being deleted. Folders without no-replace renames or hard links (some FUSE mounts, such as exfat-fuse) publish with a checked plain rename instead of losing every clip.
- Leftovers of a crashed save, recording, export or screenshot (hidden `.partial` files and `.fthr-finalize-*` folders) older than a day are removed at startup; a finished clip found as `.partial` is published under its own name instead, or as `<name> (recovered).mp4` if that is taken. alpha.2 never swept them, and the reference's sweep only knew clip names.
- A manual recording shows on the Clips page with a Recording badge while it runs, and can't be opened, renamed, deleted or uploaded until it ends; alpha.2 listed it as a finished clip (renaming it failed the recording, and an upload that deletes the file could take it).

### Quitting, and the engine behind the app

- Quitting while a clip saves lets the save finish (the engine was killed about 200 ms after the request and the clip lost), and so does a recording that is still writing its end (alpha.2 stopped the engine after 2 s). The window closes first, so the app never sits frozen on screen while it waits.
- SIGTERM, SIGINT and SIGHUP (a logout, `systemctl stop`, Ctrl+C) quit the app the way closing it does: pending settings are written, a save in flight finishes and the session closes cleanly. They used to kill it on the spot; a second signal still quits at once.
- Starting the app while it is still quitting waits for it, up to two minutes, and then opens the window; it used to say "already running" and show nothing.
- The engine no longer hangs on exit when the app dies during a save or stops reading, and never drops the result of a save in flight for an app that is still reading. It stops capturing (as soon as a recording has its last frames) and frees its socket before finishing that save, so a relaunched app can start a new engine at once, and a SIGTERM during those waits ends them immediately.
- A crashed or hung engine is restarted on its own (after 2 s, up to 3 times in 10 minutes); the reference left capture stopped until you pressed RESTART ENGINE. Saves are refused as soon as the engine goes quiet instead of being sent to it.
- Two engines never run at once: a start asks an engine left over from an earlier session to stop instead of unlinking its socket, and an engine only removes its own socket file when it exits.
- Capture settings the engine can't start with show SETTINGS APPLY FAILED instead of leaving STARTING CAPTURE up for good.
- Settings applied while another restart is pending or waiting for a save are no longer dropped: the newest settings win.
- The engine log keeps going past output that isn't valid UTF-8 (the reader used to stop, and the engine then crashed on its next log line).

### Encoders and export

- Clip settings lists the encoders that actually work: the startup test (cached in `encoder_capabilities.json`) never ran in alpha.2, so the page stayed on "Detecting available encoders…" with the pickers disabled. Redetect runs it again.
- An encoder counts as working only if its one-frame test encode returns a packet; alpha.2 listed an encoder that took the frame and produced nothing.
- Capture uses the speed/quality preset only with NVENC chosen, the one backend whose settings show it. Every other choice runs its encoder at the default preset, so a P7 picked for NVENC no longer makes Software run Kvazaar or SVT-AV1 at their slowest (the reference applied the hidden value everywhere).
- Quick Sync's best-quality preset asks for `veryslow` and Kvazaar gets its preset through `kvazaar-params`. The reference asked for a `slowest` that FFmpeg doesn't have and set a `preset` option Kvazaar doesn't have, so both ran at their defaults.
- The software encoders OpenH264, x265, libaom and rav1e use the threads FFmpeg picks instead of one.
- Software AV1 capture (SVT-AV1) runs in its low-delay mode, at a constant bitrate (the mode needs it), so a clip ends when it is saved; it held back about 2.5 seconds of frames, so clips ended that much early. Exports still use a variable bitrate.
- Software HEVC capture with Kvazaar holds no frames back and puts a keyframe every GOP (two seconds): it held about ten frames, so clips ended a third of a second early at 30 fps, and its keyframes came every 64 frames whatever the GOP. A recording with Kvazaar starts at its next keyframe, since it can't be asked for one.
- CPU colour conversion (NVENC, Quick Sync, AMF, software encoders and the VA-API fallback) runs on up to four threads, so a 1080p frame takes about 1.2 ms instead of 2.8 ms and a 4K frame about 4 to 5 ms instead of 11.
- VA-API encoders create their upload surface pools only when frames are uploaded into them, and drop them once zero-copy capture has its first frame: 15 MiB less video memory at 4K, 24 MiB less during zero-copy capture. The zero-copy pool holds only the surfaces it exports, another 34 MiB at 4K on AMD.
- The AAC encoder converts into every channel plane of its frame instead of only the first eight, so audio with more than eight channels can no longer overwrite memory, and more than 16 channels is refused with a clear message.
- Frame geometry checks can't overflow: an absurd stride is refused instead of panicking or letting a short buffer through.
- A stream-copy trim stops reading once every stream is past its end and counts dropped packets as activity. Trimming the start of a long recording no longer reads the whole file, or times out on slow storage.
- An export or overlay burn fails instead of saving a cut-off file or replacing the clip with one when its video comes out more than half a second and a frame shorter than it should, its audio stops short of the clip's own, or its filter graph fails partway.
- Size presets cap the encoder's peak bitrate at 115% of the target over a two-second buffer, as the reference's `-maxrate`/`-bufsize` did; alpha.2 computed the cap and never applied it.
- FFmpeg's messages go into the app and engine logs, warnings and errors only unless `FTHRUST_FFMPEG_LOG` asks for more. Before, FFmpeg printed everything from info up to the terminal itself, including a harmless track-title warning on every clip open.

### Clips page and editor

- The Clips grid draws every row at any scroll position; alpha.2 stopped drawing cards and day headers below the first screenful and went blank after about 2,000 px of scrolling.
- Renaming a clip moves its tag, description and editor draft with it, as the reference did; alpha.2 left them behind.
- The Clips page makes one group per full date, so the same day of two years is two groups (the label stays `Fri, Jan 15`); alpha.2 merged them by their year-less label.
- The Clips page does no per-frame work that grows with the library, decodes thumbnails on its worker threads, drops queued thumbnail jobs for cards that scrolled away and retries a failed thumbnail when the clip changes or after 30 s; alpha.2 cloned the whole library every frame, decoded JPEGs on the UI thread, kept stale jobs queued and never retried a failure.
- The library watcher ignores the writes to a recording in progress and the files it does not list, and a scan walks the tree once; alpha.2 rescanned the whole library, twice over, for every write to a recording.
- A clip with a queued or running export or share cannot be renamed or deleted from the library, and renaming or deleting a clip updates the editors parked for it; alpha.2 left them pointing at the old path.
- Show in file manager opens the folder with the clip selected (the reference called Windows' `explorer.exe`), and Open screenshot works; in alpha.2 both did nothing.
- Export, Share and Ctrl+E wait until the clip has loaded ("The clip is still loading") instead of exporting a 1 ms clip.
- Opening a clip tells you when the clip details database cannot be opened ("Could not open clip") instead of doing nothing; that database is retried every five seconds and logged once, instead of reopened every frame.
- Escape in the Share picker closes only the picker; alpha.2 closed the editor with it.
- A press between two close trim handles grabs the nearer one; the reference and alpha.2 always took the start handle.
- Closing or parking an editor frees its video frame, and the live preview's last frame, from GPU memory (alpha.2 kept 8 MB per 1080p session), and no longer waits for the video decoder to finish its seek, which froze the window for 0.4 to 0.8 s on long-GOP clips.
- Pausing does not decode the clip's GOP again when the picture on screen is already the paused frame, and an open editor's video worker sleeps while idle instead of waking 250 times a second.
- Opening a clip in the editor checks its audio manifest once instead of hashing the whole file twice.
- A thumbnail whose cached poster is damaged keeps the clip's resolution, frame rate and length and builds a new poster; finished thumbnails are taken whichever page is showing.

### Uploads

- The uploader's Terms and Privacy Policy are a link in the Upload settings instead of a gate.
- An upload is cut off only when it stops moving: an HTTP upload after 120 s without data (or without an answer after the last byte), and the uploader as a whole after 10 minutes without progress. alpha.2 failed every Catbox or Your server upload that took longer than 120 s to send, then sent it in full three more times, and stopped any upload after 10 minutes, so a large clip on a slow connection could never upload.
- Auto-delete is done by the app, by the same rules as the bin button: a clip that an export or share is still using, or that an editor has open, is kept and the bar says so. It needs a link the card could offer (an http(s) address or an SFTP location, on one line), and only removes clips inside the clips folder, so interval uploads of imported folders no longer delete the imported library; the reference deleted the clip after any non-empty `url`, even `queued`.
- When the upload history cannot be written, the clip is kept even with auto-delete on, the bar shows the link and the card keeps it for the session; alpha.2 deleted the clip and lost the link.
- An upload that worked but has something to say (no link came back, the clip could not be deleted, the history could not be saved) shows it in the bar as UPLOAD WARNING, even with "Show capture error messages" off; the reference and alpha.2 dropped the message and left a share button that did nothing. Pressing Upload on a clip uploaded without a link says "Already uploaded" and offers UPLOAD AGAIN (the editor's button reads "Upload again").
- RETRY NOW and UPLOAD AGAIN go away when their clip is renamed or deleted, instead of staying and doing nothing.
- SFTP asks for the host-key type your known_hosts holds for the server, as `ssh` does, so ECDSA and RSA entries are trusted (alpha.2 always asked for Ed25519 first and read them as an untrusted host).
- A known_hosts file with bytes that are not UTF-8 no longer breaks SFTP trust, and FTHRust's own known_hosts keeps lines it cannot decode when it records a host.
- SFTP removes what a failed upload left on the server (including a file renamed into place that then failed its checks), and an upload killed mid-transfer is cleaned up by the next upload of the same clip.
- SFTP keeps 64 write requests in flight instead of 16: over a 60 ms round trip a clip uploads at 13.6 MB/s instead of 6.4.
- A server URL or Auth header that cannot be sent is reported at once as an upload settings problem; the reference and alpha.2 retried it as a network error. Error messages no longer show the query string, fragment or credentials of the server URL.
- Release builds of the uploader ignore the test-only `FTHRUST_UPLOADER_CATBOX_URL` and `FTHRUST_UPLOADER_NO_RETRY_DELAY` variables, and `fthrust-uploader --version` prints the version.

### Hotkeys

- Hyprland binds and portal shortcuts name every key the hotkey popup can record as an xkb keysym (`PageUp` is `Prior`, `Equals` is `equal`, `Backtick` is `grave`…) from one table; alpha.2 wrote the stored names (`Pageup`, `Openbracket`), which Hyprland rejects, and a key with no keysym is now left out instead.
- The Hotkeys popup records Shift plus a shifted symbol as the key it sits on (Shift+1, not Shift+!, on a US layout); Hyprland binds a key as typed with no modifier, so alpha.2's stored shifted names (`exclam`, `question`…) never fired.
- Hotkeys stored as Shift plus a shifted symbol, whether by alpha.2 (`Ctrl+Shift+Exclamationmark`) or carried over from the Qt app (`Ctrl+Shift+!`), are handed to Hyprland and the desktop as the key they are typed on (`Ctrl+Shift+1`), so they fire without being recorded again; the symbols are `!`, `?`, `:`, `|`, `{`, `}` and `+` on a US layout.
- A desktop that describes a shortcut in other words (`Ctrl+Page Up` for `Ctrl+Pageup`) no longer raises a false "desktop kept its shortcut" warning or rewrites the stored binding.
- GlobalShortcuts portal calls have time limits, so a desktop that never answers can't hang hotkey setup for good.
- A `hotkeys.json` that can't be read at startup is left alone for the session (the defaults are used, and the startup notice says so) instead of being replaced by the defaults the next time the app saved its hotkeys.

### Settings, files and logs

- The playback and microphone pickers list your devices (they showed only the saved one), and their Refresh buttons work; Export theme asks where to save the ZIP and writes it. In alpha.2 these did nothing.
- A clip length that changes the replay buffer shows Apply like the other capture settings; raising 30 s to 120 s used to keep a ~32 s buffer without saying so.
- USE DEFAULT for the clips folder commits `~/Videos/FTHR Clips`; it used to remove the setting, so saves failed until a restart and the next start moved the library to `~/FTHR_Clips`.
- Dragging a slider or colour picker writes the settings once it stops instead of on every step.
- A settings file the app cannot read is never overwritten with defaults at exit, and unreadable uploader settings, upload history, presets, export presets and theme files are protected the same way; a damaged one is kept as `<name>.corrupt` before it is replaced (alpha.2 overwrote them). The startup notices about an unreadable or damaged `settings.json` or `theme.json` show even when error notifications are turned off, and a damaged `theme.json` is moved to `theme.json.corrupt` once instead of being found damaged at every launch.
- A full or read-only disk no longer stops the app from opening: a failed settings migration save or theme folder is reported in a STARTUP PROBLEM notice instead of exiting with status 2, so clips can still be deleted to make room. The app also opens when its diagnostics session cannot start (alpha.2 fell back to a session in the temp directory and panicked when that failed too).
- Settings, presets, upload history and the uploader's credentials are written `0600` and `~/.fthr/uploader` is `0700`; alpha.2 left them `0644` in a `0755` directory and reset any chmod on every save.
- The Hyprland Lua keybind file is edited atomically and never from a guess: alpha.2 treated a file it could not read (not UTF-8, no permission, an I/O error) as empty and rewrote it with only the managed block. Now an unreadable file is left alone, the old contents stay in `keybinds.lua.fthr-bak` (never your own `.bak`), the file is rewritten in place where a rename is refused, and a symlinked file stays a symlink.
- Theme import refuses archives over 1,000 entries, 256 MiB of assets or a 4 MiB `theme.json`, and archives that use a codec other than Deflate or Stored (bzip2, zstd, lzma, ppmd, deflate64, AES); it swaps the new icons, sounds and fonts in only after the whole archive was read. alpha.2 read everything into memory with no total limit and deleted the old assets first. An app killed in the middle of an import puts all of the previous icons, sounds and fonts back at the next start (alpha.2 put back only the folder it was moving, or deleted the old files when none was missing); if they cannot be put back, the next start says where they are kept, and a failed import never deletes them.
- The app log (`fthr.log`) and the crash log (`fthr-crash.log`) are written, at info for FTHRust and warnings for the rest (alpha.2 never opened them and, without `RUST_LOG`, showed errors only), and they and the engine log are created `0600`. A normal exit, a logout or the compositor exiting removes the session marker, so the next launch no longer reports a crash that did not happen, and the diagnostic report reads the engine's real log. Asking for a diagnostic report when diagnostics could not start no longer leaves a marker either.
- Diagnostic redaction covers `sftp_password`, `client_secret`, `upload_token`, `catbox_userhash`, `userHash` / `user_hash`, a userhash named in text and every pair of a Cookie header (alpha.2 only matched whole-word keys and cut a cookie at its first `;`), and `events.jsonl` in a report is redacted per record and stays valid JSON.

### The window and overlays

- The UI no longer repaints for every background event (health once a second, save progress, window titles), and idle timers wake far less often: the hotkey and activation sockets, the engine supervisor and its exit watcher used to poll 10 to 67 times a second each.
- Slow PipeWire or compositor connections no longer freeze the window: the sound player, the window list and the microphone meter open on their own threads, and the recording's "Finalizing…" check has a time limit.
- Customize no longer re-rasterises every icon each frame.
- The second and later capture cards show their text (they were blank, or the overlay crashed and showed no more cards), cards don't create a new GPU device each time, and the card lets clicks through to the game underneath.
- Gary uses its picture at on-screen size and no longer copies it 30 times a second while shown.
- The webcam history is capped at 256 MB as well as 90 seconds, so a 1080p MJPEG camera holds 30 to 60 seconds instead of 400 to 800 MB of memory (the reference kept 90 seconds regardless).

### Releases

- The release binaries are built with fat LTO and a single codegen unit, a third smaller than alpha.2's (50.3 MB against 76.7 MB for the app, engine and uploader together).
- A release tag no longer publishes on its own: the whole CI has to pass on the tagged commit, and the GitHub release carries that version's section of this file, a build provenance attestation for the AppImage and the tarball, and the source of the bundled FFmpeg and of every library compiled into it (alpha.2 pointed at a BtbN build that is deleted after about two years).
- THIRD_PARTY_NOTICES.md lists every external library the bundled FFmpeg is built with, about sixty with their licence texts (alpha.2 named four), generated from the build's configure line and checked against BtbN's build scripts.
- The notices say that what alpha.2 called "LGPL" is not entirely so: FFTW (GPL-2.0-or-later), which BtbN compiles into `libavformat` for Chromaprint, makes that library a GPL one; the release carries FFTW's source next to FFmpeg's.
- The tarball's notices link to licence texts that are inside the tarball (they pointed at a folder that only existed under share/).
- The launcher is named "FTHRust Clips", like the window; it was "FTHR Clips", the original app's name.

## 2.0.0-alpha.2 and earlier: changes relative to FTHR Clips linux-2.0 (aa0dc9b)

### Fixed

- The uploader ships inside the app as `fthrust-uploader` and is turned on with the Upload checkbox; the reference's install root, bundle hash and executable were Windows-only, so uploading could not be turned on under Linux.
- The uploader's own Terms and Privacy Policy are a link in the Upload settings instead of a gate. The reference made you tick both before enabling uploads, and refused every upload after a version change until you accepted them again. Catbox's consent is unchanged.
- Saved SFTP passwords work on Linux through the desktop keyring (the reference required Windows Credential Manager).
- Test connection reports a refused or missing endpoint: your server's 401/403 ("rejected the authorization header") and other 4xx/5xx replies fail, as do Catbox 5xx replies (the reference reported any HTTP reply as reachable).
- Uploads retry only network errors, 429 and 5xx replies, and never past 540 s (the reference retried every failure, including a bad URL, and could outlive its own 600 s timeout).
- A card's "Uploading…" state clears when the upload finishes or fails, so the share button and RETRY NOW work again without a restart.
- The clip card's right-click menu has the "Upload" entry the settings label promises.
- Interval uploads also scan imported folders, which were already allowed upload roots.
- Quitting stops a running upload instead of leaving the uploader process behind.
- The editor's Upload button appears and disappears as uploads are switched on or off, instead of being fixed when the editor opened.
- Your server's failures say what to fix instead of echoing the reply body: a missing or malformed token, a URL with no path or nothing behind it, a redirect (with the address to use), a clip too large, an unsupported type, rate limiting, server errors, a refused connection, an unknown host name, and http/https mixed up. Settings problems put OPEN UPLOAD SETTINGS on the UPLOAD FAILED bar.
- Test connection to your server no longer follows redirects: uploads do not follow them, so a test that passed through one promised uploads that then failed. A bare address (no `/upload`) no longer passes either.
- A long Test connection result wraps under the settings panel instead of running off its edge.

- Image layers and the webcam are burned into a saved clip in one re-encode (the reference re-encoded the whole clip once per overlay), with every audio track, its title and the audio manifest kept.
- Webcam frames are placed at their capture times; the reference wrote them at a fixed 30 fps, so a 15 fps camera played at double speed and drifted.
- An unreadable image layer is skipped with its own warning ("Image overlay could not be read: …") instead of failing every layer.
- Cameras are listed with their V4L2 names (libavdevice) instead of "Camera 1…8" from probing OpenCV indices; the running camera is never reopened by a refresh.
- Every Settings browse button works through the desktop file chooser portal; theme, font, icon and sound import failures show a message instead of only printing to the console. Custom icons are resized and stored without the reference's crop dialog.
- The saved card's thumbnail refreshes after its overlays are burned in.
- Gary mode draws on a click-through layer-shell surface (Qt's stay-on-top hints do nothing on Wayland).

- Exports run in-process through FFmpeg's libraries instead of the `ffmpeg` command line; the same graphs, encoders and validation apply.
- A sped-up export of several kept ranges keeps every range's video: speed now applies to each range's audio before the concat (the reference sped audio up after it, so `-shortest` cut the later ranges).
- Ctrl+Q in the editor is only the quick crop: egui's built-in Ctrl+Q quit (which closed the whole app while a clip was still loading) and its Ctrl+plus/minus zoom are off.
- The reference's "FFMPEG NOT FOUND" error cannot occur: there is no `ffmpeg` binary to find.
- A ready share window offers "Copy path" and "Open folder" instead of dragging the file out: winit cannot start a file drag on Wayland.
- Leaving an editor (minimize, clip navigation) is refused only while one of its shares runs; queued exports keep running while you browse, as the jobs window promises. The reference's guard watched a per-editor export thread that nothing starts any more.

- The editor's "Ready" status clears after 900 ms as intended (the reference compared it to "READY" and never cleared it).
- The editor's Color & Effects panel title shows "COLOR & EFFECTS" instead of Qt's mnemonic rendering "COLOR_EFFECTS".

- Screenshots go through the Screenshot portal, so the hotkey no longer produces black images on Wayland.
- Game detection and focus pause work on Wayland through the compositor's toplevel list instead of `xdotool`.
- Manual recording works on Linux (the reference disabled the button there).
- Hyprland hotkey binds call the app's own `--hotkey` client instead of depending on `nc`.
- The capture card is a layer-shell overlay instead of a Qt tool window that compositors could hide; without layer-shell it falls back to a desktop notification.

- Settings pages keep their real width when a scrollbar appears instead of reflowing wrapped labels under it; the Audio page hides the scrollbar when it fits.
- A navigation chord that conflicts with another binding is reported next to the bindings and the old binding is kept, instead of silently overwriting it.
- Theme edits are a draft until Apply theme; Reset all restores every default in one step.

- Settings file that is valid JSON but not an object is quarantined instead of crashing the loader.
- A failed quarantine rename and undecodable settings files no longer stop startup; defaults are used as in the reference's intent.
- Hotkeys loader drops non-string bindings instead of failing on the whole file.
- Theme loader replaces invalid colour strings with the token default instead of propagating them.
- Theme import validates archive entry names and the asset names inside `theme.json`, so a crafted theme cannot read or write outside the theme folders.
- Re-selecting an already installed custom icon, sound or font no longer truncates the file; unknown sound keys are rejected.
- Clip metadata store surfaces a locked database instead of falling back to stale JSON, and patches rows without dropping unknown keys or unparseable drafts.
- Library scan ignores an imported folder that duplicates the clips root or another import, so owned clips are not flagged as imported.
- Clip rename restores the media name when the sidecar step fails, and the rollback is covered by a test.
- Thumbnail cache keys hash raw path bytes, so clips with non-UTF-8 names reuse their thumbnails.
- Diagnostic event fields named like secrets are redacted wholesale, not only when their value looks like a credential.
- Error-bar titles are normalised (trimmed, upper-cased) before code lookup exactly as the reference does.
- A transient read error on `settings.json` or `presets.json` no longer renames the file; corrupt `presets.json` is quarantined before any rewrite instead of being overwritten.
- Theme zip import validates colour values and caps each asset at 64 MiB.
- Export presets this version cannot parse are carried through rewrites untouched instead of being deleted.
- `hotkeys.json` that cannot be read loads defaults instead of blocking startup.
- Diagnostic events and exported log tails replace the home directory with `%USERPROFILE%`, so the export's `paths_redacted` attestation is true.
- Saving a clip on a filesystem without `renameat2(RENAME_NOREPLACE)` (overlayfs, some NFS and FUSE mounts) falls back to `link` + `unlink`, so the no-replace guarantee holds instead of failing the save.
- When the AAC encoder cannot be set up the clip is still written video-only, as before, but the caller is now told (`ClipWriteResult::audio_error`) instead of silently producing a silent clip.
- Continuous recording to a recoverable fragmented MP4 is available on Linux; the reference only built it for Windows. A crash loses at most the group of pictures in progress.
- The encoder validates the source stride and buffer length and rebuilds its scaler when the source size changes, instead of reading past a short buffer or feeding a stale geometry.
- VA-API device selection tries `FTHR_VAAPI_DEVICE`, then `/dev/dri/renderD128`, then every other render node, until one of them actually opens the encoder, so a mixed-GPU machine (NVIDIA on the first node, AMD on the second) still gets hardware encoding.
- Media metadata treats bitrates or dimensions at or above 2^63 as unknown instead of wrapping.
- Encoder capability discovery keys its cache on the loaded libavcodec, since FTHRust links FFmpeg instead of shelling out to an `ffmpeg` executable, and runs each one-frame probe in a child process (the app re-invoked with `--fthrust-probe-encoder`) so a crashing or hanging GPU driver costs one candidate instead of the whole app.
- Starting a continuous recording refuses a path that already exists instead of truncating that file.
- The FFmpeg wrappers are private to the media crate; the scaler and muxer validate frame geometry and stream indexes, and a zero-size encoder packet is copied as empty instead of dereferencing a null pointer.
- Saving a clip no longer blocks the engine's command loop: saves run on their own thread, so status, pause, resume and shutdown keep flowing during a long mux.
- The engine no longer relies on `PR_SET_PDEATHSIG` (which is per parent thread): it exits when the UI closes its stdin.
- The shared-memory region (created with `shm_open` without exclusivity) is replaced by a per-user Unix socket that refuses to replace a live engine.
- The cursor is composited into wlr-screencopy and ext-image-copy-capture frames (the reference captured without it).
- Output transforms (rotated or flipped monitors) are undone before encoding; the reference encoded them rotated.
- Recovery falls through to the next capture backend once the last good one has spent its retry budget, instead of retrying the same backend forever; transient conditions (compositor not answering, selected output absent) are waited out without consuming the budget.
- Audio capture stops within a bounded time (the reference's unbounded `pa_simple_read` could wedge shutdown); audio runs on PipeWire directly, with both the desktop mix and the microphone captured by the engine.
- The engine prints `FTHR_STARTUP_ERROR: <CODE>: <detail>` (and sends the same event) for every startup failure, so the UI can show a specific error instead of the generic Error 004.
- A recording starts on an immediately requested keyframe instead of waiting up to a GOP for the next one.
- v1/v2 wlr-screencopy compositors are handled (the reference could only finish on v3).
- Clip saves carry one AAC track per captured source, labelled `Default Mix` and `Microphone`, and report which tracks were written.

### Added

- Clip editor ([the editor guide](docs/library-and-editor.md#the-editor)): timeline with splits, deleted segments, trim handles, zoom, minimap and filmstrip; undo and redo; drafts in `clip_metadata.sqlite3`; speed with pitch preservation, quick crop, crop and stretch dialogs, audio mix, colour and effect values, clip and file details, delete; up to three parked sessions in the rail; clip navigation and the editor keyboard shortcuts. The editor is a layer inside the one window; the reference's basic viewer (editor disabled) is not ported.

- Playback pipeline for the editor ([`fthrust-media` in the architecture](docs/architecture.md#crates)): per-track mixer with `atempo` rate change, keyframe-aware video decoder, the seek-proxy cache for long-GOP clips, the playback service with the PipeWire audio clock and the reference's sync rule, and the wgpu video texture.

- Clips page: the virtualised card grid with thumbnails, filters and sorting, rename and delete, keyboard navigation chords, and the file watcher that keeps it current.
- Engine supervision, replay saves, screenshots, manual recording, hotkey delivery (socket, Hyprland config, GlobalShortcuts portal), game detection, focus pause, the capture card and sounds, single instance and diagnostics export ([capturing](docs/capturing.md)).

- `fthrust-ui` crate: the egui shell. Theme tokens, fonts and icons from the same assets, a frameless Wayland window with the title strip, toolbar, navigation rail, error bar, all seven Settings tabs, the four toolbar popups and the dialogs, each held to the PySide6 reference by a marker-for-marker fidelity gate ([UI fidelity tests](docs/development.md#ui-fidelity-tests)). Visuals change on Apply theme; performance presets and keyboard navigation chords are recorded and validated through the core crate.

- `fthrust-media` crate: FFmpeg encoder with hardware candidates, encoded GOP ring, transactional MP4 writer, fragmented recorder, probe, thumbnails, capability cache and combined-audio policy, tested against reference fixtures and synthetic frames.
- `fthrust-ipc`, `fthrust-wayland`, `fthrust-portal` and `fthrust-engine` crates: the capture daemon with ext-image-copy-capture, wlr-screencopy and ScreenCast-portal backends, PipeWire audio, recovery and health, replay saves, continuous recording and a JSON-over-Unix-socket protocol ([the engine protocol](docs/architecture.md#the-engine-protocol)). A synthetic backend (`FTHRUST_SYNTHETIC_BACKEND=1`) lets the whole daemon run in CI.

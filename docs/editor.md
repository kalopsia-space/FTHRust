# Clip editor

The editor (plan 6b) opens a clip from the library and covers the whole
window while it is active, the way the reference opens its editor
maximized. Parking it returns to the main shell and leaves a button in
the navigation rail.

## Layers

- **Model** (`fthrust_core::editor`): the timeline (segments as source
  fractions, trim handles, kept ranges, the hide-deleted display mapping,
  zoom and pan), snapshots with the 100-step undo stack, the draft codec
  with the reference's validation, speed presets, quick-crop scaling and
  the label formats. Plan 6c's export reads the same model.
- **Controller** (`fthrust_ui::editor::Editor`): one open clip. It builds
  the player on a worker thread, owns the edit state and the undo stack,
  and runs every timer from the `now_s` it is ticked with: the 50 ms scrub
  debounce, the 80 ms kept-range check (25 ms slack) that skips deleted
  spans and pauses and rewinds at the end of the edit, the 450 ms draft
  save, the 900 ms "Ready" label, the 15 s load stall and the filmstrip
  (300 ms after loading, 1500 ms after a pause, cancelled on play).
- **Layer** (`editor::layer`, `timeline_ui`, `sidebar`, `dialogs`): the
  header, video surface, control bar, trim bar, sidebar panels and the
  crop and stretch dialogs, each held to the Qt renders
  `clip-editor`, `-timeline`, `-sidebar`, `-crop`, `-stretch` and `-rail`
  by the fidelity gate.
- **Sessions** (`editor::sessions`): up to three sessions; one active,
  the rest parked with their edit, position, undo history, mix and view.
  `sessions::service` runs first in every frame and consumes library open
  requests, rail clicks and the editor's requests.
- **Keys** (`editor::keys`): the "Clip editor" navigation bindings plus
  F11 and the Application sidebar steps (Shift+Up/Down). Ctrl+C, Ctrl+X
  and Ctrl+V reach egui as clipboard events, so the chord matcher maps
  them back to their chords.

## Drafts

A draft is saved 450 ms after any committed edit into
`~/.fthr/clip_metadata.sqlite3` (`editor_draft`, version 1, bound to the
file's size and mtime). Opening a clip applies its current draft without
an undo step. A changed file, or a draft that fails validation, opens
with defaults. Deleting the clip clears its draft; exporting does not.
Undo history is kept while a session is parked but is not persisted.

## Differences from the reference

- The editor is a layer inside the one window, not a second toplevel.
  Its maximize button maximizes the app window; F11 toggles fullscreen
  and Escape leaves fullscreen before it closes the editor.
- When the clip editor is disabled (`clip_editor_enabled` false) opening
  a clip does nothing and logs once: the reference's basic viewer is not
  ported.
- The "Ready" status clears after 900 ms (the reference compared it to
  "READY" and never cleared it).
- The Color & Effects panel title renders as "COLOR & EFFECTS"; Qt drew
  "COLOR_EFFECTS" because `&` marks a mnemonic in button text.
- Source rows of the audio mix are live as soon as the player is loaded.

## Plan 6c

Plan 6c adds the export and share flows behind the Export and Share
buttons (the editor queues `EditorRequest::Export` / `Share` today), the
export-jobs list, the delete check for running exports, and the live
preview of crop, stretch and colour on the video surface.

## Tests

- `cargo test -p fthrust-core --test editor_model`
- `cargo test -p fthrust-ui --test editor_controller` (lifecycle,
  debounces, kept-range playback, undo, drafts, filmstrip)
- `cargo test -p fthrust-ui --test editor_ui` (timeline and sidebar input)
- `cargo test -p fthrust-ui --test editor_sessions` (sessions, navigation,
  delete, shortcuts, dialogs)
- `cargo test -p fthrust-ui --test fidelity_all clip_editor`
- `tools/uidrive/bench.sh run tools/uidrive/scenarios/editor.sh`

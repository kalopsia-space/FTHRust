# UI fidelity: how the egui shell is held to the Qt reference

FTHRust's window must look the same as FTHR Clips' PySide6 window. The
check is not a screenshot diff: it compares the geometry, colours and text
metrics of every widget the Qt renderer reports against the markers the egui
code registers while it paints.

## The reference dumps

`docs/reference/qt-screens/<screen>.png` and `<screen>.json` are produced by
`tools/render-qt-reference.py`, which drives the reference app offscreen
(PySide6 in `.venv-reference`) and walks its widget tree. Each JSON lists the
screen size and, per widget, its name, class, rect, enabled state, sampled
pixels (`fill`, `edge`, `top` roles with the colour token they must equal)
and, for labels and buttons, the font pixel size, weight, family and advance.

Regenerate them after changing the extractor or when a new screen is added:

```sh
.venv-reference/bin/python tools/render-qt-reference.py --out docs/reference/qt-screens
.venv-reference/bin/pytest tools/test_render_qt_reference.py
```

The reference checkout is read-only; nothing in it is modified.

## Markers in the Rust code

Every widget that has a Qt counterpart registers a marker while painting:

```rust
fidelity::mark(ui, "QPushButton[Apply theme]", rect)
    .sample("fill", rect.left_center(), Color::Accent)
    .edge(Color::Border)
    .text("Apply theme", size::LABEL, Weight::W600)
    .done();
```

Names follow the Qt dump: `objectName` when the reference sets one, else
`<Class>[<text>]`, with `#2`, `#3`… for repeated names in tree order.
Markers are collected per egui pass and read back with `fidelity::take`.

## The comparison

`fidelity::compare` applies these rules to every Qt widget that has a marker
of the same name (markers without a Qt partner are ignored):

| Rule | Tolerance |
|---|---|
| Rect edges (clipped to the window) | 1 px |
| Sampled pixels against their token | exact |
| Text pixel size and weight | exact (weight skipped for `checked` widgets: Qt cannot report pseudo-state fonts) |
| Row height | 1 px |

SSIM of the whole screen against the PNG is printed for information; it is
not a gate because the two renderers rasterise text differently.

A run prints one line per screen and one per failing rule:

```
[fidelity performance] compared 48 markers, 0 failures, ssim Some(0.8993)
  - QLabel[Capturing]: w 36 vs Qt 40
```

## Running the gate

```sh
cargo test -p fthrust-ui --test fidelity_all          # every screen this plan owns
cargo test -p fthrust-ui --test fidelity_all -- --ignored   # screens owned by later plans (expected to fail until they land)
```

Screens are rendered through `egui_kittest` with the wgpu backend; CI uses
Mesa's software Vulkan driver (`WGPU_BACKEND=vulkan`).

## Known, deliberate differences

- Labels under 10 px in the colour-preview mockup use Qt's hinted integer
  glyph advances, which the unhinted layout cannot reproduce. Their markers
  are named `mockup[...]` so they are reported but not gated.
- Colour pickers use egui's picker in a popup instead of `QColorDialog`.
- The Ko-fi button's marker name is truncated at 40 characters, as the Qt
  dump truncates label names.

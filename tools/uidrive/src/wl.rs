//! The Wayland session on the bench socket: outputs (for the pointer
//! extent and grim), a virtual pointer and a virtual keyboard.

use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use xkbcommon::xkb;

use crate::bench::Bench;
use crate::keys::{Chord, Mods, char_key};

/// Pointer buttons (evdev codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// BTN_LEFT.
    Left,
    /// BTN_RIGHT.
    Right,
    /// BTN_MIDDLE.
    Middle,
}

impl Button {
    fn code(self) -> u32 {
        match self {
            Button::Left => 0x110,
            Button::Right => 0x111,
            Button::Middle => 0x112,
        }
    }

    /// Parses `left`, `right`, `middle`.
    pub fn parse(s: &str) -> Result<Button, String> {
        match s {
            "left" => Ok(Button::Left),
            "right" => Ok(Button::Right),
            "middle" => Ok(Button::Middle),
            other => Err(format!("unknown button '{other}'")),
        }
    }
}

struct OutputSlot {
    output: wl_output::WlOutput,
    name: Option<String>,
    mode: Option<(u32, u32)>,
}

#[derive(Default)]
struct State {
    outputs: Vec<OutputSlot>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(slot) = state.outputs.iter_mut().find(|s| s.output == *output) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => slot.name = Some(name),
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                slot.mode = Some((width.max(0) as u32, height.max(0) as u32));
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore ZwlrVirtualPointerV1);
delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore ZwpVirtualKeyboardV1);

/// A connected session.
pub struct Session {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
    extent: (u32, u32),
    output_name: String,
    mod_masks: ModMasks,
    started: Instant,
}

struct ModMasks {
    shift: u32,
    ctrl: u32,
    alt: u32,
    logo: u32,
}

impl Session {
    /// Connects to the bench socket and creates the virtual devices.
    pub fn connect(bench: &Bench) -> Result<Session, String> {
        let path = bench.runtime_dir.join(&bench.socket);
        let stream = UnixStream::connect(&path)
            .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?;
        let conn = Connection::from_socket(stream).map_err(|e| format!("wayland: {e}"))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
        let qh = queue.handle();
        let mut state = State::default();
        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=7, ())
            .map_err(|e| format!("wl_seat: {e}"))?;
        let pointer_mgr: ZwlrVirtualPointerManagerV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|e| format!("zwlr_virtual_pointer_manager_v1: {e}"))?;
        let keyboard_mgr: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("zwp_virtual_keyboard_manager_v1: {e}"))?;
        for g in globals.contents().clone_list() {
            if g.interface == "wl_output" {
                let output: wl_output::WlOutput =
                    globals.registry().bind(g.name, g.version.min(4), &qh, ());
                state.outputs.push(OutputSlot {
                    output,
                    name: None,
                    mode: None,
                });
            }
        }
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("wayland roundtrip: {e}"))?;
        let (extent, output_name) = state
            .outputs
            .iter()
            .find_map(|s| s.mode.map(|m| (m, s.name.clone().unwrap_or_default())))
            .ok_or("the bench compositor advertises no output with a current mode")?;
        let pointer = pointer_mgr.create_virtual_pointer(Some(&seat), &qh, ());
        let keyboard = keyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
        // Keymap: US layout, handed over as a file in the bench runtime dir.
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .ok_or("xkbcommon could not build the US keymap")?;
        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let keymap_path = bench.runtime_dir.join("uidrive-keymap.xkb");
        std::fs::write(&keymap_path, text.as_bytes()).map_err(|e| format!("keymap file: {e}"))?;
        let file = std::fs::File::open(&keymap_path).map_err(|e| format!("keymap file: {e}"))?;
        keyboard.keymap(
            xkb::KEYMAP_FORMAT_TEXT_V1,
            file.as_fd(),
            text.len() as u32 + 1,
        );
        let mask = |name: &str| -> u32 {
            let idx = keymap.mod_get_index(name);
            if idx == xkb::MOD_INVALID { 0 } else { 1 << idx }
        };
        let mod_masks = ModMasks {
            shift: mask(xkb::MOD_NAME_SHIFT),
            ctrl: mask(xkb::MOD_NAME_CTRL),
            alt: mask(xkb::MOD_NAME_ALT),
            logo: mask(xkb::MOD_NAME_LOGO),
        };
        let mut s = Session {
            conn,
            queue,
            state,
            pointer,
            keyboard,
            extent,
            output_name,
            mod_masks,
            started: Instant::now(),
        };
        s.flush()?;
        Ok(s)
    }

    /// The output size in pixels (the pointer extent).
    pub fn extent(&self) -> (u32, u32) {
        self.extent
    }

    /// The output's name (for grim).
    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    fn flush(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|e| format!("wayland roundtrip: {e}"))?;
        self.conn.flush().map_err(|e| format!("wayland flush: {e}"))
    }

    fn settle(&mut self, ms: u64) -> Result<(), String> {
        self.flush()?;
        std::thread::sleep(Duration::from_millis(ms));
        Ok(())
    }

    /// Moves the pointer to `(x, y)` in output pixels.
    pub fn move_to(&mut self, x: f64, y: f64) -> Result<(), String> {
        let (w, h) = self.extent;
        let cx = x.round().clamp(0.0, f64::from(w)) as u32;
        let cy = y.round().clamp(0.0, f64::from(h)) as u32;
        let t = self.now_ms();
        self.pointer.motion_absolute(t, cx, cy, w, h);
        self.pointer.frame();
        self.settle(10)
    }

    fn button(&mut self, button: Button, pressed: bool) -> Result<(), String> {
        let t = self.now_ms();
        let state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        self.pointer.button(t, button.code(), state);
        self.pointer.frame();
        self.flush()
    }

    /// Clicks at the current position.
    pub fn click(&mut self, button: Button, double: bool) -> Result<(), String> {
        let presses = if double { 2 } else { 1 };
        for i in 0..presses {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(80));
            }
            self.button(button, true)?;
            std::thread::sleep(Duration::from_millis(30));
            self.button(button, false)?;
        }
        self.settle(30)
    }

    /// Moves to `(x, y)` on a fresh virtual pointer and lets the seat focus
    /// the surface before a press: older wlroots (Ubuntu's cage) drops a
    /// press that follows a new pointer's first motion (seen on CI as an
    /// app receiving the release without the press).
    pub fn settle_pointer(&mut self, x: f64, y: f64) -> Result<(), String> {
        self.move_to(x, y)?;
        self.move_to(x, y)?;
        std::thread::sleep(Duration::from_millis(60));
        Ok(())
    }

    /// Wheel scroll at the current position, in steps of 15 logical px.
    pub fn scroll(&mut self, dy: f64) -> Result<(), String> {
        let steps = (dy.abs() / 15.0).ceil().max(1.0) as i32;
        let step = dy.signum() * 15.0;
        for _ in 0..steps {
            let t = self.now_ms();
            self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
            self.pointer.axis(t, wl_pointer::Axis::VerticalScroll, step);
            self.pointer.axis_discrete(
                t,
                wl_pointer::Axis::VerticalScroll,
                step,
                dy.signum() as i32,
            );
            self.pointer.frame();
            self.settle(15)?;
        }
        self.settle(60)
    }

    /// Presses at `from`, moves in `steps`, releases at `to`.
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64), steps: usize) -> Result<(), String> {
        self.settle_pointer(from.0, from.1)?;
        self.button(Button::Left, true)?;
        std::thread::sleep(Duration::from_millis(30));
        let steps = steps.max(1);
        for i in 1..=steps {
            let f = i as f64 / steps as f64;
            self.move_to(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f)?;
        }
        self.button(Button::Left, false)?;
        self.settle(30)
    }

    fn set_mods(&mut self, mods: Mods) {
        let mut depressed = 0;
        if mods.shift {
            depressed |= self.mod_masks.shift;
        }
        if mods.ctrl {
            depressed |= self.mod_masks.ctrl;
        }
        if mods.alt {
            depressed |= self.mod_masks.alt;
        }
        if mods.logo {
            depressed |= self.mod_masks.logo;
        }
        self.keyboard.modifiers(depressed, 0, 0, 0);
    }

    fn tap(&mut self, keycode: u32, mods: Mods) -> Result<(), String> {
        self.set_mods(mods);
        let t = self.now_ms();
        self.keyboard.key(t, keycode, 1);
        self.flush()?;
        std::thread::sleep(Duration::from_millis(20));
        let t = self.now_ms();
        self.keyboard.key(t, keycode, 0);
        self.set_mods(Mods::default());
        self.settle(10)
    }

    /// Presses a chord.
    pub fn key(&mut self, chord: &Chord) -> Result<(), String> {
        self.tap(chord.keycode, chord.mods)
    }

    /// Types ASCII text; refuses characters outside the US layout.
    pub fn type_text(&mut self, s: &str) -> Result<(), String> {
        for c in s.chars() {
            let (code, shift) =
                char_key(c).ok_or_else(|| format!("cannot type {c:?} on the US layout"))?;
            self.tap(
                code,
                Mods {
                    shift,
                    ..Mods::default()
                },
            )?;
        }
        Ok(())
    }
}

/// Captures the bench output with grim into `shots/<NNN>-<name>.png`.
pub fn shot(bench: &Bench, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&bench.shots).map_err(|e| e.to_string())?;
    let n = std::fs::read_dir(&bench.shots)
        .map(|d| d.count())
        .unwrap_or(0)
        + 1;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = bench.shots.join(format!("{n:03}-{safe}.png"));
    let status = std::process::Command::new("grim")
        .arg(&path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("WAYLAND_DISPLAY", &bench.socket)
        .env("XDG_RUNTIME_DIR", &bench.runtime_dir)
        .status()
        .map_err(|e| format!("cannot run grim: {e}"))?;
    if !status.success() {
        return Err(format!("grim exited with {status}"));
    }
    Ok(path)
}

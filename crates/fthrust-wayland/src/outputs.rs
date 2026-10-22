//! `wl_output` tracking: names (v4), current mode, transform and scale.

use wayland_client::Proxy;
use wayland_client::protocol::wl_output;

use crate::frame::OutputTransform;

/// What is known about one output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputInfo {
    /// `wl_output.name` (e.g. `DP-1`); empty before v4 announces it.
    pub name: String,
    /// `wl_output.description`.
    pub description: String,
    /// Current mode width in buffer pixels.
    pub width: u32,
    /// Current mode height.
    pub height: u32,
    /// Current mode refresh in mHz.
    pub refresh_mhz: u32,
    /// Integer scale.
    pub scale: i32,
    /// Output transform.
    pub transform: OutputTransform,
    /// Whether a `done` event closed the first batch.
    pub done: bool,
}

/// The bound outputs in announcement order.
#[derive(Default)]
pub struct OutputTracker {
    entries: Vec<(u32, wl_output::WlOutput, OutputInfo)>,
}

impl OutputTracker {
    /// Registers a bound output under its registry name.
    pub fn add(&mut self, registry_name: u32, output: wl_output::WlOutput) {
        self.entries.push((
            registry_name,
            output,
            OutputInfo {
                scale: 1,
                ..OutputInfo::default()
            },
        ));
    }

    /// Forgets an output the registry removed.
    pub fn remove(&mut self, registry_name: u32) {
        self.entries.retain(|(name, _, _)| *name != registry_name);
    }

    /// Mutable info for a proxy.
    pub fn info_mut(&mut self, output: &wl_output::WlOutput) -> Option<&mut OutputInfo> {
        self.entries
            .iter_mut()
            .find(|(_, o, _)| o.id() == output.id())
            .map(|(_, _, info)| info)
    }

    /// Every output's info.
    pub fn outputs(&self) -> Vec<&OutputInfo> {
        self.entries.iter().map(|(_, _, info)| info).collect()
    }

    /// The output named `wanted` (empty = the first announced output that
    /// completed its `done`).
    pub fn select(&self, wanted: &str) -> Option<(&wl_output::WlOutput, &OutputInfo)> {
        self.entries
            .iter()
            .filter(|(_, _, info)| info.done)
            .find(|(_, _, info)| wanted.is_empty() || info.name == wanted)
            .map(|(_, o, info)| (o, info))
    }

    /// Number of outputs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no output is known.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

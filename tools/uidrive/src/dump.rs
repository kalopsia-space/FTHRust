//! The app's `FTHRUST_UI_DUMP` file: parse, find widgets, wait for state.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// One named widget rectangle (logical pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Fidelity name, e.g. `QPushButton[Record]`.
    pub name: String,
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Text drawn in it, if any.
    #[serde(default)]
    pub text: Option<String>,
}

impl Marker {
    /// The centre point.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// The error bar's current entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBar {
    /// Title.
    pub title: String,
    /// Detail.
    pub detail: String,
    /// `error` or `warning`.
    pub level: String,
}

/// One frame's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dump {
    /// egui pass number.
    pub frame: u64,
    /// egui time.
    pub time: f64,
    /// Must be 1.0.
    pub pixels_per_point: f32,
    /// `clips` or `settings`.
    pub page: String,
    /// The status strip text.
    pub status: String,
    /// Every marker painted this frame.
    pub markers: Vec<Marker>,
    /// The error bar, if shown.
    #[serde(default)]
    pub error_bar: Option<ErrorBar>,
    /// The front dialog's title, if any.
    #[serde(default)]
    pub dialog: Option<String>,
}

/// Reads and validates a dump.
pub fn read(path: &Path) -> Result<Dump, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let d: Dump =
        serde_json::from_slice(&bytes).map_err(|e| format!("bad dump {}: {e}", path.display()))?;
    if (d.pixels_per_point - 1.0).abs() > 1e-6 {
        return Err(format!(
            "dump has pixels_per_point {}; the bench must run at scale 1",
            d.pixels_per_point
        ));
    }
    Ok(d)
}

/// Exact name, then exact text, then name prefix; `nth` among the first
/// tier that matches.
pub fn find<'a>(d: &'a Dump, query: &str, nth: usize) -> Option<&'a Marker> {
    type Tier<'q> = Box<dyn Fn(&Marker) -> bool + 'q>;
    let tiers: [Tier<'_>; 3] = [
        Box::new(|m| m.name == query),
        Box::new(|m| m.text.as_deref() == Some(query)),
        Box::new(|m| m.name.starts_with(query)),
    ];
    for tier in tiers {
        let matches: Vec<&Marker> = d.markers.iter().filter(|m| tier(m)).collect();
        if !matches.is_empty() {
            return matches.get(nth).copied();
        }
    }
    None
}

/// Polls the dump every 50 ms until `pred` holds on a frame newer than
/// `after_frame`, or `timeout` passes. An action that changes nothing on
/// screen produces no new frame: after a 250 ms grace with no newer frame
/// the current one is judged as it stands.
pub fn wait(
    path: &Path,
    pred: impl Fn(&Dump) -> bool,
    timeout: Duration,
    after_frame: Option<u64>,
    what: &str,
) -> Result<Dump, String> {
    let start = Instant::now();
    let deadline = start + timeout;
    let grace = Duration::from_millis(250);
    let mut last: Option<Dump> = None;
    loop {
        if let Ok(d) = read(path) {
            let newer = after_frame.is_none_or(|f| d.frame > f);
            let settled = !newer && start.elapsed() >= grace && after_frame == Some(d.frame);
            if (newer || settled) && pred(&d) {
                return Ok(d);
            }
            if newer || settled {
                last = Some(d);
            }
        }
        if Instant::now() >= deadline {
            let seen = last
                .map(|d| {
                    format!(
                        "last frame {}: page {}, status {:?}, dialog {:?}",
                        d.frame, d.page, d.status, d.dialog
                    )
                })
                .unwrap_or_else(|| "no newer frame".to_owned());
            return Err(format!(
                "timed out after {}s waiting for {what} ({seen})",
                timeout.as_secs_f64()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

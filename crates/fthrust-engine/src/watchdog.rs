//! The content watchdog: once per `fps` frames, a 16×9 sparse luma sample
//! classifies the screen as black-like or uniform; twelve suspicious
//! samples in a row raise `CONTENT_SUSPECT`. Privacy-safe: no image is
//! retained. Ports `capture_engine.cpp:345-391` over the core sampler.

use fthrust_core::capture_health::{ContentSuspicionTracker, sample_bgra_grid};
use fthrust_wayland::frame::BgraFrame;

/// One sample's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentMetrics {
    /// Samples taken so far.
    pub sequence: u32,
    /// Consecutive suspicious samples.
    pub streak: u32,
    /// Luma mean of this sample.
    pub luma_mean: f32,
    /// Luma variance of this sample.
    pub luma_variance: f32,
    /// Whether the streak reached the threshold.
    pub suspect: bool,
}

/// Samples every `fps` frames.
pub struct ContentWatchdog {
    every: u64,
    frames: u64,
    sequence: u32,
    tracker: ContentSuspicionTracker,
}

impl ContentWatchdog {
    /// Grid size.
    pub const COLUMNS: usize = 16;
    /// Grid size.
    pub const ROWS: usize = 9;

    /// One sample per second of frames.
    pub fn new(fps: u32) -> Self {
        Self {
            every: u64::from(fps.max(1)),
            frames: 0,
            sequence: 0,
            tracker: ContentSuspicionTracker::new(),
        }
    }

    /// Counts a frame; returns metrics when a sample was taken.
    pub fn observe(&mut self, frame: &BgraFrame) -> Option<ContentMetrics> {
        self.frames += 1;
        if !self.frames.is_multiple_of(self.every) {
            return None;
        }
        let sample = sample_bgra_grid(
            &frame.data,
            frame.width as usize,
            frame.height as usize,
            frame.stride,
            Self::COLUMNS,
            Self::ROWS,
        )
        .ok()?;
        let suspect = self.tracker.observe(&sample);
        self.sequence = self.sequence.wrapping_add(1);
        Some(ContentMetrics {
            sequence: self.sequence,
            streak: self.tracker.repeat_count(),
            luma_mean: sample.luma_mean as f32,
            luma_variance: sample.luma_variance as f32,
            suspect,
        })
    }
}

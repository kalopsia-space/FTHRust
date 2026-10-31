//! The content watchdog: once per `fps` frames, a 16×9 sparse luma sample
//! classifies the screen as black-like or uniform; twelve suspicious
//! samples in a row raise `CONTENT_SUSPECT` and ask for one recovery.
//! Privacy-safe: no image is retained. Ports `capture_engine.cpp:345-391`
//! over the core sampler.

use fthrust_core::capture_health::{ContentSuspicionTracker, sample_bgra_grid};
use fthrust_wayland::frame::BgraFrame;

/// One sample's result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentMetrics {
    /// Samples taken so far.
    pub sequence: u32,
    /// Consecutive suspicious (black-like or uniform) samples.
    pub streak: u32,
    /// Luma mean of this sample.
    pub luma_mean: f32,
    /// Luma variance of this sample.
    pub luma_variance: f32,
    /// Whether the streak reached the threshold.
    pub suspect: bool,
    /// Restart capture: the content just became suspect. Asked once per
    /// run of suspicious samples, so a screen that really is black (a
    /// loading screen, a dark scene) costs one restart, not one every 12 s.
    pub recover: bool,
}

/// Samples every `fps` frames. One watchdog lives for the whole capture,
/// across recoveries, so a run of suspicious samples is followed through
/// the restart it caused.
pub struct ContentWatchdog {
    every: u64,
    frames: u64,
    sequence: u32,
    tracker: ContentSuspicionTracker,
    streak: u32,
    recovery_asked: bool,
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
            streak: 0,
            recovery_asked: false,
        }
    }

    /// Counts a frame; returns metrics when a sample was taken.
    pub fn observe(&mut self, frame: &BgraFrame) -> Option<ContentMetrics> {
        self.observe_raw(
            &frame.data,
            frame.width as usize,
            frame.height as usize,
            frame.stride,
        )
    }

    /// Like [`observe`](Self::observe) over raw BGRA bytes with a stride.
    pub fn observe_raw(
        &mut self,
        data: &[u8],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<ContentMetrics> {
        if !self.tick() {
            return None;
        }
        self.sample_raw(data, width, height, stride)
    }

    /// Counts a frame without sampling it; true when this frame is due a
    /// sample (then call [`sample_raw`](Self::sample_raw)). Repeats of a
    /// static screen tick too, so the cadence matches the frame count.
    pub fn tick(&mut self) -> bool {
        self.frames += 1;
        self.frames.is_multiple_of(self.every)
    }

    /// Takes a sample for a frame that [`tick`](Self::tick) reported due.
    pub fn sample_raw(
        &mut self,
        data: &[u8],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Option<ContentMetrics> {
        let sample =
            sample_bgra_grid(data, width, height, stride, Self::COLUMNS, Self::ROWS).ok()?;
        let suspect = self.tracker.observe(&sample);
        // The reference publishes the run of suspicious samples; the
        // tracker's repeat count (identical samples) also grows on a
        // perfectly normal static desktop.
        self.streak = if sample.black_like || sample.uniform {
            self.streak.saturating_add(1)
        } else {
            0
        };
        if self.streak == 0 {
            self.recovery_asked = false;
        }
        let recover = suspect && !self.recovery_asked;
        if recover {
            self.recovery_asked = true;
        }
        self.sequence = self.sequence.wrapping_add(1);
        Some(ContentMetrics {
            sequence: self.sequence,
            streak: self.streak,
            luma_mean: sample.luma_mean as f32,
            luma_variance: sample.luma_variance as f32,
            suspect,
            recover,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_raw_matches_observe() {
        let (w, h) = (64usize, 36usize);
        let mut data = vec![0u8; w * 4 * h];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let frame = BgraFrame {
            data: data.clone(),
            stride: w * 4,
            width: w as u32,
            height: h as u32,
            ..BgraFrame::default()
        };
        let mut a = ContentWatchdog::new(1);
        let mut b = ContentWatchdog::new(1);
        let via_frame = a.observe(&frame).expect("sample");
        let via_raw = b.observe_raw(&data, w, h, w * 4).expect("sample");
        assert_eq!(via_frame, via_raw);
    }

    fn solid(w: usize, h: usize, grey: u8) -> Vec<u8> {
        vec![grey; w * 4 * h]
    }

    fn gradient(w: usize, h: usize) -> Vec<u8> {
        (0..w * 4 * h)
            .map(|i| (i / 4 % w * 255 / w) as u8)
            .collect()
    }

    #[test]
    fn the_streak_counts_suspicious_samples_not_identical_ones() {
        let (w, h) = (64usize, 36usize);
        let mut dog = ContentWatchdog::new(1);
        // A static, normal picture: every sample is the same, none suspect.
        let desktop = gradient(w, h);
        for _ in 0..5 {
            let m = dog.observe_raw(&desktop, w, h, w * 4).unwrap();
            assert_eq!(m.streak, 0, "{m:?}");
        }
        let black = solid(w, h, 0);
        for expected in 1..=3 {
            let m = dog.observe_raw(&black, w, h, w * 4).unwrap();
            assert_eq!(m.streak, expected, "{m:?}");
        }
        let m = dog.observe_raw(&desktop, w, h, w * 4).unwrap();
        assert_eq!(m.streak, 0, "a normal sample ends the streak");
    }

    #[test]
    fn one_recovery_per_run_of_suspect_content() {
        let (w, h) = (64usize, 36usize);
        let (black, desktop) = (solid(w, h, 0), gradient(w, h));
        let mut dog = ContentWatchdog::new(1);
        let run = |dog: &mut ContentWatchdog, frame: &[u8], samples: u32| -> Vec<u32> {
            (0..samples)
                .filter_map(|_| dog.observe_raw(frame, w, h, w * 4))
                .filter(|m| m.recover)
                .map(|m| m.streak)
                .collect()
        };
        assert_eq!(
            run(&mut dog, &black, 40),
            [12],
            "asked when the content becomes suspect, then not again"
        );
        assert!(run(&mut dog, &desktop, 1).is_empty());
        assert_eq!(run(&mut dog, &black, 12), [12], "a new run asks again");
    }
}

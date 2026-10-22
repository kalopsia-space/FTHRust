//! Lock-based encoded replay ring.
//!
//! Ports `reference/FTHRcapture_linux/src/ring_buffer.{h,cpp}`: packets are
//! pushed from the encode thread, snapshots are taken from the save thread.
//! The ring is time bounded and optionally byte bounded; over budget, whole
//! GOPs are dropped from the front so the oldest retained packet is always
//! a keyframe and the newest GOP is never dropped.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::replay_interval::{self, Sample};

/// One encoded packet with its capture time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPacket {
    /// Compressed payload.
    pub data: Vec<u8>,
    /// Presentation timestamp in `1/fps` ticks.
    pub pts: i64,
    /// Decode timestamp in `1/fps` ticks.
    pub dts: i64,
    /// Whether the packet starts a decodable GOP.
    pub keyframe: bool,
    /// Monotonic nanoseconds when the source frame was captured.
    pub wall_time_ns: i64,
    /// Encoder generation that produced the packet; packets from different
    /// generations carry different extradata and cannot share a track.
    pub generation: u64,
}

/// The packets selected for one save, with the timing facts the writer needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Packets from the decode-start keyframe to the last packet at or
    /// before the target end.
    pub packets: Vec<EncodedPacket>,
    /// Whether the ring held the whole requested duration.
    pub full_history: bool,
    /// The pts at which presentation starts (pre-roll packets have smaller pts).
    pub presentation_start_pts: i64,
    /// Capture time at which presentation starts.
    pub presentation_start_ns: i64,
    /// The requested end (capture time).
    pub presentation_end_ns: i64,
    /// Capture time of the last selected packet.
    pub media_end_ns: i64,
    /// The ring's frame rate (the packet time base is `1/fps`).
    pub fps: u32,
}

/// Counters describing the ring's contents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingStats {
    /// Packets held.
    pub packets: usize,
    /// Payload bytes held.
    pub bytes: u64,
    /// Newest minus oldest capture time.
    pub span_ns: i64,
    /// Times the byte budget dropped history since construction or clear.
    pub budget_trims: u64,
    /// Capture time of the newest packet, 0 when empty.
    pub latest_wall_time_ns: i64,
}

#[derive(Debug, Default)]
struct Inner {
    packets: VecDeque<EncodedPacket>,
    bytes: u64,
    keyframes: usize,
    budget_trims: u64,
    budget_logged: bool,
}

impl Inner {
    fn pop_front(&mut self) {
        if let Some(front) = self.packets.pop_front() {
            self.bytes -= front.data.len() as u64;
            if front.keyframe {
                self.keyframes -= 1;
            }
        }
    }

    fn span_ns(&self) -> i64 {
        match (self.packets.front(), self.packets.back()) {
            (Some(first), Some(last)) => last.wall_time_ns - first.wall_time_ns,
            _ => 0,
        }
    }
}

/// The encoded replay ring.
#[derive(Debug)]
pub struct EncodedRing {
    inner: Mutex<Inner>,
    published: Condvar,
    max_duration_ms: u64,
    fps: u32,
    max_bytes: u64,
    latest_wall_time_ns: AtomicI64,
}

impl EncodedRing {
    /// A ring holding at most `max_duration_ms` of capture time and, when
    /// `max_bytes` is non-zero, at most that many payload bytes.
    pub fn new(max_duration_ms: u64, fps: u32, max_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            published: Condvar::new(),
            max_duration_ms,
            fps,
            max_bytes,
            latest_wall_time_ns: AtomicI64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends a packet, trimming by time and then by byte budget.
    pub fn push(&self, packet: EncodedPacket) {
        {
            let mut inner = self.lock();
            inner.bytes += packet.data.len() as u64;
            if packet.keyframe {
                inner.keyframes += 1;
            }
            inner.packets.push_back(packet);
            while inner.packets.len() > 1 {
                let span_ms = inner.span_ns() / 1_000_000;
                if span_ms <= self.max_duration_ms as i64 {
                    break;
                }
                inner.pop_front();
            }
            let mut trimmed = false;
            while self.max_bytes != 0
                && inner.bytes > self.max_bytes
                && inner.keyframes > usize::from(inner.packets.front().is_some_and(|p| p.keyframe))
                && inner.packets.len() > 1
            {
                inner.pop_front();
                while inner.packets.front().is_some_and(|p| !p.keyframe) {
                    inner.pop_front();
                }
                trimmed = true;
            }
            if trimmed {
                inner.budget_trims += 1;
                if !inner.budget_logged {
                    inner.budget_logged = true;
                    tracing::warn!(
                        budget_mib = self.max_bytes >> 20,
                        kept_seconds = inner.span_ns() / 1_000_000_000,
                        "replay ring byte budget reached; dropping the oldest video"
                    );
                }
            }
            let latest = inner.packets.back().map_or(0, |p| p.wall_time_ns);
            self.latest_wall_time_ns.store(latest, Ordering::Release);
        }
        self.published.notify_all();
    }

    /// Selects the packets covering the last `duration_ms` ending at
    /// `target_end_ns` (0 = the newest packet), restricted to `generation`
    /// (0 = all). `None` when nothing usable is held.
    pub fn snapshot(
        &self,
        duration_ms: u32,
        target_end_ns: i64,
        generation: u64,
    ) -> Option<Snapshot> {
        let inner = self.lock();
        if inner.packets.is_empty() || duration_ms == 0 || self.fps == 0 {
            return None;
        }
        let eligible: Vec<&EncodedPacket> = inner
            .packets
            .iter()
            .filter(|p| generation == 0 || p.generation == generation)
            .collect();
        let last = eligible.last()?;
        let target_end_ns = if target_end_ns <= 0 {
            last.wall_time_ns
        } else {
            target_end_ns
        };
        let samples: Vec<Sample> = eligible
            .iter()
            .map(|p| Sample {
                timestamp: p.wall_time_ns,
                keyframe: p.keyframe,
            })
            .collect();
        let selection =
            replay_interval::select(&samples, target_end_ns, i64::from(duration_ms) * 1_000_000)
                .ok()?;
        let packets: Vec<EncodedPacket> = eligible[selection.decode_start..=selection.end]
            .iter()
            .map(|p| (*p).clone())
            .collect();
        let newest_pts = packets.last().map_or(0, |p| p.pts);
        let first_pts = packets.first().map_or(0, |p| p.pts);
        let presentation_start_pts = replay_interval::presentation_start_pts(
            newest_pts,
            i64::from(duration_ms) * i64::from(self.fps) / 1000,
            selection.full_history,
            first_pts,
        );
        let presentation_start_ns = if selection.full_history {
            target_end_ns - i64::from(duration_ms) * 1_000_000
        } else {
            packets.first().map_or(0, |p| p.wall_time_ns)
        };
        let media_end_ns = packets.last().map_or(0, |p| p.wall_time_ns);
        Some(Snapshot {
            packets,
            full_history: selection.full_history,
            presentation_start_pts,
            presentation_start_ns,
            presentation_end_ns: target_end_ns,
            media_end_ns,
            fps: self.fps,
        })
    }

    /// Waits until a packet captured at or after `target_end_ns` has been
    /// pushed, or `timeout` passes. A non-positive target returns true.
    pub fn wait_until_published(&self, target_end_ns: i64, timeout: Duration) -> bool {
        if target_end_ns <= 0 {
            return true;
        }
        let guard = self.lock();
        let (_guard, result) = self
            .published
            .wait_timeout_while(guard, timeout, |_| {
                self.latest_wall_time_ns.load(Ordering::Acquire) < target_end_ns
            })
            .unwrap_or_else(|e| e.into_inner());
        !result.timed_out() || self.latest_wall_time_ns.load(Ordering::Acquire) >= target_end_ns
    }

    /// Current counters.
    pub fn stats(&self) -> RingStats {
        let inner = self.lock();
        RingStats {
            packets: inner.packets.len(),
            bytes: inner.bytes,
            span_ns: inner.span_ns(),
            budget_trims: inner.budget_trims,
            latest_wall_time_ns: self.latest_wall_time_ns.load(Ordering::Acquire),
        }
    }

    /// Drops everything (encoder reopen or reconfigure).
    pub fn clear(&self) {
        let mut inner = self.lock();
        *inner = Inner::default();
        self.latest_wall_time_ns.store(0, Ordering::Release);
    }

    /// The ring's frame rate.
    pub fn fps(&self) -> u32 {
        self.fps
    }
}

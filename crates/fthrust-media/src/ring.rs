//! Lock-based encoded replay ring.
//!
//! Ports `reference/FTHRcapture_linux/src/ring_buffer.{h,cpp}`: packets are
//! pushed from the encode thread, snapshots are taken from the save thread.
//! The ring is time bounded and optionally byte bounded; over budget, whole
//! GOPs are dropped from the front so the oldest retained packet is always
//! a keyframe and the newest GOP is never dropped.
//!
//! In disk mode a writer thread moves each payload into the segment files
//! after it is pushed, so the encode thread never waits on the disk and
//! never holds the ring lock across a write.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use crate::payload::{Payload, SegmentStore};
use crate::replay_interval::{self, Sample};

/// One encoded packet with its capture time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPacket {
    /// Compressed payload.
    pub data: Payload,
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
    /// Sequence number of `packets.front()`. Numbers never repeat, so the
    /// disk writer can tell the packet it was handed from a later one.
    front_seq: u64,
    bytes: u64,
    keyframes: usize,
    budget_trims: u64,
    budget_logged: bool,
}

impl Inner {
    fn pop_front(&mut self) {
        if let Some(front) = self.packets.pop_front() {
            self.front_seq += 1;
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

    fn clear(&mut self) {
        self.front_seq += self.packets.len() as u64;
        self.packets.clear();
        self.bytes = 0;
        self.keyframes = 0;
        self.budget_trims = 0;
        self.budget_logged = false;
    }

    fn get_mut(&mut self, seq: u64) -> Option<&mut EncodedPacket> {
        let index = usize::try_from(seq.checked_sub(self.front_seq)?).ok()?;
        self.packets.get_mut(index)
    }
}

#[derive(Debug)]
struct Shared {
    inner: Mutex<Inner>,
    published: Condvar,
    latest_wall_time_ns: AtomicI64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A payload waiting to be written: the packet's sequence number and bytes.
type DiskJob = (u64, Arc<[u8]>);

/// The disk side of a disk-mode ring: a queue to its writer thread.
#[derive(Debug)]
struct DiskWriter {
    jobs: mpsc::Sender<DiskJob>,
    /// Set by the writer when the store failed; payloads stay in memory.
    failed: Arc<AtomicBool>,
    queued: AtomicU64,
    done: Arc<AtomicU64>,
}

impl DiskWriter {
    fn spawn(shared: Arc<Shared>, store: SegmentStore) -> std::io::Result<Self> {
        let (jobs, rx) = mpsc::channel::<DiskJob>();
        let failed = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let (thread_failed, thread_done) = (failed.clone(), done.clone());
        std::thread::Builder::new()
            .name("fthrust-replay-io".into())
            .spawn(move || write_payloads(&shared, store, &rx, &thread_failed, &thread_done))?;
        Ok(Self {
            jobs,
            failed,
            queued: AtomicU64::new(0),
            done,
        })
    }

    fn send(&self, job: DiskJob) {
        self.queued.fetch_add(1, Ordering::AcqRel);
        if self.jobs.send(job).is_err() {
            self.done.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// The writer thread: stores each queued payload that the ring still holds
/// and swaps the packet's in-memory payload for the disk reference. Only the
/// swap takes the ring lock; the write itself runs without it.
fn write_payloads(
    shared: &Shared,
    store: SegmentStore,
    jobs: &mpsc::Receiver<DiskJob>,
    failed: &AtomicBool,
    done: &AtomicU64,
) {
    let mut store = Some(store);
    for (seq, bytes) in jobs {
        let held = shared.lock().get_mut(seq).is_some();
        if held && let Some(s) = store.as_mut() {
            match s.store(&bytes) {
                Ok(r) => {
                    let mut inner = shared.lock();
                    if let Some(p) = inner.get_mut(seq)
                        && p.data.as_mem().is_some_and(|m| Arc::ptr_eq(m, &bytes))
                    {
                        p.data = r.into_payload();
                    }
                }
                Err(e) => {
                    tracing::info!(
                        reason = %e,
                        "disk replay buffer unavailable; keeping it in memory"
                    );
                    failed.store(true, Ordering::Release);
                    store = None;
                }
            }
        }
        done.fetch_add(1, Ordering::AcqRel);
    }
}

/// The encoded replay ring.
#[derive(Debug)]
pub struct EncodedRing {
    shared: Arc<Shared>,
    disk: Option<DiskWriter>,
    max_duration_ms: u64,
    fps: u32,
    max_bytes: u64,
}

impl EncodedRing {
    /// A ring holding at most `max_duration_ms` of capture time and, when
    /// `max_bytes` is non-zero, at most that many payload bytes.
    pub fn new(max_duration_ms: u64, fps: u32, max_bytes: u64) -> Self {
        Self {
            shared: Self::shared(),
            disk: None,
            max_duration_ms,
            fps,
            max_bytes,
        }
    }

    /// As [`EncodedRing::new`], but payloads move into `store`'s segment
    /// files on a writer thread and only packet metadata stays in memory. A
    /// packet is in memory until the writer reaches it. If a write fails
    /// the ring logs once and keeps payloads in memory from then on.
    pub fn new_on_disk(
        max_duration_ms: u64,
        fps: u32,
        max_bytes: u64,
        store: SegmentStore,
    ) -> Self {
        let shared = Self::shared();
        let disk = match DiskWriter::spawn(shared.clone(), store) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::info!(
                    reason = %e,
                    "disk replay buffer unavailable; keeping it in memory"
                );
                None
            }
        };
        Self {
            shared,
            disk,
            max_duration_ms,
            fps,
            max_bytes,
        }
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            published: Condvar::new(),
            latest_wall_time_ns: AtomicI64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared.lock()
    }

    /// Appends a packet, trimming by time and then by byte budget. In disk
    /// mode the payload is queued for the writer thread after the lock is
    /// released.
    pub fn push(&self, packet: EncodedPacket) {
        let job = {
            let mut inner = self.lock();
            let seq = inner.front_seq + inner.packets.len() as u64;
            let job = match (&self.disk, &packet.data) {
                (Some(disk), Payload::Mem(bytes)) if !disk.failed.load(Ordering::Acquire) => {
                    Some((seq, bytes.clone()))
                }
                _ => None,
            };
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
            self.shared
                .latest_wall_time_ns
                .store(latest, Ordering::Release);
            job
        };
        if let (Some(disk), Some(job)) = (&self.disk, job) {
            disk.send(job);
        }
        self.shared.published.notify_all();
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
        let (front, back) = (inner.packets.front()?, inner.packets.back()?);
        if duration_ms == 0 || self.fps == 0 {
            return None;
        }
        // Generations only grow, and the engine clears the ring when it
        // reopens the encoder, so a ring of one generation (the usual case)
        // needs no filtered copy of its packets.
        let picked: Option<Vec<usize>> = if generation == 0
            || (front.generation == generation && back.generation == generation)
        {
            None
        } else {
            Some(
                inner
                    .packets
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.generation == generation)
                    .map(|(i, _)| i)
                    .collect(),
            )
        };
        let at = |i: usize| match &picked {
            None => &inner.packets[i],
            Some(indexes) => &inner.packets[indexes[i]],
        };
        let count = picked.as_ref().map_or(inner.packets.len(), Vec::len);
        let samples: Vec<Sample> = (0..count)
            .map(|i| {
                let p = at(i);
                Sample {
                    timestamp: p.wall_time_ns,
                    keyframe: p.keyframe,
                }
            })
            .collect();
        let target_end_ns = if target_end_ns <= 0 {
            samples.last()?.timestamp
        } else {
            target_end_ns
        };
        let duration_ns = i64::from(duration_ms) * 1_000_000;
        let selection = replay_interval::select(&samples, target_end_ns, duration_ns).ok()?;
        let packets: Vec<EncodedPacket> = (selection.decode_start..=selection.end)
            .map(|i| at(i).clone())
            .collect();
        drop(inner);
        let newest_pts = packets.last().map_or(0, |p| p.pts);
        let first_pts = packets.first().map_or(0, |p| p.pts);
        let presentation_start_pts = replay_interval::presentation_start_pts(
            newest_pts,
            i64::from(duration_ms) * i64::from(self.fps) / 1000,
            selection.full_history,
            first_pts,
        );
        let presentation_start_ns = if selection.full_history {
            target_end_ns - duration_ns
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
        let latest = || self.shared.latest_wall_time_ns.load(Ordering::Acquire);
        let guard = self.lock();
        let (_guard, result) = self
            .shared
            .published
            .wait_timeout_while(guard, timeout, |_| latest() < target_end_ns)
            .unwrap_or_else(|e| e.into_inner());
        !result.timed_out() || latest() >= target_end_ns
    }

    /// Current counters.
    pub fn stats(&self) -> RingStats {
        let inner = self.lock();
        RingStats {
            packets: inner.packets.len(),
            bytes: inner.bytes,
            span_ns: inner.span_ns(),
            budget_trims: inner.budget_trims,
            latest_wall_time_ns: self.shared.latest_wall_time_ns.load(Ordering::Acquire),
        }
    }

    /// Drops every packet (encoder reopen or reconfigure). A disk ring
    /// keeps its segment store.
    pub fn clear(&self) {
        self.lock().clear();
        self.shared.latest_wall_time_ns.store(0, Ordering::Release);
    }

    /// The ring's frame rate.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Waits until the disk writer has handled every payload pushed so far,
    /// or `timeout` passes; true when it caught up. Memory rings return at
    /// once (tests, and anyone who must see payloads on disk).
    pub fn wait_disk_writes(&self, timeout: Duration) -> bool {
        let Some(disk) = &self.disk else {
            return true;
        };
        let deadline = Instant::now() + timeout;
        loop {
            if disk.done.load(Ordering::Acquire) >= disk.queued.load(Ordering::Acquire) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for EncodedRing {
    /// Empties the ring so the writer thread skips whatever is still queued
    /// and exits once the queue closes.
    fn drop(&mut self) {
        self.lock().clear();
    }
}

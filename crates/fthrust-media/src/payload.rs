//! Encoded packet payloads, held in memory or in segment files on disk.
//!
//! A disk payload points into a [`Segment`], an append-only file shared by
//! every packet written while it was current. The file is deleted when the
//! last payload referencing it is dropped, so a snapshot keeps its bytes
//! readable after the ring has trimmed them.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

/// Segment size used by the engine.
pub const DEFAULT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;

/// One packet's compressed bytes.
#[derive(Clone, Debug)]
pub enum Payload {
    /// In memory, shared between the ring, saves and recording.
    Mem(Arc<[u8]>),
    /// In a segment file.
    Disk(DiskRef),
}

/// A byte range in a segment file.
#[derive(Clone, Debug)]
pub struct DiskRef {
    segment: Arc<Segment>,
    offset: u64,
    len: u32,
}

/// An append-only replay file, deleted on drop.
#[derive(Debug)]
pub struct Segment {
    file: File,
    path: PathBuf,
}

impl Drop for Segment {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Payload {
    /// Payload length in bytes.
    pub fn len(&self) -> usize {
        match self {
            Payload::Mem(b) => b.len(),
            Payload::Disk(d) => d.len as usize,
        }
    }

    /// Whether the payload is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Calls `f` with the bytes, reading a disk payload into `scratch`.
    pub fn with_bytes<R>(
        &self,
        scratch: &mut Vec<u8>,
        f: impl FnOnce(&[u8]) -> R,
    ) -> io::Result<R> {
        match self {
            Payload::Mem(b) => Ok(f(b)),
            Payload::Disk(d) => {
                scratch.resize(d.len as usize, 0);
                d.segment.file.read_exact_at(scratch, d.offset)?;
                Ok(f(scratch))
            }
        }
    }

    /// The bytes: borrowed in memory, read for a disk payload.
    pub fn bytes(&self) -> io::Result<Cow<'_, [u8]>> {
        match self {
            Payload::Mem(b) => Ok(Cow::Borrowed(b)),
            Payload::Disk(_) => {
                let mut v = Vec::new();
                self.with_bytes(&mut v, |_| ())?;
                Ok(Cow::Owned(v))
            }
        }
    }

    /// The shared in-memory buffer, if the payload is in memory.
    pub fn as_mem(&self) -> Option<&Arc<[u8]>> {
        match self {
            Payload::Mem(b) => Some(b),
            Payload::Disk(_) => None,
        }
    }

    /// The segment file holding a disk payload.
    pub fn segment_path(&self) -> Option<&Path> {
        match self {
            Payload::Mem(_) => None,
            Payload::Disk(d) => Some(&d.segment.path),
        }
    }
}

impl PartialEq for Payload {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Payload::Mem(a), Payload::Mem(b)) => a == b,
            (Payload::Disk(a), Payload::Disk(b)) => {
                Arc::ptr_eq(&a.segment, &b.segment) && a.offset == b.offset && a.len == b.len
            }
            _ => false,
        }
    }
}

impl Eq for Payload {}

impl From<Vec<u8>> for Payload {
    fn from(v: Vec<u8>) -> Self {
        Payload::Mem(v.into())
    }
}

impl From<Arc<[u8]>> for Payload {
    fn from(v: Arc<[u8]>) -> Self {
        Payload::Mem(v)
    }
}

impl From<&[u8]> for Payload {
    fn from(v: &[u8]) -> Self {
        Payload::Mem(v.into())
    }
}

static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(0);

/// Writes payloads into rotating segment files in one private directory.
#[derive(Debug)]
pub struct SegmentStore {
    dir: PathBuf,
    segment_bytes: u64,
    /// Distinguishes this store's files from an earlier store's in the same
    /// directory (a rebuilt ring while a save still reads the old one).
    store_id: u64,
    next_index: u64,
    current: Option<(Arc<Segment>, u64)>,
    /// The segment closed before the current one, for page-cache release.
    previous: Option<Weak<Segment>>,
}

impl SegmentStore {
    /// A store writing `segment_bytes`-sized files into `dir`, which must
    /// exist.
    pub fn new(dir: PathBuf, segment_bytes: u64) -> io::Result<Self> {
        if !dir.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not a directory", dir.display()),
            ));
        }
        Ok(Self {
            dir,
            segment_bytes: segment_bytes.max(1),
            store_id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            next_index: 0,
            current: None,
            previous: None,
        })
    }

    /// The directory segments are written into.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends `data` and returns where it went.
    pub fn store(&mut self, data: &[u8]) -> io::Result<DiskRef> {
        let len = u32::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "payload too large"))?;
        let full = self.current.as_ref().is_none_or(|(_, written)| {
            *written > 0 && written + data.len() as u64 > self.segment_bytes
        });
        if full {
            self.rotate()?;
        }
        let (segment, written) = self.current.as_mut().expect("rotated above");
        segment.file.write_all_at(data, *written)?;
        let r = DiskRef {
            segment: segment.clone(),
            offset: *written,
            len,
        };
        *written += data.len() as u64;
        Ok(r)
    }

    fn rotate(&mut self) -> io::Result<()> {
        let path = self
            .dir
            .join(format!("{:04}-{:08}.seg", self.store_id, self.next_index));
        self.next_index += 1;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let next = (Arc::new(Segment { file, path }), 0);
        if let Some((closed, _)) = self.current.replace(next) {
            // Start writeback of the segment just closed; the one closed
            // before it has had a segment's time to reach the disk, so its
            // pages can leave the cache.
            start_writeback(&closed.file);
            if let Some(prev) = self.previous.take().and_then(|w| w.upgrade()) {
                drop_cache(&prev.file);
            }
            self.previous = Some(Arc::downgrade(&closed));
        }
        Ok(())
    }
}

#[allow(unsafe_code)]
fn start_writeback(file: &File) {
    // SAFETY: a valid open fd; the call only schedules writeback.
    unsafe {
        libc::sync_file_range(file.as_raw_fd(), 0, 0, libc::SYNC_FILE_RANGE_WRITE);
    }
}

#[allow(unsafe_code)]
fn drop_cache(file: &File) {
    // SAFETY: a valid open fd; the call is advisory.
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}

impl DiskRef {
    /// A payload from this reference.
    pub fn into_payload(self) -> Payload {
        Payload::Disk(self)
    }
}

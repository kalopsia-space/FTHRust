//! A memfd-backed `wl_shm_pool` holding one capture buffer that is
//! reallocated when the compositor's geometry changes.
#![allow(unsafe_code)]

use std::os::fd::{AsFd, OwnedFd};

use memmap2::MmapMut;
use nix::sys::memfd::{MFdFlags, memfd_create};
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
use wayland_client::{Dispatch, QueueHandle};

/// Shm failures.
#[derive(Debug, thiserror::Error)]
pub enum ShmError {
    /// `memfd_create`, `ftruncate` or sealing failed.
    #[error("shared memory: {0}")]
    Os(#[from] nix::Error),
    /// Mapping failed.
    #[error("shared memory mapping: {0}")]
    Map(#[from] std::io::Error),
    /// Geometry does not fit `i32` or is zero.
    #[error("invalid buffer geometry {0}x{1} stride {2}")]
    Geometry(u32, u32, u32),
}

/// Geometry of the current buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferGeometry {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// `wl_shm_format` value.
    pub format: u32,
}

/// Number of buffers in a pool.
pub const SLOTS: usize = 2;

/// One pool, two buffers of the same geometry (`2 * stride * height` bytes).
pub struct MemfdPool {
    fd: OwnedFd,
    map: MmapMut,
    size: usize,
    pool: wl_shm_pool::WlShmPool,
    buffers: Option<([wl_buffer::WlBuffer; SLOTS], BufferGeometry)>,
}

impl MemfdPool {
    /// Creates a pool of `bytes` (at least one page).
    pub fn new<S>(shm: &wl_shm::WlShm, qh: &QueueHandle<S>, bytes: usize) -> Result<Self, ShmError>
    where
        S: Dispatch<wl_shm_pool::WlShmPool, ()> + 'static,
    {
        let size = bytes.max(4096);
        let fd = memfd_create(
            c"fthrust-shm",
            MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
        )?;
        nix::unistd::ftruncate(&fd, size as i64)?;
        let _ = nix::fcntl::fcntl(
            &fd,
            nix::fcntl::FcntlArg::F_ADD_SEALS(nix::fcntl::SealFlag::F_SEAL_SHRINK),
        );
        // SAFETY: the memfd is private to this process; nothing else maps or
        // shrinks it (the shrink seal is set), and the mapping is dropped
        // before the fd.
        let map = unsafe { MmapMut::map_mut(&fd)? };
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        Ok(Self {
            fd,
            map,
            size,
            pool,
            buffers: None,
        })
    }

    /// Returns the `wl_buffer` for `slot` at `geometry`, reusing both
    /// buffers when the geometry matches and recreating them (growing the
    /// pool when needed) when it does not.
    pub fn ensure<S>(
        &mut self,
        qh: &QueueHandle<S>,
        geometry: BufferGeometry,
        slot: usize,
    ) -> Result<&wl_buffer::WlBuffer, ShmError>
    where
        S: Dispatch<wl_buffer::WlBuffer, ()> + 'static,
    {
        if geometry.width == 0
            || geometry.height == 0
            || geometry.stride < geometry.width.saturating_mul(4)
            || i32::try_from(geometry.stride).is_err()
            || i32::try_from(geometry.height).is_err()
            || slot >= SLOTS
        {
            return Err(ShmError::Geometry(
                geometry.width,
                geometry.height,
                geometry.stride,
            ));
        }
        let reuse = matches!(&self.buffers, Some((_, current)) if *current == geometry);
        if !reuse {
            let one = geometry.stride as usize * geometry.height as usize;
            let needed = one * SLOTS;
            if i32::try_from(needed).is_err() {
                return Err(ShmError::Geometry(
                    geometry.width,
                    geometry.height,
                    geometry.stride,
                ));
            }
            if needed > self.size {
                nix::unistd::ftruncate(&self.fd, needed as i64)?;
                // SAFETY: as in `new`; the old mapping is replaced before use.
                self.map = unsafe { MmapMut::map_mut(&self.fd)? };
                self.pool.resize(needed as i32);
                self.size = needed;
            }
            if let Some((old, _)) = self.buffers.take() {
                for b in old {
                    b.destroy();
                }
            }
            let format =
                wl_shm::Format::try_from(geometry.format).unwrap_or(wl_shm::Format::Xrgb8888);
            let make = |i: usize| {
                self.pool.create_buffer(
                    (i * one) as i32,
                    geometry.width as i32,
                    geometry.height as i32,
                    geometry.stride as i32,
                    format,
                    qh,
                    (),
                )
            };
            self.buffers = Some(([make(0), make(1)], geometry));
        }
        Ok(&self.buffers.as_ref().expect("set above").0[slot])
    }

    /// The current buffers' geometry.
    pub fn geometry(&self) -> Option<BufferGeometry> {
        self.buffers.as_ref().map(|(_, g)| *g)
    }

    /// The bytes of `slot` (`stride * height`), empty before `ensure`.
    pub fn data(&self, slot: usize) -> &[u8] {
        match &self.buffers {
            Some((_, g)) if slot < SLOTS => {
                &self.map[slot_range(g.stride as usize, g.height as usize, slot)]
            }
            _ => &[],
        }
    }
}

impl Drop for MemfdPool {
    fn drop(&mut self) {
        if let Some((buffers, _)) = self.buffers.take() {
            for b in buffers {
                b.destroy();
            }
        }
        self.pool.destroy();
    }
}

/// The byte range of `slot` in the pool's mapping.
fn slot_range(stride: usize, height: usize, slot: usize) -> std::ops::Range<usize> {
    let one = stride * height;
    slot * one..(slot + 1) * one
}

/// Which of the two slots is delivered and which is requested next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotState {
    delivered: Option<usize>,
}

impl SlotState {
    /// The slot to request into: never the delivered one.
    pub fn next_request(&self) -> usize {
        self.delivered.map_or(0, |d| (d + 1) % SLOTS)
    }

    /// Marks `slot` as the delivered frame.
    pub fn delivered(&mut self, slot: usize) {
        self.delivered = Some(slot % SLOTS);
    }

    /// The slot a view should point at, if a frame was delivered.
    pub fn view_slot(&self) -> Option<usize> {
        self.delivered
    }

    /// Forgets the delivered frame (its bytes are no longer valid).
    pub fn reset(&mut self) {
        self.delivered = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_alternate_and_view_points_at_delivered_slot() {
        let mut s = SlotState::default();
        assert_eq!(s.view_slot(), None);
        assert_eq!(s.next_request(), 0);
        s.delivered(0);
        assert_eq!(s.next_request(), 1);
        assert_eq!(s.view_slot(), Some(0));
        s.delivered(1);
        assert_eq!(s.next_request(), 0);
        assert_eq!(s.view_slot(), Some(1));
    }

    #[test]
    fn pool_slots_do_not_overlap() {
        let (stride, h) = (16usize, 3usize);
        let (a, b) = (slot_range(stride, h, 0), slot_range(stride, h, 1));
        assert_eq!(a.len(), stride * h);
        assert_eq!(b.len(), stride * h);
        assert!(a.end <= b.start);
    }
}

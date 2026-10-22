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

/// One pool, one buffer.
pub struct MemfdPool {
    fd: OwnedFd,
    map: MmapMut,
    size: usize,
    pool: wl_shm_pool::WlShmPool,
    buffer: Option<(wl_buffer::WlBuffer, BufferGeometry)>,
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
            buffer: None,
        })
    }

    /// Returns a `wl_buffer` for `geometry`, reusing the current one when
    /// it matches and growing the pool when needed.
    pub fn ensure<S>(
        &mut self,
        qh: &QueueHandle<S>,
        geometry: BufferGeometry,
    ) -> Result<&wl_buffer::WlBuffer, ShmError>
    where
        S: Dispatch<wl_buffer::WlBuffer, ()> + 'static,
    {
        if geometry.width == 0
            || geometry.height == 0
            || geometry.stride < geometry.width.saturating_mul(4)
            || i32::try_from(geometry.stride).is_err()
            || i32::try_from(geometry.height).is_err()
        {
            return Err(ShmError::Geometry(
                geometry.width,
                geometry.height,
                geometry.stride,
            ));
        }
        if let Some((_, current)) = &self.buffer
            && *current == geometry
        {
            return Ok(&self.buffer.as_ref().expect("checked").0);
        }
        let needed = geometry.stride as usize * geometry.height as usize;
        if needed > self.size {
            nix::unistd::ftruncate(&self.fd, needed as i64)?;
            // SAFETY: as in `new`; the old mapping is replaced before use.
            self.map = unsafe { MmapMut::map_mut(&self.fd)? };
            self.pool.resize(needed as i32);
            self.size = needed;
        }
        if let Some((old, _)) = self.buffer.take() {
            old.destroy();
        }
        let format = wl_shm::Format::try_from(geometry.format).unwrap_or(wl_shm::Format::Xrgb8888);
        let buffer = self.pool.create_buffer(
            0,
            geometry.width as i32,
            geometry.height as i32,
            geometry.stride as i32,
            format,
            qh,
            (),
        );
        self.buffer = Some((buffer, geometry));
        Ok(&self.buffer.as_ref().expect("just set").0)
    }

    /// The current buffer's geometry.
    pub fn geometry(&self) -> Option<BufferGeometry> {
        self.buffer.as_ref().map(|(_, g)| *g)
    }

    /// The current buffer's bytes (`stride * height`).
    pub fn data(&self) -> &[u8] {
        match &self.buffer {
            Some((_, g)) => &self.map[..(g.stride as usize * g.height as usize).min(self.size)],
            None => &[],
        }
    }
}

impl Drop for MemfdPool {
    fn drop(&mut self) {
        if let Some((buffer, _)) = self.buffer.take() {
            buffer.destroy();
        }
        self.pool.destroy();
    }
}

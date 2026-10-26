//! Dmabuf capture buffers: the images a caller hands in, the session's
//! dmabuf constraints, and the pure accumulator behind them.

use std::os::fd::OwnedFd;

/// `DRM_FORMAT_XRGB8888` (`XR24`): bytes B, G, R, X in memory, the same
/// layout as [`crate::frame::PixelLayout::Bgrx`].
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
/// `DRM_FORMAT_ARGB8888` (`AR24`): bytes B, G, R, A in memory.
pub const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;

/// One plane of a dmabuf image.
#[derive(Debug)]
pub struct DmabufPlane {
    /// The dmabuf.
    pub fd: OwnedFd,
    /// Byte offset of the plane in the dmabuf.
    pub offset: u32,
    /// Bytes per row.
    pub stride: u32,
}

/// A dmabuf image the compositor copies frames into.
#[derive(Debug)]
pub struct DmabufImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// DRM fourcc.
    pub fourcc: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Planes, in plane-index order.
    pub planes: Vec<DmabufPlane>,
}

/// What a capture session accepts for dmabuf buffers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DmabufConstraints {
    /// The `dev_t` buffers must be allocated on (0 when not announced).
    pub device: u64,
    /// Each offered fourcc with its modifiers.
    pub formats: Vec<(u32, Vec<u64>)>,
}

/// Whether `c` accepts `fourcc` with `modifier`.
pub fn offers(c: &DmabufConstraints, fourcc: u32, modifier: u64) -> bool {
    c.formats
        .iter()
        .any(|(f, mods)| *f == fourcc && mods.contains(&modifier))
}

/// Collects one constraints batch from `dmabuf_device`/`dmabuf_format`
/// events; `done` publishes it (a batch without formats withdraws dmabuf).
#[derive(Debug, Default)]
pub(crate) struct ConstraintsAccumulator {
    pending_device: Option<u64>,
    pending_formats: Vec<(u32, Vec<u64>)>,
    current: Option<DmabufConstraints>,
}

impl ConstraintsAccumulator {
    /// A `dmabuf_device` event: a native-endian `dev_t`.
    pub(crate) fn device(&mut self, bytes: &[u8]) {
        self.pending_device = match bytes.len() {
            8 => Some(u64::from_ne_bytes(bytes.try_into().expect("8 bytes"))),
            4 => Some(u64::from(u32::from_ne_bytes(
                bytes.try_into().expect("4 bytes"),
            ))),
            n => {
                tracing::warn!(len = n, "ignoring a dmabuf_device of unexpected size");
                None
            }
        };
    }

    /// A `dmabuf_format` event: `modifiers` is an array of native-endian u64.
    pub(crate) fn format(&mut self, fourcc: u32, modifiers: &[u8]) {
        let mods: Vec<u64> = modifiers
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_ne_bytes(*c))
            .collect();
        match self.pending_formats.iter_mut().find(|(f, _)| *f == fourcc) {
            Some((_, existing)) => {
                for m in mods {
                    if !existing.contains(&m) {
                        existing.push(m);
                    }
                }
            }
            None => self.pending_formats.push((fourcc, mods)),
        }
    }

    /// The batch's `done`.
    pub(crate) fn done(&mut self) {
        let device = self.pending_device.take();
        let formats = std::mem::take(&mut self.pending_formats);
        self.current = (!formats.is_empty()).then(|| DmabufConstraints {
            device: device.unwrap_or(0),
            formats,
        });
    }

    /// The last completed batch's constraints.
    pub(crate) fn constraints(&self) -> Option<DmabufConstraints> {
        self.current.clone()
    }
}

/// Round-robin over `len` images that never hands out the delivered one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImageRing {
    len: usize,
    delivered: Option<usize>,
}

impl ImageRing {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            len,
            delivered: None,
        }
    }

    /// The image to attach next: the one after the delivered image.
    pub(crate) fn next_request(&self) -> usize {
        self.delivered.map_or(0, |d| (d + 1) % self.len.max(1))
    }

    pub(crate) fn delivered(&mut self, index: usize) {
        self.delivered = Some(index);
    }

    pub(crate) fn view(&self) -> Option<usize> {
        self.delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constraints() -> DmabufConstraints {
        DmabufConstraints {
            device: 0xe280,
            formats: vec![
                (DRM_FORMAT_XRGB8888, vec![0, 0x0100_0000_0000_0001]),
                (DRM_FORMAT_ARGB8888, vec![0]),
            ],
        }
    }

    #[test]
    fn offers_matches_format_and_modifier() {
        let c = constraints();
        assert!(offers(&c, DRM_FORMAT_XRGB8888, 0));
        assert!(offers(&c, DRM_FORMAT_XRGB8888, 0x0100_0000_0000_0001));
        assert!(offers(&c, DRM_FORMAT_ARGB8888, 0));
    }

    #[test]
    fn offers_rejects_unlisted_modifier() {
        let c = constraints();
        assert!(!offers(&c, DRM_FORMAT_ARGB8888, 0x0100_0000_0000_0001));
        assert!(!offers(&c, DRM_FORMAT_XRGB8888, 7));
        assert!(!offers(&c, 0x3231_564e, 0));
    }

    #[test]
    fn ring_never_requests_the_delivered_image() {
        let mut r = ImageRing::new(3);
        assert_eq!((r.view(), r.next_request()), (None, 0));
        for expect in [1, 2, 0, 1] {
            let req = r.next_request();
            r.delivered(req);
            assert_eq!(r.next_request(), expect);
            assert_ne!(r.next_request(), r.view().unwrap());
        }
    }

    #[test]
    fn constraints_collect_device_and_formats() {
        let mut acc = ConstraintsAccumulator::default();
        assert_eq!(acc.constraints(), None);
        let dev: libc::dev_t = 0xe280;
        acc.device(&dev.to_ne_bytes());
        let mods: Vec<u8> = [0u64, 0x0100_0000_0000_0001]
            .iter()
            .flat_map(|m| m.to_ne_bytes())
            .collect();
        acc.format(DRM_FORMAT_XRGB8888, &mods);
        acc.format(DRM_FORMAT_ARGB8888, &0u64.to_ne_bytes());
        // Nothing is visible until the batch is done.
        assert_eq!(acc.constraints(), None);
        acc.done();
        assert_eq!(acc.constraints(), Some(constraints()));
        // A later batch without dmabuf events withdraws the offer.
        acc.done();
        assert_eq!(acc.constraints(), None);
    }
}

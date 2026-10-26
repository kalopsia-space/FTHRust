//! Whether a capture can hand the compositor the encoder's own surfaces
//! (dmabuf zero copy) instead of copying frames through shared memory.

use fthrust_media::vaapi::{ExportedImage, same_gpu};
use fthrust_wayland::dmabuf::{DmabufConstraints, DmabufImage, DmabufPlane, offers};

/// The decision for one capture generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroCopy {
    /// Capture into the encoder's surfaces.
    Use,
    /// Copy frames through shm, for this reason.
    Off(&'static str),
}

/// Decides zero copy. `encoder_gpu` is the `dev_t` of the encoder's render
/// node; `image` is the fourcc and modifier the encoder's surfaces export.
pub fn zero_copy_plan(
    constraints: Option<&DmabufConstraints>,
    encoder_gpu: Option<u64>,
    gpu_conversion: bool,
    image: Option<(u32, u64)>,
    env_disabled: bool,
) -> ZeroCopy {
    if env_disabled {
        return ZeroCopy::Off("disabled by the environment");
    }
    if !gpu_conversion {
        return ZeroCopy::Off("the encoder does not convert colour on the GPU");
    }
    let Some(c) = constraints else {
        return ZeroCopy::Off("the compositor offers no dmabuf capture");
    };
    let same = encoder_gpu.is_some_and(|gpu| c.device != 0 && same_gpu(c.device, gpu));
    if !same {
        return ZeroCopy::Off("the compositor's dmabuf device is not the encoder's GPU");
    }
    match image {
        Some((fourcc, modifier)) if offers(c, fourcc, modifier) => ZeroCopy::Use,
        _ => ZeroCopy::Off("the compositor does not accept the surface format"),
    }
}

/// `FTHR_NO_ZERO_COPY=1` or `FTHR_CPU_COLOR_CONVERT=1`.
pub fn env_disabled() -> bool {
    ["FTHR_NO_ZERO_COPY", "FTHR_CPU_COLOR_CONVERT"]
        .iter()
        .any(|k| std::env::var(k).as_deref() == Ok("1"))
}

/// The Wayland-side description of an exported encoder surface.
pub fn to_dmabuf_image(image: ExportedImage) -> DmabufImage {
    DmabufImage {
        width: image.width,
        height: image.height,
        fourcc: image.fourcc,
        modifier: image.modifier,
        planes: image
            .planes
            .into_iter()
            .map(|(fd, offset, stride)| DmabufPlane { fd, offset, stride })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fthrust_wayland::dmabuf::DRM_FORMAT_XRGB8888;

    const MODIFIER: u64 = 0x0200_0000_1234_5678;

    fn constraints(device: u64) -> DmabufConstraints {
        DmabufConstraints {
            device,
            formats: vec![(DRM_FORMAT_XRGB8888, vec![MODIFIER])],
        }
    }

    fn plan(c: Option<&DmabufConstraints>, gpu: Option<u64>) -> ZeroCopy {
        zero_copy_plan(c, gpu, true, Some((DRM_FORMAT_XRGB8888, MODIFIER)), false)
    }

    #[test]
    fn zero_copy_off_without_constraints() {
        assert!(matches!(plan(None, Some(7)), ZeroCopy::Off(_)));
    }

    #[test]
    fn zero_copy_off_for_other_gpu() {
        // Device numbers with no sysfs entry compare by value only.
        let c = constraints(0xdead_0001);
        assert!(matches!(
            plan(Some(&c), Some(0xdead_0002)),
            ZeroCopy::Off(_)
        ));
        assert!(matches!(plan(Some(&c), None), ZeroCopy::Off(_)));
        let unannounced = constraints(0);
        assert!(matches!(
            plan(Some(&unannounced), Some(0)),
            ZeroCopy::Off(_)
        ));
    }

    #[test]
    fn zero_copy_off_when_modifier_not_offered() {
        let c = constraints(0xdead_0001);
        let other = zero_copy_plan(
            Some(&c),
            Some(0xdead_0001),
            true,
            Some((DRM_FORMAT_XRGB8888, MODIFIER + 1)),
            false,
        );
        assert!(matches!(other, ZeroCopy::Off(_)));
        let none = zero_copy_plan(Some(&c), Some(0xdead_0001), true, None, false);
        assert!(matches!(none, ZeroCopy::Off(_)));
    }

    #[test]
    fn zero_copy_off_without_gpu_conversion() {
        let c = constraints(0xdead_0001);
        let p = zero_copy_plan(
            Some(&c),
            Some(0xdead_0001),
            false,
            Some((DRM_FORMAT_XRGB8888, MODIFIER)),
            false,
        );
        assert!(matches!(p, ZeroCopy::Off(_)));
    }

    #[test]
    fn zero_copy_off_when_env_disabled() {
        let c = constraints(0xdead_0001);
        let p = zero_copy_plan(
            Some(&c),
            Some(0xdead_0001),
            true,
            Some((DRM_FORMAT_XRGB8888, MODIFIER)),
            true,
        );
        assert!(matches!(p, ZeroCopy::Off(_)));
    }

    #[test]
    fn zero_copy_used_when_everything_matches() {
        let c = constraints(0xdead_0001);
        assert_eq!(plan(Some(&c), Some(0xdead_0001)), ZeroCopy::Use);
    }
}

//! Live check: the compositor's ext-image-copy session advertises dmabuf
//! constraints. Run inside a headless compositor (see tools/uidrive), never
//! against a desktop session.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use fthrust_wayland::dmabuf::{DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888, offers};
use fthrust_wayland::ext_capture::ExtImageCopy;

#[test]
#[ignore = "needs a headless ext-image-copy compositor with a GPU renderer"]
fn ext_capture_offers_dmabuf_constraints() {
    let cancel = Arc::new(AtomicBool::new(false));
    let backend = ExtImageCopy::open("", false, 30, cancel).expect("open ext-image-copy");
    let c = backend
        .dmabuf_constraints()
        .expect("the session offers dmabuf constraints");
    eprintln!(
        "dmabuf device {:#x}, {} formats: {:?}",
        c.device,
        c.formats.len(),
        c.formats
            .iter()
            .map(|(f, m)| (format!("{f:#010x}"), m.len()))
            .collect::<Vec<_>>()
    );
    assert!(!c.formats.is_empty());
    assert!(c.formats.iter().all(|(_, mods)| !mods.is_empty()));
    let linear_bgrx = offers(&c, DRM_FORMAT_XRGB8888, 0) || offers(&c, DRM_FORMAT_ARGB8888, 0);
    eprintln!("linear XRGB/ARGB8888 offered: {linear_bgrx}");
}

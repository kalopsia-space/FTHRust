//! Camera discovery (`CameraRecorder.list_devices`): V4L2 capture nodes
//! with their driver names, instead of probing OpenCV indices 0–7.

use std::path::{Path, PathBuf};

/// One camera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// `/dev/videoN`.
    pub path: PathBuf,
    /// The V4L2 card name, else `Camera N`.
    pub name: String,
}

/// Every V4L2 capture device, in device-node order.
pub fn list_devices() -> Vec<Device> {
    if crate::init().is_err() {
        return Vec::new();
    }
    devices_from(crate::ffi::device::v4l2_sources(), Path::new("/sys"))
}

/// Keeps primary nodes only (sysfs `index` 0: a UVC camera's second node
/// is its metadata stream, which libavdevice still lists), sorts by node
/// and names unnamed ones `Camera N`.
pub fn devices_from(mut found: Vec<(PathBuf, String)>, sysfs: &Path) -> Vec<Device> {
    found.retain(|(p, _)| {
        let Some(node) = p.file_name() else {
            return true;
        };
        let index = sysfs.join("class/video4linux").join(node).join("index");
        std::fs::read_to_string(index).map_or(true, |i| i.trim() == "0")
    });
    found.sort_by_key(|(p, _)| node_number(p));
    found
        .into_iter()
        .enumerate()
        .map(|(i, (path, desc))| Device {
            name: if desc.trim().is_empty() {
                format!("Camera {}", i + 1)
            } else {
                desc
            },
            path,
        })
        .collect()
}

fn node_number(p: &std::path::Path) -> u32 {
    p.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("video"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

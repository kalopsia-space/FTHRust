//! `avdevice_list_input_sources` for the V4L2 input.

use std::ffi::CStr;
use std::path::PathBuf;

use ffmpeg_next as ff;

/// `(device path, description)` of every V4L2 capture node libavdevice
/// lists (non-capture nodes are skipped by libavdevice itself).
pub fn v4l2_sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    // SAFETY: the format pointer comes from libavdevice's registry; the list
    // is owned here and freed once; every string is read before the free.
    unsafe {
        let fmt = ff::ffi::av_find_input_format(c"v4l2".as_ptr());
        if fmt.is_null() {
            return out;
        }
        let mut list: *mut ff::ffi::AVDeviceInfoList = std::ptr::null_mut();
        let n = ff::ffi::avdevice_list_input_sources(
            fmt,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut list,
        );
        if n > 0 && !list.is_null() {
            for i in 0..(*list).nb_devices.max(0) as usize {
                let d = *(*list).devices.add(i);
                if d.is_null() || (*d).device_name.is_null() {
                    continue;
                }
                let name = CStr::from_ptr((*d).device_name)
                    .to_string_lossy()
                    .into_owned();
                let desc = if (*d).device_description.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr((*d).device_description)
                        .to_string_lossy()
                        .into_owned()
                };
                out.push((PathBuf::from(name), desc));
            }
        }
        if !list.is_null() {
            ff::ffi::avdevice_free_list_devices(&mut list);
        }
    }
    out
}

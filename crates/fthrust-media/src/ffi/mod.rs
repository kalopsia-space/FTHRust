//! The only module allowed to use `unsafe`: thin wrappers over FFmpeg APIs
//! that `ffmpeg-next` does not expose safely (hardware device and frame
//! contexts). Every `unsafe` block carries a `SAFETY` comment.
#![allow(unsafe_code)]

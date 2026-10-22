//! Wayland capture for FTHRust: frame model and normalisation, bounded
//! dispatch, output tracking, shm pools and the protocol backends.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod backend;
pub mod connection;
pub mod dispatch;
pub mod ext_capture;
pub mod frame;
pub mod outputs;
pub mod screencopy;
pub mod shm;

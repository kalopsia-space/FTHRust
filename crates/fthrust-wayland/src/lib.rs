//! Wayland capture for FTHRust: frame model and normalisation, bounded
//! dispatch, output tracking, shm pools and the protocol backends.
#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod connection;
pub mod dispatch;
pub mod frame;
pub mod outputs;
pub mod shm;

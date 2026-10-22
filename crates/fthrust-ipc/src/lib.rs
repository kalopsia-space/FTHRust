//! Engine <-> UI IPC for FTHRust: message types, length-prefixed JSON
//! framing and the Unix-socket transport.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// Wire protocol version carried in `Hello`.
pub const PROTOCOL_VERSION: u32 = 1;

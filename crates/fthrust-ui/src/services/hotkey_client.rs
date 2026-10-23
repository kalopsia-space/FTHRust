//! `fthrust --hotkey <action>`: the compositor bind client that writes an
//! action to the running instance's hotkey socket.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Sends `action`; returns the process exit code (0 sent, 1 not running).
pub fn send(runtime_dir: &Path, action: &str) -> i32 {
    let sock = runtime_dir.join("hotkey.sock");
    match UnixStream::connect(&sock) {
        Ok(mut s) => {
            let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
            if s.write_all(action.as_bytes()).is_ok() && s.flush().is_ok() {
                0
            } else {
                eprintln!("fthrust: could not deliver the hotkey");
                1
            }
        }
        Err(_) => {
            eprintln!("FTHR Clips is not running");
            1
        }
    }
}

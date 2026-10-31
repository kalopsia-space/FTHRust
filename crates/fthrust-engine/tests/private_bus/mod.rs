//! A `dbus-daemon` on a socket in a temporary directory, with no service
//! directories: nothing can be activated on it and nothing reaches the
//! user's session bus.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct PrivateBus {
    child: Child,
    pub address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    /// Starts the daemon and points `DBUS_SESSION_BUS_ADDRESS` at it, or
    /// `None` without `dbus-daemon`. Call it first thing in a test binary
    /// that holds one test.
    pub fn start() -> Option<Self> {
        let dir = tempfile::tempdir().ok()?;
        let socket: PathBuf = dir.path().join("bus");
        let config = dir.path().join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
                socket.display()
            ),
        )
        .ok()?;
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut bus = Self {
            child,
            address: format!("unix:path={}", socket.display()),
            _dir: dir,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            if Instant::now() > deadline || bus.child.try_wait().ok().flatten().is_some() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: called first in a binary with one test, before anything in
        // the process connects to D-Bus or reads the environment.
        unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address) };
        Some(bus)
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

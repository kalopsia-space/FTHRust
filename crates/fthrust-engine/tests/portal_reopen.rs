//! The portal backend must open again inside one engine process (recovery
//! and reconfigure do that), and a portal that is missing or failing must
//! leave the restore token alone. Runs against a private D-Bus daemon with
//! no services on it, so nothing reaches the user's session bus.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use fthrust_core::FthrPaths;
use fthrust_core::capture_config::CaptureConfig;
use fthrust_engine::backend::{BackendFactory, RealFactory};
use fthrust_ipc::BackendKind;

/// A `dbus-daemon` on a socket in a temporary directory, with no service
/// directories: nothing can be activated on it.
struct PrivateBus {
    child: Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    fn start() -> Option<Self> {
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
        Some(bus)
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn portal_opens_again_in_one_process_and_keeps_the_token() {
    let Some(bus) = PrivateBus::start() else {
        return eprintln!("skipping: dbus-daemon is not available");
    };
    // SAFETY: this binary holds this one test, which sets the variable
    // before anything in the process connects to D-Bus or reads it.
    unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address) };
    let home = tempfile::tempdir().unwrap();
    let paths = FthrPaths::new(home.path());
    let token_path = fthrust_portal::token::path(&paths);
    fthrust_portal::token::save(&token_path, "restore-me").unwrap();
    let factory = RealFactory::new(Some(paths));
    let cfg = CaptureConfig::default();
    let mut reasons = Vec::new();
    for attempt in 0..2 {
        let started = Instant::now();
        let failure = match factory.create(
            BackendKind::PortalScreenCast,
            &cfg,
            false,
            Arc::new(AtomicBool::new(false)),
        ) {
            Ok(_) => panic!("no portal runs on the private bus"),
            Err(f) => f,
        };
        let took = started.elapsed();
        eprintln!("open {attempt} after {took:?}: {}", failure.reason);
        assert!(
            took < Duration::from_secs(5),
            "open {attempt} took {took:?}: {}",
            failure.reason
        );
        assert!(!failure.retry_pointless, "{failure:?}");
        reasons.push(failure.reason);
    }
    assert_eq!(
        reasons[0], reasons[1],
        "the second open must fail the way the first did"
    );
    assert_eq!(
        fthrust_portal::token::load(&token_path).unwrap().as_deref(),
        Some("restore-me"),
        "a portal that is not there must not cost the restore token"
    );
}

//! The portal backend must open again inside one engine process (recovery
//! and reconfigure do that), and a portal that is missing or failing must
//! leave the restore token alone. Runs against a private D-Bus daemon with
//! no services on it, so nothing reaches the user's session bus.

mod private_bus;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use fthrust_core::FthrPaths;
use fthrust_core::capture_config::CaptureConfig;
use fthrust_engine::backend::{BackendFactory, RealFactory};
use fthrust_ipc::BackendKind;

#[test]
fn portal_opens_again_in_one_process_and_keeps_the_token() {
    let Some(_bus) = private_bus::PrivateBus::start() else {
        return eprintln!("skipping: dbus-daemon is not available");
    };
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

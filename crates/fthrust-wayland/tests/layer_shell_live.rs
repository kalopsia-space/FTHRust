//! Plan 5 Task 4: live checks on this machine (ignored in CI).

use std::time::{Duration, Instant};

#[test]
#[ignore = "needs a running Wayland compositor with wlr-layer-shell"]
fn layer_shell_live_creates_and_configures() {
    let deadline = Instant::now() + Duration::from_secs(2);
    let keep = || true;
    let conn = fthrust_wayland::connection::WaylandConnection::connect(deadline, &keep).unwrap();
    assert!(fthrust_wayland::layer_shell::available());
    let best = conn
        .outputs()
        .outputs()
        .into_iter()
        .max_by_key(|o| o.refresh_mhz)
        .map(|o| o.name.clone())
        .unwrap();
    let started = Instant::now();
    let surface = fthrust_wayland::layer_shell::LayerSurface::create(
        Some(&best),
        (340, 116),
        (16, 16, 0, 0),
        "fthrust-capture-card",
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "configured within 1 s"
    );
    assert_eq!(surface.size(), (340, 116));
    let _handles = surface.handles();
    drop(conn);
}

#[test]
#[ignore = "needs a running Wayland compositor with wlr-foreign-toplevel-management"]
fn toplevel_watcher_live_lists_windows() {
    let w = fthrust_wayland::toplevels::ToplevelWatcher::connect().unwrap();
    assert!(w.available());
    std::thread::sleep(Duration::from_millis(300));
    let snap = w.snapshot();
    eprintln!("{snap:?}");
    assert!(!snap.is_empty());
    w.close();
}

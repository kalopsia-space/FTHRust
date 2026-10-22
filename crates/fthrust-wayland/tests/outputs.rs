use std::time::{Duration, Instant};

use fthrust_wayland::connection::{ConnectError, WaylandConnection};

#[test]
fn lists_outputs_with_names() {
    let conn = match WaylandConnection::connect(Instant::now() + Duration::from_secs(5), &|| true) {
        Ok(c) => c,
        Err(ConnectError::NoDisplay) => {
            eprintln!("skipping: no WAYLAND_DISPLAY");
            return;
        }
        Err(e) => panic!("connect: {e}"),
    };
    let outputs = conn.outputs().outputs();
    assert!(!outputs.is_empty());
    for o in &outputs {
        assert!(o.done, "{o:?}");
        assert!(!o.name.is_empty(), "wl_output v4 name: {o:?}");
        assert!(o.width > 0 && o.height > 0, "{o:?}");
    }
    let (_, first) = conn.outputs().select("").unwrap();
    assert_eq!(first, outputs[0]);
    assert!(conn.outputs().select("no-such-output-xyz").is_none());
    assert!(conn.core().global("wl_shm").is_some());
    assert!(
        !conn.core().shm_formats.is_empty(),
        "wl_shm announced formats"
    );
    assert!(conn.core().global("wl_compositor").is_some());
}

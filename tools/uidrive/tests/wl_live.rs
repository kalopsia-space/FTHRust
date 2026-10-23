//! Live check against a headless cage: set `UIDRIVE_LIVE=1` to run it.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uidrive::bench::Bench;
use uidrive::keys::parse_chord;
use uidrive::wl::{Button, Session};

#[test]
fn live_session_moves_and_types() {
    if std::env::var("UIDRIVE_LIVE").as_deref() != Ok("1") {
        eprintln!("UIDRIVE_LIVE != 1; skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("rt");
    std::fs::create_dir_all(&rt).unwrap();
    std::fs::set_permissions(&rt, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let mut cage = Command::new("cage")
        .args(["--", "sleep", "30"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("XDG_RUNTIME_DIR", &rt)
        .env("WLR_BACKENDS", "headless")
        .env("WLR_RENDERER", "pixman")
        .env("WLR_LIBINPUT_NO_DEVICES", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cage installed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rt.join("wayland-0").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let bench = Bench {
        dir: tmp.path().to_path_buf(),
        socket: "wayland-0".into(),
        runtime_dir: rt.clone(),
        dump: tmp.path().join("ui.json"),
        shots: tmp.path().join("shots"),
        logs: tmp.path().join("logs"),
        app: None,
    };
    let result = (|| -> Result<(), String> {
        let mut s = Session::connect(&bench)?;
        let (w, h) = s.extent();
        assert!(w > 0 && h > 0, "extent {w}x{h}");
        assert!(!s.output_name().is_empty());
        s.move_to(10.0, 10.0)?;
        s.click(Button::Left, false)?;
        s.drag((10.0, 10.0), (50.0, 40.0), 4)?;
        s.key(&parse_chord("Escape")?)?;
        s.key(&parse_chord("Ctrl+Shift+S")?)?;
        s.type_text("ab !")?;
        assert!(
            s.type_text("é").is_err(),
            "unsupported characters are refused"
        );
        let shot = uidrive::wl::shot(&bench, "live")?;
        assert!(shot.exists(), "{shot:?}");
        assert!(std::fs::metadata(&shot).unwrap().len() > 100);
        Ok(())
    })();
    let _ = cage.kill();
    let _ = cage.wait();
    result.unwrap();
}

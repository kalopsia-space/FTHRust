use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use fthrust_core::capture_config::{CaptureConfig, Codec, EncoderPref};
use fthrust_core::capture_health::flags;
use fthrust_ipc::transport::EngineClient;
use fthrust_ipc::{BackendKind, Command, EngineConfig, Event};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command as Proc};

fn have_software_h264() -> bool {
    fthrust_media::init().unwrap();
    ["libopenh264", "libx264"]
        .iter()
        .any(|n| ffmpeg_next::encoder::find_by_name(n).is_some())
}

struct Daemon {
    child: Child,
    socket: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn spawn(extra_env: &[(&str, &str)], remove_wayland: bool) -> Daemon {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("run");
    std::fs::create_dir_all(&runtime).unwrap();
    let socket = runtime.join("engine.sock");
    let mut cmd = Proc::new(env!("CARGO_BIN_EXE_fthrust-engine"));
    cmd.arg("--socket")
        .arg(&socket)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("HOME", tmp.path())
        .env("FTHRUST_SYNTHETIC_BACKEND", "1")
        .env("FTHR_CAPTURE_BACKEND", "synthetic")
        .env("FTHRUST_SYNTHETIC_SIZE", "320x180")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    if remove_wayland {
        cmd.env_remove("WAYLAND_DISPLAY");
    }
    let mut child = cmd.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if line.starts_with("FTHRUST_ENGINE_READY") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(ready, "daemon did not print its ready line");
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Daemon {
        child,
        socket,
        _tmp: tmp,
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        capture: CaptureConfig {
            fps: 30,
            buffer_seconds: 10,
            bitrate_kbps: 2000,
            codec: Codec::H264,
            encoder: EncoderPref::Software,
            preset: 1,
            audio_enabled: false,
            ..CaptureConfig::default()
        },
        max_buffer_mb: 0,
        backend_override: Some(BackendKind::Synthetic),
    }
}

async fn next_event(
    client: &mut EngineClient,
    timeout: Duration,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, client.events().recv()).await {
            Ok(Some(e)) if pred(&e) => return e,
            Ok(Some(_)) => {}
            Ok(None) => panic!("engine hung up"),
            Err(_) => panic!("no matching event within {timeout:?}"),
        }
    }
}

async fn wait_exit(child: &mut Child, timeout: Duration) -> i32 {
    let status = tokio::time::timeout(timeout, child.wait())
        .await
        .expect("exit in time")
        .unwrap();
    status.code().unwrap_or(-1)
}

async fn connect(socket: &Path) -> EngineClient {
    let started = Instant::now();
    loop {
        match EngineClient::connect(socket).await {
            Ok(c) => return c,
            Err(e) if started.elapsed() < Duration::from_secs(5) => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("connect: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_configure_save_record_shutdown() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut d = spawn(&[], false).await;
    let mut client = connect(&d.socket).await;
    assert_eq!(client.hello().pid, d.child.id().unwrap());
    client.send(Command::Configure(config())).await.unwrap();
    next_event(&mut client, Duration::from_secs(10), |e| {
        matches!(e, Event::Configured { .. })
    })
    .await;
    next_event(&mut client, Duration::from_secs(5), |e| {
        matches!(e, Event::Status(s) if s.health_flags == flags::ACTIVE && s.backend == Some(BackendKind::Synthetic))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let clip = d._tmp.path().join("clip.mp4");
    client
        .send(Command::SaveClip {
            request_id: 1,
            path: clip.clone(),
            seconds: 2,
        })
        .await
        .unwrap();
    next_event(&mut client, Duration::from_secs(5), |e| {
        matches!(e, Event::SaveStarted { request_id: 1 })
    })
    .await;
    let e = next_event(&mut client, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 1, .. })
    })
    .await;
    let Event::SaveResult {
        result: Ok(saved), ..
    } = e
    else {
        panic!("{e:?}")
    };
    assert_eq!(saved.path, clip);
    let d_s = fthrust_media::probe::probe_video_metadata(&clip)
        .unwrap()
        .duration_seconds
        .unwrap();
    assert!((d_s - 2.0).abs() < 0.2, "duration {d_s}");
    let rec = d._tmp.path().join("rec.mp4");
    client
        .send(Command::StartRecording {
            request_id: 2,
            path: rec.clone(),
        })
        .await
        .unwrap();
    next_event(&mut client, Duration::from_secs(5), |e| {
        matches!(e, Event::RecordingStarted { request_id: 2, .. })
    })
    .await;
    next_event(&mut client, Duration::from_secs(5), |e| {
        matches!(e, Event::Status(s) if matches!(s.recording, fthrust_ipc::RecordingState::Recording { .. }))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    client
        .send(Command::StopRecording { request_id: 3 })
        .await
        .unwrap();
    let e = next_event(&mut client, Duration::from_secs(10), |e| {
        matches!(
            e,
            Event::RecordingStopped { request_id: 3, .. } | Event::RecordingError { .. }
        )
    })
    .await;
    assert!(matches!(e, Event::RecordingStopped { .. }), "{e:?}");
    assert!(rec.exists());
    client.send(Command::Shutdown).await.unwrap();
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(3)).await, 0);
    assert!(!d.socket.exists(), "socket removed on exit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_reconnect_keeps_last_save() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut d = spawn(&[], false).await;
    let mut client = connect(&d.socket).await;
    client.send(Command::Configure(config())).await.unwrap();
    next_event(
        &mut client,
        Duration::from_secs(5),
        |e| matches!(e, Event::Status(s) if s.health_flags == flags::ACTIVE),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let clip = d._tmp.path().join("clip.mp4");
    client
        .send(Command::SaveClip {
            request_id: 5,
            path: clip,
            seconds: 1,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(client);
    let mut again = connect(&d.socket).await;
    let started = Instant::now();
    loop {
        again.send(Command::GetStatus).await.unwrap();
        let e = next_event(&mut again, Duration::from_secs(5), |e| {
            matches!(e, Event::Status(_))
        })
        .await;
        let Event::Status(s) = e else { unreachable!() };
        if let Some((id, result)) = &s.last_save {
            assert_eq!(*id, 5);
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(
                s.capture_generation, 1,
                "capture was not restarted by the reconnect"
            );
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "last_save never appeared"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    again.send(Command::Shutdown).await.unwrap();
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(3)).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_eof_and_sigterm_exit_cleanly() {
    let mut d = spawn(&[], false).await;
    drop(d.child.stdin.take());
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(2)).await, 0);
    let mut d = spawn(&[], false).await;
    let pid = d.child.id().unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(3)).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_error_is_coded() {
    let mut d = spawn(&[("FTHR_CAPTURE_BACKEND", "wlr")], true).await;
    let stderr = d.child.stderr.take().unwrap();
    let mut client = connect(&d.socket).await;
    let mut cfg = config();
    cfg.backend_override = None;
    client.send(Command::Configure(cfg)).await.unwrap();
    let e = next_event(&mut client, Duration::from_secs(20), |e| {
        matches!(e, Event::StartupError { .. })
    })
    .await;
    let Event::StartupError { code, .. } = e else {
        unreachable!()
    };
    assert_eq!(code, "WAYLAND_UNAVAILABLE");
    let code = wait_exit(&mut d.child, Duration::from_secs(5)).await;
    assert_eq!(code, 1, "exit code {code}");
    let mut text = String::new();
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        text.push_str(&line);
        text.push('\n');
    }
    assert!(
        text.contains("FTHR_STARTUP_ERROR: WAYLAND_UNAVAILABLE:"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_cadence() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut d = spawn(&[], false).await;
    let mut client = connect(&d.socket).await;
    client.send(Command::Configure(config())).await.unwrap();
    next_event(
        &mut client,
        Duration::from_secs(5),
        |e| matches!(e, Event::Status(s) if s.health_flags == flags::ACTIVE),
    )
    .await;
    let started = Instant::now();
    let mut count = 0;
    while started.elapsed() < Duration::from_secs(6) {
        match tokio::time::timeout(Duration::from_secs(2), client.events().recv()).await {
            Ok(Some(Event::Status(_) | Event::Heartbeat)) => count += 1,
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    assert!(count >= 10, "{count} status/heartbeat events in 6 s");
    client.send(Command::Shutdown).await.unwrap();
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(3)).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_a_save_in_flight() {
    if !have_software_h264() {
        return eprintln!("skipping: no software H.264 encoder");
    }
    let mut d = spawn(&[], false).await;
    let mut client = connect(&d.socket).await;
    client.send(Command::Configure(config())).await.unwrap();
    next_event(
        &mut client,
        Duration::from_secs(5),
        |e| matches!(e, Event::Status(s) if s.health_flags == flags::ACTIVE),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let clip = d._tmp.path().join("last.mp4");
    client
        .send(Command::SaveClip {
            request_id: 8,
            path: clip.clone(),
            seconds: 2,
        })
        .await
        .unwrap();
    client.send(Command::Shutdown).await.unwrap();
    let e = next_event(&mut client, Duration::from_secs(20), |e| {
        matches!(e, Event::SaveResult { request_id: 8, .. })
    })
    .await;
    assert!(
        matches!(e, Event::SaveResult { result: Ok(_), .. }),
        "{e:?}"
    );
    assert_eq!(wait_exit(&mut d.child, Duration::from_secs(10)).await, 0);
    assert!(clip.exists());
}

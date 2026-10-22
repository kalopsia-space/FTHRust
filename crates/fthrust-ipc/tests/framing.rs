use fthrust_core::capture_config::CaptureConfig;
use fthrust_ipc::framing::{FrameReader, FrameWriter, decode, encode};
use fthrust_ipc::{
    AudioStatus, BackendKind, Command, EngineConfig, Event, IpcError, MAX_FRAME_BYTES,
    RecordingState, SavedClip, SourceStatus, StatusSnapshot,
};
use std::path::PathBuf;

fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        frames_captured: 1234,
        bytes_written: 99,
        encoder_open: true,
        active_codec: "h264_vaapi".into(),
        health_flags: 1,
        capture_generation: 2,
        encoder_generation: 3,
        content_sample_sequence: 4,
        content_suspicious_streak: 0,
        content_luma_mean: 12.5,
        content_luma_variance: 3.25,
        capture_failure_detail: String::new(),
        recovery_reason: String::new(),
        paused: false,
        recording: RecordingState::Recording {
            path: PathBuf::from("/tmp/r.mp4"),
            started_ns: 77,
        },
        backend: Some(BackendKind::ExtImageCopy),
        selected_output: "DP-1".into(),
        native_width: 2560,
        native_height: 1440,
        audio: AudioStatus {
            system: SourceStatus::Streaming,
            microphone: SourceStatus::Reconnecting {
                reason: "device gone".into(),
            },
        },
        last_save: Some((
            9,
            Ok(SavedClip {
                path: PathBuf::from("/tmp/c.mp4"),
                bytes_written: 5,
                audio_included: true,
                audio_error: None,
                audio_tracks: vec!["Default Mix".into()],
            }),
        )),
    }
}

fn commands() -> Vec<Command> {
    vec![
        Command::Configure(EngineConfig {
            capture: CaptureConfig {
                fps: 60,
                buffer_seconds: 30,
                ..CaptureConfig::default()
            },
            max_buffer_mb: 512,
            backend_override: Some(BackendKind::WlrScreencopy),
        }),
        Command::SaveClip {
            request_id: 1,
            path: PathBuf::from("/tmp/a.mp4"),
            seconds: 30,
        },
        Command::StartRecording {
            request_id: 2,
            path: PathBuf::from("/tmp/b.mp4"),
        },
        Command::StopRecording { request_id: 2 },
        Command::Pause,
        Command::Resume,
        Command::GetStatus,
        Command::Shutdown,
    ]
}

#[test]
fn roundtrip_command_and_event() {
    for c in commands() {
        let bytes = encode(&c).unwrap();
        assert_eq!(decode::<Command>(&bytes[4..]).unwrap(), c);
    }
    let events = vec![
        Event::Hello {
            protocol_version: 1,
            pid: 42,
            version: "2.0.0".into(),
        },
        Event::StartupWarning {
            code: "DESKTOP_AUDIO_UNAVAILABLE".into(),
            detail: "no server".into(),
        },
        Event::Status(Box::new(snapshot())),
        Event::SaveResult {
            request_id: 1,
            result: Err("nope".into()),
        },
        Event::Heartbeat,
    ];
    for e in events {
        let bytes = encode(&e).unwrap();
        assert_eq!(decode::<Event>(&bytes[4..]).unwrap(), e);
    }
}

#[test]
fn length_prefix_is_u32_le() {
    let bytes = encode(&Command::Pause).unwrap();
    let body = &bytes[4..];
    assert_eq!(&bytes[..4], &(body.len() as u32).to_le_bytes());
    assert_eq!(
        body,
        serde_json::to_vec(&Command::Pause).unwrap().as_slice()
    );
}

#[tokio::test]
async fn reader_and_writer_stream_frames() {
    let (client, server) = tokio::io::duplex(1 << 16);
    let mut w = FrameWriter::new(client);
    let mut r = FrameReader::new(server);
    for c in commands() {
        w.write(&c).await.unwrap();
    }
    drop(w);
    for c in commands() {
        assert_eq!(r.read::<Command>().await.unwrap(), Some(c));
    }
    assert_eq!(
        r.read::<Command>().await.unwrap(),
        None,
        "clean EOF is None"
    );
}

#[tokio::test]
async fn oversize_frame_rejected() {
    let header = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes();
    let mut r = FrameReader::new(std::io::Cursor::new(header.to_vec()));
    assert!(
        matches!(r.read::<Command>().await, Err(IpcError::FrameTooLarge(n)) if n == MAX_FRAME_BYTES + 1)
    );
}

#[tokio::test]
async fn truncated_stream_is_error() {
    let mut bytes = encode(&Command::Shutdown).unwrap();
    bytes.truncate(bytes.len() - 3);
    let mut r = FrameReader::new(std::io::Cursor::new(bytes));
    assert!(matches!(
        r.read::<Command>().await,
        Err(IpcError::Truncated)
    ));
    let mut r = FrameReader::new(std::io::Cursor::new(vec![1u8, 0]));
    assert!(
        matches!(r.read::<Command>().await, Err(IpcError::Truncated)),
        "partial header"
    );
}

//! The engine <-> UI message vocabulary. Every type is plain data that
//! serialises as JSON; field names are part of the wire protocol.

use std::path::PathBuf;

use fthrust_core::capture_config::CaptureConfig;
use serde::{Deserialize, Serialize};

/// A capture backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    /// `zwlr_screencopy_manager_v1`.
    WlrScreencopy,
    /// `ext_image_copy_capture_manager_v1`.
    ExtImageCopy,
    /// The ScreenCast portal over PipeWire.
    PortalScreenCast,
    /// A test-only frame generator (`FTHRUST_SYNTHETIC_BACKEND=1`).
    Synthetic,
}

impl BackendKind {
    /// The reference's display name.
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::WlrScreencopy => "wlr-screencopy",
            BackendKind::ExtImageCopy => "ext-image-copy-capture",
            BackendKind::PortalScreenCast => "ScreenCast portal",
            BackendKind::Synthetic => "synthetic",
        }
    }

    /// Parses `FTHR_CAPTURE_BACKEND` (`wlr`, `ext`, `portal`, `synthetic`,
    /// case-insensitive). `x11`, empty and unknown values are ignored.
    pub fn parse_override(value: &str) -> Option<BackendKind> {
        match value.trim().to_ascii_lowercase().as_str() {
            "wlr" => Some(BackendKind::WlrScreencopy),
            "ext" => Some(BackendKind::ExtImageCopy),
            "portal" => Some(BackendKind::PortalScreenCast),
            "synthetic" => Some(BackendKind::Synthetic),
            _ => None,
        }
    }
}

/// What the engine is started with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// The capture settings (the UI's `CaptureConfig`).
    pub capture: CaptureConfig,
    /// Byte budget of the encoded ring in MiB; 0 = time-bounded only.
    pub max_buffer_mb: u32,
    /// A forced backend (no fallback), normally from `FTHR_CAPTURE_BACKEND`.
    pub backend_override: Option<BackendKind>,
}

/// A command from the UI to the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Start capture with this configuration, or reconfigure a running one.
    Configure(EngineConfig),
    /// Save the last `seconds` of replay to `path`.
    SaveClip {
        /// Correlates the reply events.
        request_id: u64,
        /// Destination file; must not exist.
        path: PathBuf,
        /// Window length; 0 means the configured buffer length.
        seconds: u32,
    },
    /// Start continuous recording to `path`.
    StartRecording {
        /// Correlates the reply events.
        request_id: u64,
        /// Destination file; must not exist.
        path: PathBuf,
    },
    /// Stop continuous recording.
    StopRecording {
        /// Correlates the reply events.
        request_id: u64,
    },
    /// Focus pause: stop encoding, keep the ring.
    Pause,
    /// Resume after `Pause`.
    Resume,
    /// Ask for a `Status` event now.
    GetStatus,
    /// Stop capture and exit.
    Shutdown,
}

/// Continuous-recording state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RecordingState {
    /// Not recording.
    Idle,
    /// Writing to `path` since `started_ns` (CLOCK_MONOTONIC).
    Recording {
        /// The file being written.
        path: PathBuf,
        /// When it started.
        started_ns: i64,
    },
    /// Finishing the file.
    Stopping,
}

/// One audio source's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SourceStatus {
    /// Not configured.
    Disabled,
    /// Opening.
    Connecting,
    /// Delivering samples.
    Streaming,
    /// Lost; retrying every second.
    Reconnecting {
        /// Why it was lost.
        reason: String,
    },
}

/// Both audio sources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioStatus {
    /// The desktop (sink monitor) source.
    pub system: SourceStatus,
    /// The microphone source.
    pub microphone: SourceStatus,
}

/// A successfully saved clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedClip {
    /// The published file.
    pub path: PathBuf,
    /// Video payload bytes written.
    pub bytes_written: u64,
    /// Whether any audio track was written.
    pub audio_included: bool,
    /// Why audio (or one track) was dropped, when it was.
    pub audio_error: Option<String>,
    /// Titles of the audio tracks written, in stream order.
    pub audio_tracks: Vec<String>,
}

/// The engine's state, pushed on change and on every heartbeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// Encoded frames since start.
    pub frames_captured: u64,
    /// Bytes written by the save in progress (or the last one).
    pub bytes_written: u64,
    /// Whether a replay encoder is open (the reference's `nvenc_active`).
    pub encoder_open: bool,
    /// FFmpeg encoder name, e.g. `h264_vaapi`.
    pub active_codec: String,
    /// `fthrust_core::capture_health::flags` bits.
    pub health_flags: u32,
    /// Incremented each time a backend generation starts.
    pub capture_generation: u32,
    /// Incremented each time the encoder is (re)opened.
    pub encoder_generation: u32,
    /// Content watchdog sample counter.
    pub content_sample_sequence: u32,
    /// Consecutive suspicious samples.
    pub content_suspicious_streak: u32,
    /// Last sample's luma mean.
    pub content_luma_mean: f32,
    /// Last sample's luma variance.
    pub content_luma_variance: f32,
    /// Why capture failed for good (with `BACKEND_FAILED`).
    pub capture_failure_detail: String,
    /// Why capture is recovering (with `RECOVERING`).
    pub recovery_reason: String,
    /// Focus pause active.
    pub paused: bool,
    /// Continuous recording state.
    pub recording: RecordingState,
    /// The backend in use.
    pub backend: Option<BackendKind>,
    /// The output being captured (wl_output name), when known.
    pub selected_output: String,
    /// Source width.
    pub native_width: u32,
    /// Source height.
    pub native_height: u32,
    /// Audio sources.
    pub audio: AudioStatus,
    /// The most recent save's outcome, kept across client reconnects.
    pub last_save: Option<(u64, Result<SavedClip, String>)>,
}

/// An event from the engine to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// First message on every connection.
    Hello {
        /// Must equal [`crate::PROTOCOL_VERSION`].
        protocol_version: u32,
        /// The engine's pid.
        pid: u32,
        /// The engine's crate version.
        version: String,
    },
    /// A startup condition the UI should show but that does not stop capture.
    StartupWarning {
        /// E.g. `DESKTOP_AUDIO_UNAVAILABLE`.
        code: String,
        /// Human-readable detail.
        detail: String,
    },
    /// Capture could not start; the engine exits after this.
    StartupError {
        /// E.g. `WAYLAND_UNAVAILABLE`.
        code: String,
        /// Human-readable detail.
        detail: String,
    },
    /// A `Configure` was applied.
    Configured {
        /// Normalisation warnings, one per clamped field.
        warnings: Vec<String>,
    },
    /// State changed, or a heartbeat with state.
    Status(StatusSnapshot),
    /// The save was admitted and is running.
    SaveStarted {
        /// From the command.
        request_id: u64,
    },
    /// Save progress.
    SaveProgress {
        /// From the command.
        request_id: u64,
        /// Video bytes written so far.
        bytes_written: u64,
    },
    /// The save finished.
    SaveResult {
        /// From the command.
        request_id: u64,
        /// The clip, or why it was not saved.
        result: Result<SavedClip, String>,
    },
    /// Recording started.
    RecordingStarted {
        /// From the command.
        request_id: u64,
        /// The file.
        path: PathBuf,
    },
    /// Recording stopped cleanly.
    RecordingStopped {
        /// From the command.
        request_id: u64,
        /// The file.
        path: PathBuf,
        /// Its size.
        bytes: u64,
    },
    /// Recording failed (the file keeps every committed fragment).
    RecordingError {
        /// From the command.
        request_id: u64,
        /// What went wrong.
        message: String,
    },
    /// Nothing changed in the last 500 ms.
    Heartbeat,
}

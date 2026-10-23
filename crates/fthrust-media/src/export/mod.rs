//! In-process export (`core/export_queue.py` `run_export`,
//! `core/export_lifecycle.py` `ExportJob`): runs a [`GraphPlan`] into a
//! staged file next to the output, validates it and renames it into place.
//! A hardware encoder failure retries once in software. Any other end
//! removes the staged file.

mod encode;
mod run;
pub mod validate;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use fthrust_core::export::graph::GraphPlan;

pub use encode::status_for_encoder;

/// Inactivity bound: no output for this long times the job out.
pub const INACTIVITY_S: f64 = 120.0;

/// The `export_quality` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// "Very fast".
    Fast,
    /// "High quality".
    Quality,
    /// "Lossless · MKV": FFV1 and FLAC.
    Lossless,
}

impl Quality {
    /// From the setting value (anything unknown is `Quality`).
    pub fn from_setting(value: &str) -> Quality {
        match value {
            "fast" => Quality::Fast,
            "lossless" => Quality::Lossless,
            _ => Quality::Quality,
        }
    }
}

/// One export.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// The clip.
    pub source: PathBuf,
    /// The final output path.
    pub output: PathBuf,
    /// What to run.
    pub plan: GraphPlan,
    /// Quality.
    pub quality: Quality,
    /// A size preset's video bitrate.
    pub video_bitrate_kbps: Option<u32>,
    /// A size preset's peak rate (`-maxrate`).
    pub max_rate_kbps: Option<u32>,
    /// Speed-adjusted kept length, for progress.
    pub expected_duration_s: f64,
    /// A size preset's target: the output must not exceed it.
    pub target_size_mb: Option<f64>,
    /// The output frame rate when a preset lowers it (else the source's).
    pub output_fps: Option<f64>,
    /// Where decoding may start for a multi-range export (the first kept
    /// range; its trims use source timestamps).
    pub decode_from_s: f64,
    /// Test hook: the inactivity bound instead of [`INACTIVITY_S`].
    #[doc(hidden)]
    pub inactivity_s: Option<f64>,
    /// Test hook: the first encoder attempt fails as a hardware one would.
    #[doc(hidden)]
    pub fail_first_encoder: bool,
}

/// `ExportState` terminal states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportState {
    /// Written, validated and renamed.
    Completed,
    /// Failed; `detail` says why.
    Failed,
    /// Cancelled.
    Cancelled,
    /// No output for [`INACTIVITY_S`].
    TimedOut,
}

impl ExportState {
    /// The queue's status word (`state.value.title()`).
    pub fn status(self) -> &'static str {
        match self {
            ExportState::Completed => "Completed",
            ExportState::Failed => "Failed",
            ExportState::Cancelled => "Cancelled",
            ExportState::TimedOut => "Timed_Out",
        }
    }
}

/// How an export ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportResult {
    /// End state.
    pub state: ExportState,
    /// Failure text (empty on success).
    pub detail: String,
}

impl ExportResult {
    fn new(state: ExportState, detail: impl Into<String>) -> ExportResult {
        ExportResult {
            state,
            detail: detail.into(),
        }
    }
}

/// Why one attempt stopped.
#[derive(Debug)]
pub(crate) enum Stop {
    Cancelled,
    TimedOut,
    /// The video encoder failed; `hardware` says whether a software retry
    /// may help.
    Encoder {
        hardware: bool,
        message: String,
    },
    Failed(String),
}

impl From<ffmpeg_next::Error> for Stop {
    fn from(e: ffmpeg_next::Error) -> Stop {
        Stop::Failed(e.to_string())
    }
}

impl From<crate::MediaError> for Stop {
    fn from(e: crate::MediaError) -> Stop {
        Stop::Failed(e.to_string())
    }
}

impl From<std::io::Error> for Stop {
    fn from(e: std::io::Error) -> Stop {
        Stop::Failed(e.to_string())
    }
}

/// `n` random lowercase hex digits (output and staged names).
pub fn random_hex(n: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(n);
    out
}

fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Runs `job`: `on_status` gets the reference's status strings,
/// `on_progress` a percentage capped at 99 while running.
pub fn run(
    job: &Job,
    cancel: &AtomicBool,
    on_status: &mut dyn FnMut(&str),
    on_progress: &mut dyn FnMut(f64),
) -> ExportResult {
    if let Err(e) = crate::init() {
        return ExportResult::new(ExportState::Failed, e.to_string());
    }
    if cancel.load(Ordering::Relaxed) {
        return ExportResult::new(ExportState::Cancelled, "Export cancelled");
    }
    on_status("Preparing");
    let staged = fthrust_core::export::naming::staged_name(&job.output, &random_hex(32));
    let final_existed = job.output.exists();
    let mut software = false;
    let mut fail_injected = job.fail_first_encoder;
    let outcome = loop {
        let attempt = run::attempt(
            job,
            &staged,
            software,
            std::mem::take(&mut fail_injected),
            cancel,
            on_status,
            on_progress,
        );
        match attempt {
            Err(Stop::Encoder { hardware: true, .. }) if !software => {
                discard(&staged);
                software = true;
                on_status("Software fallback · software");
            }
            other => break other,
        }
    };
    let result = match outcome {
        Ok(()) => finish(job, &staged, cancel),
        Err(Stop::Cancelled) => ExportResult::new(ExportState::Cancelled, "Export cancelled"),
        Err(Stop::TimedOut) => ExportResult::new(ExportState::TimedOut, "Export timed out"),
        Err(Stop::Encoder { message, .. }) | Err(Stop::Failed(message)) => {
            ExportResult::new(ExportState::Failed, message)
        }
    };
    discard(&staged);
    if result.state != ExportState::Completed && !final_existed && job.output.exists() {
        discard(&job.output);
    }
    result
}

/// FINALIZING: a non-empty staged file that validates, then the rename.
fn finish(job: &Job, staged: &Path, cancel: &AtomicBool) -> ExportResult {
    let size = std::fs::metadata(staged).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return ExportResult::new(ExportState::Failed, "Export produced no usable output");
    }
    if let Err(e) = validate::check(staged, cancel) {
        if cancel.load(Ordering::Relaxed) {
            return ExportResult::new(ExportState::Cancelled, "Export cancelled");
        }
        return ExportResult::new(ExportState::Failed, e);
    }
    if let Some(target) = job.target_size_mb
        && size as f64 > target * 1_048_576.0
    {
        return ExportResult::new(
            ExportState::Failed,
            format!(
                "Output is {:.1} MB, above the {} MB target",
                size as f64 / 1_048_576.0,
                trim_float(target)
            ),
        );
    }
    if cancel.load(Ordering::Relaxed) {
        return ExportResult::new(ExportState::Cancelled, "Export cancelled");
    }
    if let Err(e) = std::fs::rename(staged, &job.output) {
        return ExportResult::new(ExportState::Failed, e.to_string());
    }
    if std::fs::metadata(&job.output).map(|m| m.len()).unwrap_or(0) == 0 {
        return ExportResult::new(
            ExportState::Failed,
            "Export finalization produced no usable output",
        );
    }
    ExportResult::new(ExportState::Completed, "")
}

/// Python's `{:g}` for the size target.
fn trim_float(v: f64) -> String {
    let s = format!("{v}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

/// `probe_media` for preset planning; failures read
/// `Media inspection failed: …`.
pub fn media_facts(path: &Path) -> Result<fthrust_core::export::plan::MediaFacts, String> {
    use ffmpeg_next as ff;
    let fail = |e: &dyn std::fmt::Display| format!("Media inspection failed: {e}");
    crate::init().map_err(|e| fail(&e))?;
    let size_bytes = std::fs::metadata(path).map_err(|e| fail(&e))?.len();
    let ictx = ff::format::input(&path).map_err(|e| fail(&e))?;
    let duration_s = if ictx.duration() > 0 {
        ictx.duration() as f64 / f64::from(ff::ffi::AV_TIME_BASE)
    } else {
        0.0
    };
    let video = ictx
        .streams()
        .best(ff::media::Type::Video)
        .ok_or_else(|| fail(&"no video stream"))?;
    let vpar = video.parameters();
    let vdec = ff::codec::context::Context::from_parameters(vpar.clone())
        .and_then(|c| c.decoder().video())
        .map_err(|e| fail(&e))?;
    let rate = video.avg_frame_rate();
    let fps = if rate.numerator() > 0 && rate.denominator() > 0 {
        f64::from(rate.numerator()) / f64::from(rate.denominator())
    } else {
        0.0
    };
    let audio = ictx.streams().best(ff::media::Type::Audio);
    let has_audio = audio.is_some();
    let (audio_codec, audio_kbps) = audio
        .map(|a| {
            let par = a.parameters();
            let kbps = ff::codec::context::Context::from_parameters(par.clone())
                .and_then(|c| c.decoder().audio())
                .map(|d| d.bit_rate() / 1000)
                .unwrap_or(0);
            (par.id().name().to_owned(), kbps as u32)
        })
        .unwrap_or_default();
    Ok(fthrust_core::export::plan::MediaFacts {
        size_bytes,
        duration_s,
        width: vdec.width(),
        height: vdec.height(),
        fps,
        video_codec: vpar.id().name().to_owned(),
        has_audio,
        audio_codec,
        audio_kbps,
    })
}

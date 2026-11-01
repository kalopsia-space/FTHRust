//! The replay video encoder: tries the policy's candidates in order, scales
//! BGRA input to the output size, derives timestamps from the capture clock
//! and forces a keyframe every two seconds. Ports `encoder.cpp`.

use std::collections::VecDeque;

use ffmpeg_next::ffi;
use fthrust_core::capture_config::{Codec, EncoderPref};

use crate::encoder_policy::{encoder_candidates, preset_options};
use crate::ffi::capture_surfaces::{CaptureSurfaces, ExportedImage};
use crate::ffi::codec::{HwInput, OpenParams, RawEncoder};
use crate::ffi::frame::{OwnedFrame, check_bgra_geometry};
use crate::ffi::hw::{POOL_SURFACES, VaapiFrames, candidate_devices};
use crate::ffi::scaler::Scaler;
use crate::ffi::vpp::VaapiConverter;
use crate::ring::EncodedPacket;
use crate::{MediaError, Result};

/// What to open the encoder with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Expected source width (the scaler is rebuilt if frames differ).
    pub src_width: u32,
    /// Expected source height.
    pub src_height: u32,
    /// Output width.
    pub enc_width: u32,
    /// Output height.
    pub enc_height: u32,
    /// Frame rate; the packet time base is `1/fps`.
    pub fps: u32,
    /// Target bitrate.
    pub bitrate_kbps: u32,
    /// Codec preference.
    pub codec: Codec,
    /// Backend preference.
    pub encoder: EncoderPref,
    /// Preset 1 (fastest) to 7 (best quality).
    pub preset: u8,
}

/// A peak bitrate and the rate-control (VBV) buffer it holds over, FFmpeg's
/// `-maxrate` and `-bufsize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateCap {
    /// Peak bitrate.
    pub max_kbps: u32,
    /// Buffer size.
    pub buffer_kbits: u32,
}

/// How an encoder opens, besides its [`EncoderConfig`].
#[derive(Clone, Copy, Debug, Default)]
struct Opening {
    /// Capture: the encoder gets the settings
    /// [`crate::encoder_policy::capture_options`] lists (no frames held
    /// back, a keyframe every GOP) where its library takes them.
    capture: bool,
    /// Keyframe interval in frames (`None`: two seconds).
    gop: Option<u32>,
    /// Convert on the CPU even where the GPU could.
    force_cpu: bool,
    /// Test hook.
    inject: Option<GpuFailure>,
    rate_cap: Option<RateCap>,
}

#[derive(Clone, Copy, Debug)]
struct FrameTiming {
    pts: i64,
    wall_time_ns: i64,
}

/// What choosing a frame's pts and keyframe changes.
#[derive(Clone, Copy, Debug)]
struct Clock {
    epoch_ns: Option<i64>,
    last_input_pts: i64,
    keyframe_requested: bool,
}

/// Where BGRA is converted to the encoder's pixel format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conversion {
    /// On the GPU with VA-API `scale_vaapi` (VA-API encoders only).
    Gpu,
    /// On the CPU with swscale.
    Cpu,
}

/// Test hook: where to inject a GPU conversion failure.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuFailure {
    /// Opening the encoder with the converter's frames fails.
    Open,
    /// The first GPU convert fails.
    FirstConvert,
}

fn log_gpu_fallback(reason: &dyn std::fmt::Display) {
    tracing::info!(
        reason = %reason,
        "GPU colour conversion unavailable; converting on the CPU"
    );
}

/// The CPU conversion state.
#[derive(Debug)]
struct CpuPath {
    scaler: Scaler,
    sw_frame: OwnedFrame,
}

impl CpuPath {
    fn new(src_w: u32, src_h: u32, raw: &RawEncoder) -> Result<Self> {
        let (w, h) = raw.size();
        Ok(Self {
            scaler: Scaler::new(src_w.max(1), src_h.max(1), w, h, raw.input_format())?,
            sw_frame: OwnedFrame::video(raw.input_format(), w, h)?,
        })
    }
}

/// An open encoder.
#[derive(Debug)]
pub struct Encoder {
    raw: RawEncoder,
    /// `None` while the GPU converter is in use.
    cpu: Option<CpuPath>,
    /// Whether a GPU frame has been encoded successfully since the converter
    /// was last (re)built.
    gpu_proven: bool,
    inject_convert_failure: bool,
    inject_send_failure: bool,
    hw_frame: Option<OwnedFrame>,
    /// Scratch frame holding a fresh reference to the last frame for repeats.
    repeat_frame: OwnedFrame,
    /// Whether the last frame handed to the codec can be sent again.
    last_frame_valid: bool,
    conversions: u64,
    cfg: EncoderConfig,
    pts_epoch_ns: Option<i64>,
    last_input_pts: i64,
    last_forced_keyframe_pts: i64,
    keyframe_requested: bool,
    pending: VecDeque<FrameTiming>,
    generation: u64,
    /// Forced keyframe interval in frames (two seconds unless overridden).
    forced_keyframe_interval: i64,
    /// Exported capture surfaces, after [`Encoder::dmabuf_targets`].
    surfaces: Option<CaptureSurfaces>,
}

/// Whether this FFmpeg build has the named encoder (tests skip on `false`).
pub fn encoder_exists(name: &str) -> bool {
    crate::init().is_ok() && RawEncoder::exists(name)
}

impl Encoder {
    /// Opens the first candidate encoder that initialises, for capture.
    /// VA-API candidates are tried on every render node from
    /// [`crate::vaapi::candidate_devices`] until one encodes. A capture
    /// encoder must not hold frames back and must put a keyframe every GOP,
    /// so it gets the settings [`crate::encoder_policy::capture_options`]
    /// lists where its library takes them (SVT-AV1's low delay; Kvazaar
    /// without its B-frame pyramid and overlapped frames, and with the
    /// GOP's intra period), and its preset's alone where not.
    /// Exports, which flush at the end, use [`Encoder::open_with_gop`].
    pub fn open(cfg: &EncoderConfig) -> Result<Self> {
        crate::init()?;
        Self::open_with_devices(cfg, &candidate_devices())
    }

    /// Makes the next GPU convert fail (tests).
    #[doc(hidden)]
    pub fn inject_gpu_convert_failure(&mut self) {
        self.inject_convert_failure = true;
    }

    /// Makes the next frame fail before it reaches the codec, on any path
    /// (tests).
    #[doc(hidden)]
    pub fn inject_send_failure(&mut self) {
        self.inject_send_failure = true;
    }

    fn injected_send_failure(&mut self) -> Result<()> {
        if std::mem::take(&mut self.inject_send_failure) {
            return Err(MediaError::Invalid("injected send failure".into()));
        }
        Ok(())
    }

    /// [`Encoder::open`] with an explicit list of VA-API render nodes.
    pub fn open_with_devices(cfg: &EncoderConfig, devices: &[String]) -> Result<Self> {
        let opening = Opening {
            capture: true,
            ..Opening::default()
        };
        Self::open_inner(cfg, devices, opening)
    }

    /// [`Encoder::open`] with the CPU colour conversion forced (tests and
    /// the throughput measurement).
    #[doc(hidden)]
    pub fn open_forcing_cpu_conversion(cfg: &EncoderConfig) -> Result<Self> {
        crate::init()?;
        let opening = Opening {
            force_cpu: true,
            ..Opening::default()
        };
        Self::open_inner(cfg, &candidate_devices(), opening)
    }

    /// Makes frame uploads use the staging copy and `av_hwframe_transfer_data`
    /// instead of writing into mapped surfaces (the throughput comparison).
    #[doc(hidden)]
    pub fn force_transfer_upload(&mut self) {
        if let Some(hw) = self.raw.vaapi() {
            hw.force_transfer_upload();
        }
    }

    /// [`Encoder::open`] with an explicit keyframe interval in frames
    /// (playback proxies and tests; capture keeps the two-second default).
    pub fn open_with_gop(cfg: &EncoderConfig, gop_frames: u32) -> Result<Self> {
        crate::init()?;
        let opening = Opening {
            gop: Some(gop_frames.max(1)),
            ..Opening::default()
        };
        Self::open_inner(cfg, &candidate_devices(), opening)
    }

    /// [`Encoder::open_with_gop`] with a peak bitrate (size-targeted
    /// exports).
    pub fn open_with_gop_and_cap(
        cfg: &EncoderConfig,
        gop_frames: u32,
        cap: RateCap,
    ) -> Result<Self> {
        crate::init()?;
        let opening = Opening {
            gop: Some(gop_frames.max(1)),
            rate_cap: Some(cap),
            ..Opening::default()
        };
        Self::open_inner(cfg, &candidate_devices(), opening)
    }

    /// [`Encoder::open`] with a GPU conversion failure injected (tests).
    #[doc(hidden)]
    pub fn open_with_injected_gpu_failure(
        cfg: &EncoderConfig,
        failure: GpuFailure,
    ) -> Result<Self> {
        crate::init()?;
        let opening = Opening {
            inject: Some(failure),
            ..Opening::default()
        };
        let mut enc = Self::open_inner(cfg, &candidate_devices(), opening)?;
        enc.inject_convert_failure = failure == GpuFailure::FirstConvert;
        Ok(enc)
    }

    fn open_inner(cfg: &EncoderConfig, devices: &[String], opening: Opening) -> Result<Self> {
        crate::init()?;
        if cfg.fps == 0 || cfg.enc_width == 0 || cfg.enc_height == 0 {
            return Err(MediaError::Invalid(
                "encoder size and fps must be non-zero".into(),
            ));
        }
        let candidates = encoder_candidates(cfg.codec, cfg.encoder);
        let mut failures = Vec::new();
        let mut logged = false;
        for name in &candidates {
            if !RawEncoder::exists(name) {
                failures.push(format!("{name}: not in this FFmpeg build"));
                continue;
            }
            match Self::try_open(name, cfg, devices, opening, &mut logged) {
                Ok(raw) => {
                    tracing::info!(
                        encoder = *name,
                        width = cfg.enc_width,
                        height = cfg.enc_height,
                        fps = cfg.fps,
                        bitrate_kbps = cfg.bitrate_kbps,
                        max_kbps = opening.rate_cap.map(|c| c.max_kbps),
                        preset = cfg.preset,
                        "encoder opened"
                    );
                    return Self::finish_open(raw, cfg, opening.gop);
                }
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        let hint = if cfg.encoder == EncoderPref::Software {
            ""
        } else {
            " (no automatic software fallback; choose Software explicitly to use one)"
        };
        Err(MediaError::EncoderUnavailable(format!(
            "{}{hint}",
            if failures.is_empty() {
                "no candidates".to_string()
            } else {
                failures.join("; ")
            }
        )))
    }

    fn try_open(
        name: &'static str,
        cfg: &EncoderConfig,
        devices: &[String],
        opening: Opening,
        logged: &mut bool,
    ) -> Result<RawEncoder> {
        let Opening {
            capture,
            gop,
            force_cpu,
            inject,
            rate_cap,
        } = opening;
        let is_vaapi = name.contains("_vaapi");
        let is_hw =
            is_vaapi || name.contains("nvenc") || name.contains("amf") || name.contains("qsv");
        let params = OpenParams {
            width: cfg.enc_width,
            height: cfg.enc_height,
            fps: cfg.fps,
            bitrate_bps: i64::from(cfg.bitrate_kbps) * 1000,
            rate_cap: rate_cap.map(|c| {
                (
                    i64::from(c.max_kbps) * 1000,
                    i64::from(c.buffer_kbits) * 1000,
                )
            }),
            options: preset_options(name, cfg.preset).unwrap_or_default(),
            gop_frames: gop,
            pix_fmt: if is_hw {
                ffi::AVPixelFormat::AV_PIX_FMT_NV12
            } else {
                ffi::AVPixelFormat::AV_PIX_FMT_YUV420P
            },
            strict_options: false,
        };
        if !is_vaapi {
            let preferred = if capture {
                // The GOP the codec context and the forced keyframes use.
                let gop_frames = gop.unwrap_or(cfg.fps * 2).max(1);
                crate::encoder_policy::capture_options(name, cfg.preset, gop_frames)
            } else {
                None
            };
            let wanted = preferred.is_some();
            let (raw, taken) = RawEncoder::open_preferring(name, &params, preferred)?;
            if wanted && !taken {
                tracing::warn!(
                    encoder = name,
                    "the encoder refused its capture settings: it may hold frames back, \
                     so a clip ends that much before the moment it was saved"
                );
            }
            return Ok(raw);
        }
        // The node where a device opens is not necessarily the node whose
        // driver has this encoder (NVIDIA on renderD128, AMD on renderD129):
        // keep going until an encoder actually opens.
        let cpu_only = force_cpu || std::env::var("FTHR_CPU_COLOR_CONVERT").as_deref() == Ok("1");
        let mut failures = Vec::new();
        for device in devices {
            if !cpu_only {
                let attempt = VaapiFrames::create_on(device, cfg.enc_width, cfg.enc_height)
                    .and_then(|hw| Self::gpu_input(hw, cfg))
                    .and_then(|input| {
                        if inject == Some(GpuFailure::Open) {
                            return Err(MediaError::Invalid("injected GPU open failure".into()));
                        }
                        RawEncoder::open(name, &params, Some(input))
                    });
                match attempt {
                    Ok(raw) => return Ok(raw),
                    Err(e) => {
                        // Retry this device with plain NV12 uploads.
                        if !std::mem::replace(logged, true) {
                            log_gpu_fallback(&e);
                        }
                    }
                }
            }
            match VaapiFrames::create_on(device, cfg.enc_width, cfg.enc_height)
                .and_then(|hw| RawEncoder::open(name, &params, Some(HwInput::Pool(hw))))
            {
                Ok(raw) => return Ok(raw),
                Err(e) => failures.push(format!("{device}: {e}")),
            }
        }
        Err(MediaError::EncoderUnavailable(if failures.is_empty() {
            "no VA-API render node to try".into()
        } else {
            failures.join(", ")
        }))
    }

    /// Wraps `hw` with a GPU converter that has converted one black frame.
    fn gpu_input(hw: VaapiFrames, cfg: &EncoderConfig) -> Result<HwInput> {
        let (src_w, src_h) = (cfg.src_width.max(1), cfg.src_height.max(1));
        let mut converter = VaapiConverter::new(&hw, src_w, src_h, cfg.enc_width, cfg.enc_height)?;
        let row = src_w as usize * 4;
        let len = row
            .checked_mul(src_h as usize)
            .ok_or_else(|| MediaError::Invalid(format!("source {src_w}x{src_h} is too large")))?;
        let black = vec![0u8; len];
        let mut out = OwnedFrame::empty()?;
        converter.convert(&black, row, &mut out)?;
        Ok(HwInput::Converter(hw, converter))
    }

    fn finish_open(raw: RawEncoder, cfg: &EncoderConfig, gop: Option<u32>) -> Result<Self> {
        let cpu = if raw.has_converter() {
            None
        } else {
            Some(CpuPath::new(cfg.src_width, cfg.src_height, &raw)?)
        };
        let hw_frame = if raw.is_vaapi() {
            Some(OwnedFrame::empty()?)
        } else {
            None
        };
        Ok(Self {
            raw,
            cpu,
            gpu_proven: false,
            inject_convert_failure: false,
            inject_send_failure: false,
            hw_frame,
            repeat_frame: OwnedFrame::empty()?,
            last_frame_valid: false,
            conversions: 0,
            cfg: cfg.clone(),
            pts_epoch_ns: None,
            last_input_pts: -1,
            last_forced_keyframe_pts: -1,
            keyframe_requested: false,
            pending: VecDeque::new(),
            generation: 0,
            forced_keyframe_interval: i64::from(gop.unwrap_or(cfg.fps * 2).max(1)),
            surfaces: None,
        })
    }

    /// Whether `name` exists and completes a real one-frame 640×360 encode
    /// that yields a packet with data (registration alone does not prove a
    /// driver works, and accepting a frame does not prove it encodes).
    pub fn encoder_works(name: &'static str) -> bool {
        if crate::init().is_err() || !RawEncoder::exists(name) {
            return false;
        }
        let cfg = EncoderConfig {
            src_width: 640,
            src_height: 360,
            enc_width: 640,
            enc_height: 360,
            fps: 30,
            bitrate_kbps: 4000,
            codec: Codec::Auto,
            encoder: EncoderPref::Auto,
            preset: 4,
        };
        let Ok(raw) = Self::try_open(
            name,
            &cfg,
            &candidate_devices(),
            Opening::default(),
            &mut false,
        ) else {
            return false;
        };
        let Ok(mut encoder) = Self::finish_open(raw, &cfg, None) else {
            return false;
        };
        let black = vec![0u8; 640 * 360 * 4];
        let mut out = Vec::new();
        encoder
            .encode(&black, 640 * 4, 640, 360, 0, &mut out)
            .is_ok()
            && encoder.flush(&mut out).is_ok()
            && out.iter().any(|p| !p.data.is_empty())
    }

    /// Name of the encoder in use.
    pub fn active_codec(&self) -> &'static str {
        self.raw.name()
    }

    /// Output width.
    pub fn width(&self) -> u32 {
        self.cfg.enc_width
    }

    /// Output height.
    pub fn height(&self) -> u32 {
        self.cfg.enc_height
    }

    /// Where colour conversion happens right now.
    pub fn conversion(&self) -> Conversion {
        if self.cpu.is_some() {
            Conversion::Cpu
        } else {
            Conversion::Gpu
        }
    }

    /// Whether the VA-API path is active.
    pub fn is_vaapi(&self) -> bool {
        self.raw.is_vaapi()
    }

    /// The render node in use, when the VA-API path is active.
    pub fn vaapi_device(&self) -> Option<&str> {
        self.raw.vaapi().map(|v| v.device_path.as_str())
    }

    /// The render node the encoder (and its capture surfaces) live on.
    pub fn render_node(&self) -> Option<&str> {
        self.vaapi_device()
    }

    /// Frees the capture surfaces from [`Encoder::dmabuf_targets`] once the
    /// compositor no longer writes into them (zero copy fell back to shm).
    /// The converter built on their pool goes too, so it frees with them;
    /// the next shared-memory frame builds one for uploads.
    pub fn release_dmabuf_targets(&mut self) {
        self.surfaces = None;
        if self.cpu.is_none() && !self.raw.converter_takes_uploads() {
            self.raw.drop_converter();
        }
    }

    /// Whether the encoder's own NV12 upload pool and the GPU converter's
    /// BGRX upload pool exist (tests). Neither does while capture writes
    /// into dmabuf surfaces: shared-memory frames encoded before the first
    /// surface frame create the converter's, and that frame drops it.
    #[doc(hidden)]
    pub fn upload_pools_created(&self) -> (bool, bool) {
        self.raw.upload_pools_created()
    }

    /// Allocates `count` `width`×`height` BGRX VA surfaces on the encoder's
    /// device, exported as dmabufs the compositor can copy captures into,
    /// and builds the GPU converter on their pool, which replaces its
    /// upload pool. Replaces any earlier set. Errors unless conversion is
    /// on the GPU.
    pub fn dmabuf_targets(
        &mut self,
        width: u32,
        height: u32,
        count: usize,
    ) -> Result<Vec<ExportedImage>> {
        if self.conversion() != Conversion::Gpu {
            return Err(MediaError::Invalid(
                "dmabuf capture needs GPU colour conversion".into(),
            ));
        }
        if count == 0 || count > POOL_SURFACES {
            return Err(MediaError::Invalid(format!(
                "dmabuf targets: count must be 1 to {POOL_SURFACES}, got {count}"
            )));
        }
        self.surfaces = None;
        let device = self
            .raw
            .vaapi()
            .ok_or_else(|| MediaError::Invalid("no VA-API device".into()))?;
        let surfaces = CaptureSurfaces::new(device, width, height, count)?;
        let images = surfaces.images()?;
        self.raw.bind_converter_to(surfaces.surface(0))?;
        self.gpu_proven = false;
        self.surfaces = Some(surfaces);
        Ok(images)
    }

    fn capture_surfaces(&self) -> Result<&CaptureSurfaces> {
        self.surfaces
            .as_ref()
            .ok_or_else(|| MediaError::Invalid("no dmabuf targets allocated".into()))
    }

    /// Downloads capture surface `index` as BGRX rows into `out` and returns
    /// their stride (the watchdog's fallback frame).
    pub fn download_surface(&self, index: usize, out: &mut Vec<u8>) -> Result<usize> {
        self.capture_surfaces()?.download(index, out)
    }

    /// Copies the content watchdog's `columns`×`rows` sample pixels out of
    /// capture surface `index` into `out`, packed BGRX, without
    /// downloading the surface.
    pub fn sample_surface(
        &self,
        index: usize,
        columns: usize,
        rows: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        self.capture_surfaces()?
            .sample_grid(index, columns, rows, out)
    }

    /// Writes BGRA rows into capture surface `index` through a mapping, in
    /// place of the compositor (tests).
    #[doc(hidden)]
    pub fn write_surface(&self, index: usize, bgra: &[u8], stride: usize) -> Result<()> {
        self.capture_surfaces()?.write(index, bgra, stride)
    }

    /// Encodes capture surface `index`, captured at `wall_time_ns`, converting
    /// it on the GPU with no upload. pts, keyframes, the repeat frame and
    /// packet timing follow [`Encoder::encode`]. A failure (on the first
    /// surface frame or later) returns `Err` with nothing sent and the
    /// clock restored; the engine then drops that frame and captures through
    /// shared memory.
    pub fn encode_surface(
        &mut self,
        index: usize,
        wall_time_ns: i64,
        out: &mut Vec<EncodedPacket>,
    ) -> Result<()> {
        let Some(surfaces) = &self.surfaces else {
            return Err(MediaError::Invalid("no dmabuf targets allocated".into()));
        };
        if index >= surfaces.len() {
            return Err(MediaError::Invalid(format!(
                "surface {index} out of range ({} surfaces)",
                surfaces.len()
            )));
        }
        if self.cpu.is_some() {
            return Err(MediaError::Invalid(
                "dmabuf capture needs GPU colour conversion".into(),
            ));
        }
        self.last_frame_valid = false;
        self.repeat_frame.unref();
        // A converter built for uploads (shared-memory frames encoded
        // before the first surface frame, as zero copy's warm-up does)
        // goes too, with its upload pool, rather than staying for the run.
        if self.raw.converter_source_size() != Some(surfaces.size())
            || self.raw.converter_takes_uploads()
        {
            self.raw.bind_converter_to(surfaces.surface(index))?;
            self.gpu_proven = false;
        }
        // Nothing reaches the codec on a failure: restore the clock state
        // so the caller's retry through `encode` gets this frame's pts and
        // keyframe.
        let clock = self.clock();
        let (frame_pts, force_key) = self.next_pts_and_key(wall_time_ns);
        if let Err(e) = self.convert_surface_and_send(index, frame_pts, force_key) {
            self.restore_clock(clock);
            tracing::debug!(reason = %e, "encoding from the capture surface failed");
            return Err(e);
        }
        self.gpu_proven = true;
        self.last_frame_valid = true;
        self.pending.push_back(FrameTiming {
            pts: frame_pts,
            wall_time_ns,
        });
        if force_key {
            self.last_forced_keyframe_pts = frame_pts;
        }
        self.drain(wall_time_ns, out)
    }

    fn convert_surface_and_send(&mut self, index: usize, pts: i64, force_key: bool) -> Result<()> {
        if std::mem::take(&mut self.inject_convert_failure) {
            return Err(MediaError::Invalid("injected GPU convert failure".into()));
        }
        let (Some(surfaces), Some(hw_frame)) = (&self.surfaces, self.hw_frame.as_mut()) else {
            return Err(MediaError::Invalid("GPU conversion without a frame".into()));
        };
        self.raw
            .convert_surface(surfaces.surface(index), hw_frame)?;
        self.conversions += 1;
        hw_frame.set_pts(pts);
        hw_frame.set_forced_keyframe(force_key);
        self.raw.send_frame(Some(hw_frame))
    }

    /// Makes the next frame an IDR frame (a recording can start at once
    /// instead of waiting for the next scheduled keyframe).
    pub fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    /// Stamps every subsequent packet with `generation`.
    pub fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
    }

    /// The codec's global header for the container writer.
    pub fn extradata(&self) -> Vec<u8> {
        self.raw.extradata()
    }

    /// The codec id for the container writer.
    pub fn codec_id(&self) -> ffmpeg_next::codec::Id {
        ffmpeg_next::codec::Id::from(self.raw.codec_id())
    }

    /// Encodes one BGRA frame captured at `wall_time_ns` (monotonic). The
    /// output size stays fixed; a different source size rebuilds the scaler.
    /// Packets are appended to `out` with capture times matched by pts.
    pub fn encode(
        &mut self,
        bgra: &[u8],
        stride: usize,
        width: u32,
        height: u32,
        wall_time_ns: i64,
        out: &mut Vec<EncodedPacket>,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(MediaError::Invalid("frame size must be non-zero".into()));
        }
        check_bgra_geometry(bgra.len(), stride, width, height as usize)?;
        // The new frame replaces the last one: drop the repeat reference so
        // it pins no surface through the rebuild or the next convert.
        self.last_frame_valid = false;
        self.repeat_frame.unref();
        if self.cpu.is_none()
            && (self.raw.converter_source_size() != Some((width, height))
                || !self.raw.converter_takes_uploads())
        {
            if self.raw.converter_takes_uploads() {
                tracing::info!(
                    width,
                    height,
                    "source size changed; rebuilding the GPU converter"
                );
            } else {
                tracing::debug!(width, height, "building the GPU converter for uploads");
            }
            // Frames from the rebuilt converter come from a new sink frames
            // context on the same VADisplay with the same NV12 format and
            // output size. h264_vaapi only uses avctx->hw_frames_ctx at init
            // and reads each surface from data[3]. A driver that rejects
            // this fails the first send after the rebuild, which gets the same
            // CPU fallback and retry as the first frame ever (gpu_proven is
            // reset here).
            match self.raw.rebuild_converter(width, height) {
                Ok(()) => self.gpu_proven = false,
                Err(e) => {
                    log_gpu_fallback(&e);
                    self.switch_to_cpu(width, height)?;
                }
            }
        }

        // A frame that never reaches the codec leaves the clock as it was,
        // so a pending keyframe request waits for the next frame that does.
        let clock = self.clock();
        let (frame_pts, force_key) = self.next_pts_and_key(wall_time_ns);
        if let Err(e) =
            self.convert_with_fallback(bgra, stride, width, height, frame_pts, force_key)
        {
            self.restore_clock(clock);
            return Err(e);
        }
        if self.cpu.is_none() {
            self.gpu_proven = true;
        }
        self.last_frame_valid = true;
        self.pending.push_back(FrameTiming {
            pts: frame_pts,
            wall_time_ns,
        });
        if force_key {
            self.last_forced_keyframe_pts = frame_pts;
        }
        self.drain(wall_time_ns, out)
    }

    /// [`Encoder::convert_and_send`], retried on the CPU when the first GPU
    /// frame fails.
    fn convert_with_fallback(
        &mut self,
        bgra: &[u8],
        stride: usize,
        width: u32,
        height: u32,
        pts: i64,
        force_key: bool,
    ) -> Result<()> {
        let first_gpu = self.cpu.is_none() && !self.gpu_proven;
        match self.convert_and_send(bgra, stride, width, height, pts, force_key) {
            Err(e) if first_gpu => {
                log_gpu_fallback(&e);
                self.switch_to_cpu(width, height)?;
                self.convert_and_send(bgra, stride, width, height, pts, force_key)
            }
            sent => sent,
        }
    }

    /// The clock state [`Encoder::next_pts_and_key`] advances.
    fn clock(&self) -> Clock {
        Clock {
            epoch_ns: self.pts_epoch_ns,
            last_input_pts: self.last_input_pts,
            keyframe_requested: self.keyframe_requested,
        }
    }

    /// Undoes [`Encoder::next_pts_and_key`] for a frame the codec never got.
    fn restore_clock(&mut self, clock: Clock) {
        self.pts_epoch_ns = clock.epoch_ns;
        self.last_input_pts = clock.last_input_pts;
        self.keyframe_requested = clock.keyframe_requested;
    }

    /// The pts for a frame captured at `wall_time_ns` and whether it is a
    /// forced keyframe; consumes a pending keyframe request.
    fn next_pts_and_key(&mut self, wall_time_ns: i64) -> (i64, bool) {
        let fps = i64::from(self.cfg.fps);
        let frame_pts = match self.pts_epoch_ns {
            None => {
                self.pts_epoch_ns = Some(wall_time_ns);
                0
            }
            Some(epoch) => {
                let elapsed = (wall_time_ns - epoch).max(0);
                let whole = elapsed / 1_000_000_000;
                let rem = elapsed % 1_000_000_000;
                whole * fps + (rem * fps + 500_000_000) / 1_000_000_000
            }
        };
        let frame_pts = frame_pts.max(self.last_input_pts + 1);
        self.last_input_pts = frame_pts;
        let force_key = self.last_forced_keyframe_pts < 0
            || frame_pts - self.last_forced_keyframe_pts >= self.forced_keyframe_interval
            || std::mem::take(&mut self.keyframe_requested);
        (frame_pts, force_key)
    }

    /// Sends the last frame handed to the codec again, stamped for
    /// `wall_time_ns`: a new reference to its surface, no conversion. The pts
    /// and keyframe follow the same rules as [`Encoder::encode`]. Returns
    /// `Ok(false)`, sending nothing, when there is no previous frame to
    /// repeat; the caller then uses `encode`.
    pub fn encode_repeat(
        &mut self,
        wall_time_ns: i64,
        out: &mut Vec<EncodedPacket>,
    ) -> Result<bool> {
        if !self.last_frame_valid || self.pts_epoch_ns.is_none() {
            return Ok(false);
        }
        // Both VA-API paths hand the codec `hw_frame`; software encoders
        // take the scaler's output.
        let last = match (&self.hw_frame, &self.cpu) {
            (Some(hw), _) => hw,
            (None, Some(cpu)) => &cpu.sw_frame,
            (None, None) => return Ok(false),
        };
        self.repeat_frame.reference(last)?;
        let clock = self.clock();
        let (pts, force_key) = self.next_pts_and_key(wall_time_ns);
        self.repeat_frame.set_pts(pts);
        self.repeat_frame.set_forced_keyframe(force_key);
        let sent = self
            .injected_send_failure()
            .and_then(|()| self.raw.send_frame(Some(&self.repeat_frame)));
        if let Err(e) = sent {
            self.restore_clock(clock);
            return Err(e);
        }
        self.pending.push_back(FrameTiming { pts, wall_time_ns });
        if force_key {
            self.last_forced_keyframe_pts = pts;
        }
        self.drain(wall_time_ns, out)?;
        Ok(true)
    }

    /// Whether a repeat still holds a reference to an old surface (tests).
    #[doc(hidden)]
    pub fn holds_repeat_reference(&self) -> bool {
        self.repeat_frame.is_referenced()
    }

    /// How many BGRA frames have been converted (tests).
    #[doc(hidden)]
    pub fn conversions(&self) -> u64 {
        self.conversions
    }

    fn switch_to_cpu(&mut self, width: u32, height: u32) -> Result<()> {
        self.raw.drop_converter();
        self.cpu = Some(CpuPath::new(width, height, &self.raw)?);
        Ok(())
    }

    /// Converts `bgra` on the active path and submits it to the encoder.
    fn convert_and_send(
        &mut self,
        bgra: &[u8],
        stride: usize,
        width: u32,
        height: u32,
        pts: i64,
        force_key: bool,
    ) -> Result<()> {
        self.injected_send_failure()?;
        if let Some(cpu) = &mut self.cpu {
            if cpu.scaler.source_size() != (width, height) {
                tracing::info!(
                    width,
                    height,
                    "source size changed; rescaling to the output size"
                );
                let (w, h) = self.raw.size();
                cpu.scaler = Scaler::new(width, height, w, h, self.raw.input_format())?;
            }
            cpu.sw_frame.make_writable()?;
            cpu.scaler
                .scale_bgra(bgra, stride, height, &mut cpu.sw_frame)?;
            self.conversions += 1;
            cpu.sw_frame.set_pts(pts);
            cpu.sw_frame.set_forced_keyframe(force_key);
            match (self.raw.vaapi(), self.hw_frame.as_mut()) {
                (Some(hw), Some(hw_frame)) => {
                    hw.upload(hw_frame, &cpu.sw_frame)?;
                    self.raw.send_frame(Some(hw_frame))
                }
                _ => self.raw.send_frame(Some(&cpu.sw_frame)),
            }
        } else if let Some(hw_frame) = self.hw_frame.as_mut() {
            if std::mem::take(&mut self.inject_convert_failure) {
                return Err(MediaError::Invalid("injected GPU convert failure".into()));
            }
            self.raw.convert(bgra, stride, hw_frame)?;
            self.conversions += 1;
            hw_frame.set_pts(pts);
            hw_frame.set_forced_keyframe(force_key);
            self.raw.send_frame(Some(hw_frame))
        } else {
            Err(MediaError::Invalid("GPU conversion without a frame".into()))
        }
    }

    fn drain(&mut self, fallback_wall_ns: i64, out: &mut Vec<EncodedPacket>) -> Result<()> {
        let generation = self.generation;
        let pending = &mut self.pending;
        self.raw.receive_packets(|p| {
            let mut wall_time_ns = fallback_wall_ns;
            if let Some(pos) = pending.iter().position(|t| t.pts == p.pts) {
                wall_time_ns = pending[pos].wall_time_ns;
                pending.drain(..=pos);
            }
            out.push(EncodedPacket {
                data: p.data.into(),
                pts: p.pts,
                dts: p.dts,
                keyframe: p.keyframe,
                wall_time_ns,
                generation,
            });
        })
    }

    /// Drains the encoder's delayed packets (end of a generation).
    pub fn flush(&mut self, out: &mut Vec<EncodedPacket>) -> Result<()> {
        self.raw.send_frame(None)?;
        let last = self.pending.back().map_or(0, |t| t.wall_time_ns);
        self.drain(last, out)
    }
}

/// Free-function form of [`Encoder::encoder_works`].
pub fn encoder_works(name: &'static str) -> bool {
    Encoder::encoder_works(name)
}

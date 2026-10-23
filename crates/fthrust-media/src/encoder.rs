//! The replay video encoder: tries the policy's candidates in order, scales
//! BGRA input to the output size, derives timestamps from the capture clock
//! and forces a keyframe every two seconds. Ports `encoder.cpp`.

use std::collections::VecDeque;

use ffmpeg_next::ffi;
use fthrust_core::capture_config::{Codec, EncoderPref};

use crate::encoder_policy::{encoder_candidates, preset_options};
use crate::ffi::codec::{OpenParams, RawEncoder};
use crate::ffi::frame::OwnedFrame;
use crate::ffi::hw::{VaapiFrames, candidate_devices};
use crate::ffi::scaler::Scaler;
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

#[derive(Clone, Copy, Debug)]
struct FrameTiming {
    pts: i64,
    wall_time_ns: i64,
}

/// An open encoder.
#[derive(Debug)]
pub struct Encoder {
    raw: RawEncoder,
    scaler: Scaler,
    sw_frame: OwnedFrame,
    hw_frame: Option<OwnedFrame>,
    cfg: EncoderConfig,
    pts_epoch_ns: Option<i64>,
    last_input_pts: i64,
    last_forced_keyframe_pts: i64,
    keyframe_requested: bool,
    pending: VecDeque<FrameTiming>,
    generation: u64,
    /// Forced keyframe interval in frames (two seconds unless overridden).
    forced_keyframe_interval: i64,
}

impl Encoder {
    /// Opens the first candidate encoder that initialises. VA-API
    /// candidates are tried on every render node from
    /// [`crate::vaapi::candidate_devices`] until one encodes.
    pub fn open(cfg: &EncoderConfig) -> Result<Self> {
        crate::init()?;
        Self::open_with_devices(cfg, &candidate_devices())
    }

    /// [`Encoder::open`] with an explicit list of VA-API render nodes.
    pub fn open_with_devices(cfg: &EncoderConfig, devices: &[String]) -> Result<Self> {
        Self::open_inner(cfg, devices, None)
    }

    /// [`Encoder::open`] with an explicit keyframe interval in frames
    /// (playback proxies and tests; capture keeps the two-second default).
    pub fn open_with_gop(cfg: &EncoderConfig, gop_frames: u32) -> Result<Self> {
        crate::init()?;
        Self::open_inner(cfg, &candidate_devices(), Some(gop_frames.max(1)))
    }

    fn open_inner(cfg: &EncoderConfig, devices: &[String], gop: Option<u32>) -> Result<Self> {
        crate::init()?;
        if cfg.fps == 0 || cfg.enc_width == 0 || cfg.enc_height == 0 {
            return Err(MediaError::Invalid(
                "encoder size and fps must be non-zero".into(),
            ));
        }
        let candidates = encoder_candidates(cfg.codec, cfg.encoder);
        let mut failures = Vec::new();
        for name in &candidates {
            if !RawEncoder::exists(name) {
                failures.push(format!("{name}: not in this FFmpeg build"));
                continue;
            }
            match Self::try_open(name, cfg, devices, gop) {
                Ok(raw) => {
                    tracing::info!(
                        encoder = *name,
                        width = cfg.enc_width,
                        height = cfg.enc_height,
                        fps = cfg.fps,
                        bitrate_kbps = cfg.bitrate_kbps,
                        preset = cfg.preset,
                        "encoder opened"
                    );
                    return Self::finish_open(raw, cfg, gop);
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
        gop: Option<u32>,
    ) -> Result<RawEncoder> {
        let is_vaapi = name.contains("_vaapi");
        let is_hw =
            is_vaapi || name.contains("nvenc") || name.contains("amf") || name.contains("qsv");
        let params = OpenParams {
            width: cfg.enc_width,
            height: cfg.enc_height,
            fps: cfg.fps,
            bitrate_bps: i64::from(cfg.bitrate_kbps) * 1000,
            options: preset_options(name, cfg.preset).unwrap_or_default(),
            gop_frames: gop,
            pix_fmt: if is_hw {
                ffi::AVPixelFormat::AV_PIX_FMT_NV12
            } else {
                ffi::AVPixelFormat::AV_PIX_FMT_YUV420P
            },
        };
        if !is_vaapi {
            return RawEncoder::open(name, &params, None);
        }
        // The node where a device opens is not necessarily the node whose
        // driver has this encoder (NVIDIA on renderD128, AMD on renderD129):
        // keep going until an encoder actually opens.
        let mut failures = Vec::new();
        for device in devices {
            match VaapiFrames::create_on(device, cfg.enc_width, cfg.enc_height)
                .and_then(|hw| RawEncoder::open(name, &params, Some(hw)))
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

    fn finish_open(raw: RawEncoder, cfg: &EncoderConfig, gop: Option<u32>) -> Result<Self> {
        let (w, h) = raw.size();
        let scaler = Scaler::new(
            cfg.src_width.max(1),
            cfg.src_height.max(1),
            w,
            h,
            raw.input_format(),
        )?;
        let sw_frame = OwnedFrame::video(raw.input_format(), w, h)?;
        let hw_frame = if raw.is_vaapi() {
            Some(OwnedFrame::empty()?)
        } else {
            None
        };
        Ok(Self {
            raw,
            scaler,
            sw_frame,
            hw_frame,
            cfg: cfg.clone(),
            pts_epoch_ns: None,
            last_input_pts: -1,
            last_forced_keyframe_pts: -1,
            keyframe_requested: false,
            pending: VecDeque::new(),
            generation: 0,
            forced_keyframe_interval: i64::from(gop.unwrap_or(cfg.fps * 2).max(1)),
        })
    }

    /// Whether `name` exists and completes a real one-frame 640×360 encode
    /// (registration alone does not prove a driver works).
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
        let Ok(raw) = Self::try_open(name, &cfg, &candidate_devices(), None) else {
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

    /// Whether the VA-API path is active.
    pub fn is_vaapi(&self) -> bool {
        self.raw.is_vaapi()
    }

    /// The render node in use, when the VA-API path is active.
    pub fn vaapi_device(&self) -> Option<&str> {
        self.raw.vaapi().map(|v| v.device_path.as_str())
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
        if stride < width as usize * 4 {
            return Err(MediaError::Invalid(format!(
                "stride {stride} is smaller than width*4 ({})",
                width * 4
            )));
        }
        if bgra.len() < stride * (height as usize - 1) + width as usize * 4 {
            return Err(MediaError::Invalid(
                "frame buffer is shorter than its declared geometry".into(),
            ));
        }
        if self.scaler.source_size() != (width, height) {
            tracing::info!(
                width,
                height,
                "source size changed; rescaling to the output size"
            );
            let (w, h) = self.raw.size();
            self.scaler = Scaler::new(width, height, w, h, self.raw.input_format())?;
        }
        self.sw_frame.make_writable()?;
        self.scaler
            .scale_bgra(bgra, stride, height, &mut self.sw_frame)?;

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
        self.sw_frame.set_pts(frame_pts);
        self.sw_frame.set_forced_keyframe(force_key);

        if let (Some(hw), Some(hw_frame)) = (self.raw.vaapi(), self.hw_frame.as_mut()) {
            hw.upload(hw_frame, &self.sw_frame)?;
            self.raw.send_frame(Some(hw_frame))?;
        } else {
            self.raw.send_frame(Some(&self.sw_frame))?;
        }
        self.pending.push_back(FrameTiming {
            pts: frame_pts,
            wall_time_ns,
        });
        if force_key {
            self.last_forced_keyframe_pts = frame_pts;
        }
        self.drain(wall_time_ns, out)
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
                data: p.data,
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

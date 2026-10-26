//! The seek-proxy cache (`core/playback_proxy.py`): clips with keyframe gaps
//! over 2.1 s get a disposable 30-frame-GOP preview under
//! `~/.fthr/playback`, keyed by the source file's identity. Originals,
//! metadata and exports never use the proxy; a failure keeps the source.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_next::media::Type;
use ffmpeg_next::util::color::TransferCharacteristic;
use ffmpeg_next::{codec, format};
use sha2::{Digest, Sha256};

use super::video::VideoDecoder;
use crate::encoder::{Encoder, EncoderConfig};
use crate::ffi::mux::Muxer;
use crate::ring::EncodedPacket;
use crate::{MediaError, Result};

/// A keyframe gap longer than this needs a proxy.
pub const MAX_KEYFRAME_GAP_S: f64 = 2.1;
/// Probe this much of the clip.
pub const PROBE_SECONDS: f64 = 8.0;
/// Clips longer than this keep their original.
pub const MAX_SOURCE_SECONDS: f64 = 300.0;
/// Proxy keyframe interval in frames.
pub const PROXY_GOP: u32 = 30;
/// Proxy video bitrate.
pub const PROXY_BITRATE_KBPS: u32 = 16_000;
/// Cache limits.
pub const MAX_ENTRIES: usize = 8;
/// Cache byte budget (2 GiB).
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One video packet's timing from the probe.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyframeSample {
    /// Presentation time in seconds.
    pub pts_s: f64,
    /// Whether the packet is a keyframe.
    pub keyframe: bool,
}

/// The probe's findings.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    /// Packets within the probed window, in order.
    pub samples: Vec<KeyframeSample>,
    /// Container duration.
    pub duration_s: f64,
    /// PQ or HLG transfer (kept on the original, colour-managed path).
    pub hdr: bool,
}

/// `<home>/.fthr/playback`.
pub fn cache_dir(home: &Path) -> PathBuf {
    home.join(".fthr").join("playback")
}

/// The cache file for `source`: sha256 of its identity (device, inode,
/// size, mtime) and the proxy recipe version.
pub fn cache_path(cache_dir: &Path, source: &Path) -> Result<PathBuf> {
    let meta = std::fs::metadata(source)?;
    let identity = format!(
        "({}, {}, {}, {})|gop30-v2",
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime() as i128 * 1_000_000_000 + i128::from(meta.mtime_nsec())
    );
    let digest = Sha256::digest(identity.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(cache_dir.join(format!("{hex}.mp4")))
}

/// Measured keyframe gaps over 2.1 s (any packet that far after the last
/// keyframe counts); packets before the first keyframe are ignored.
pub fn needs_seek_proxy(samples: &[KeyframeSample]) -> bool {
    let mut last_keyframe: Option<f64> = None;
    for s in samples {
        if !s.pts_s.is_finite() {
            continue;
        }
        if let Some(k) = last_keyframe
            && s.pts_s - k > MAX_KEYFRAME_GAP_S
        {
            return true;
        }
        if s.keyframe {
            last_keyframe = Some(s.pts_s);
        }
    }
    false
}

/// Reads the first `seconds` of video packets without decoding them.
pub fn probe(source: &Path, seconds: f64) -> Result<Probe> {
    crate::init()?;
    let mut ictx = format::input(source)?;
    let stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| MediaError::Invalid("no video stream".into()))?;
    let index = stream.index();
    let tb = stream.time_base();
    let time_base = f64::from(tb.numerator()) / f64::from(tb.denominator());
    let hdr = codec::context::Context::from_parameters(stream.parameters())
        .ok()
        .and_then(|c| c.decoder().video().ok())
        .map(|d| {
            matches!(
                d.color_transfer_characteristic(),
                TransferCharacteristic::SMPTE2084 | TransferCharacteristic::ARIB_STD_B67
            )
        })
        .unwrap_or(false);
    let duration_s = if ictx.duration() > 0 {
        ictx.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
    } else {
        stream.duration().max(0) as f64 * time_base
    };
    let mut samples = Vec::new();
    for (s, packet) in ictx.packets() {
        if s.index() != index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        let pts_s = pts as f64 * time_base;
        if pts_s > seconds {
            break;
        }
        samples.push(KeyframeSample {
            pts_s,
            keyframe: packet.is_key(),
        });
    }
    Ok(Probe {
        samples,
        duration_s,
        hdr,
    })
}

/// The cached proxy if it exists, else the source; builds the proxy when
/// the probe says it is needed and the build is not cancelled.
pub fn prepare(cache_dir: &Path, source: &Path, cancel: &AtomicBool) -> PathBuf {
    let Ok(target) = cache_path(cache_dir, source) else {
        return source.to_path_buf();
    };
    if target.is_file() {
        // A published proxy was validated before its rename; a truncated or
        // unreadable file is a crash leftover and is rebuilt.
        if probe(&target, 1.0).is_ok_and(|p| p.duration_s > 0.0) {
            return target;
        }
        let _ = std::fs::remove_file(&target);
    }
    if cancel.load(Ordering::Relaxed) {
        return source.to_path_buf();
    }
    prune(cache_dir, MAX_ENTRIES, MAX_BYTES);
    let Ok(p) = probe(source, PROBE_SECONDS) else {
        return source.to_path_buf();
    };
    if p.hdr
        || !p.duration_s.is_finite()
        || p.duration_s <= 0.0
        || p.duration_s > MAX_SOURCE_SECONDS
    {
        return source.to_path_buf();
    }
    if !needs_seek_proxy(&p.samples) {
        return source.to_path_buf();
    }
    match build(cache_dir, source, &target, p.duration_s, cancel) {
        Ok(()) => {
            prune(cache_dir, MAX_ENTRIES, MAX_BYTES);
            target
        }
        Err(e) => {
            tracing::info!(error = %e, source = %source.display(), "playback proxy not built");
            let _ = std::fs::remove_file(&target);
            source.to_path_buf()
        }
    }
}

/// Removes the proxy for `source` when `cached` is it.
pub fn discard(cache_dir: &Path, source: &Path, cached: &Path) {
    if let Ok(expected) = cache_path(cache_dir, source)
        && expected == cached
    {
        let _ = std::fs::remove_file(expected);
    }
}

/// Deletes partials older than an hour (a dead build's leftovers; fresh
/// ones are builds in progress), anything else that is not `<64 hex>.mp4`,
/// and the oldest entries beyond `max_entries` or `max_bytes`.
pub fn prune(cache_dir: &Path, max_entries: usize, max_bytes: u64) {
    let Ok(rd) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(m) = e.metadata() else {
            continue;
        };
        let modified = m.modified().unwrap_or(std::time::UNIX_EPOCH);
        if name.contains(".partial") {
            let age = now
                .duration_since(modified)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age > STALE_PARTIAL_S {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let well_formed = name.len() == 68
            && name.ends_with(".mp4")
            && name[..64].chars().all(|c| c.is_ascii_hexdigit());
        if !well_formed {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        entries.push((path, m.len(), modified));
    }
    entries.sort_by_key(|a| std::cmp::Reverse(a.2));
    let mut total = 0u64;
    for (i, (path, size, _)) in entries.iter().enumerate() {
        total += size;
        if i >= max_entries || total > max_bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Partials older than this are leftovers of a dead build.
const STALE_PARTIAL_S: u64 = 3600;

/// Decodes the source and re-encodes the video with a 30-frame GOP through
/// the software H.264 encoder, streaming packets into a uniquely named
/// partial that is validated and then renamed into place. The proxy is
/// video-only: the editor always decodes audio from the original.
fn build(
    cache_dir: &Path,
    source: &Path,
    target: &Path,
    duration_s: f64,
    cancel: &AtomicBool,
) -> Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = cache_dir.join(format!("{stem}.{}-{nanos}.partial.mp4", std::process::id()));
    let result = build_into(source, &temp, cancel).and_then(|()| validate(&temp, duration_s));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    if std::fs::rename(&temp, target).is_err() {
        // Another build won the race (or the directory vanished); ours goes.
        let _ = std::fs::remove_file(&temp);
        if !target.is_file() {
            return Err(MediaError::Invalid("could not publish the proxy".into()));
        }
    }
    Ok(())
}

/// Size and duration checks, before anything can see the file under its
/// final name.
fn validate(temp: &Path, duration_s: f64) -> Result<()> {
    let size = std::fs::metadata(temp)?.len();
    if size > MAX_BYTES {
        return Err(MediaError::Invalid(
            "proxy larger than the cache budget".into(),
        ));
    }
    let check = probe(temp, 1.0)?;
    if (check.duration_s - duration_s).abs() > 0.2 {
        return Err(MediaError::Invalid(format!(
            "proxy duration {:.2}s differs from the source {duration_s:.2}s",
            check.duration_s
        )));
    }
    Ok(())
}

/// The muxer is opened on the first packet (the encoder's extradata is
/// only known then).
struct ProxyMux {
    mux: Option<(Muxer, usize)>,
    written: usize,
    fps: u32,
    width: u32,
    height: u32,
}

impl ProxyMux {
    fn write(
        &mut self,
        temp: &Path,
        encoder: &Encoder,
        out: &mut Vec<EncodedPacket>,
    ) -> Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        if self.mux.is_none() {
            let mut m = Muxer::new(temp, false)?;
            m.set_metadata("comment", "fthr-playback-proxy")?;
            m.set_metadata("fthr_frame_rate", &format!("{}/1", self.fps))?;
            let index = m.add_video_stream(
                encoder.codec_id().into(),
                self.width,
                self.height,
                self.fps,
                self.fps as i32,
                i64::from(PROXY_BITRATE_KBPS) * 1000,
                &encoder.extradata(),
            )?;
            m.write_header()?;
            self.mux = Some((m, index));
        }
        let (m, index) = self.mux.as_mut().expect("muxer");
        for p in out.drain(..) {
            if p.data.is_empty() {
                continue;
            }
            m.write_packet(
                *index,
                &p.data.bytes()?,
                p.pts,
                p.dts,
                1,
                p.keyframe,
                (1, self.fps as i32),
            )?;
            self.written += 1;
        }
        Ok(())
    }
}

fn build_into(source: &Path, temp: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut dec = VideoDecoder::open(source)?;
    let info = dec.info().clone();
    let fps = info.fps.round().max(1.0) as u32;
    let cfg = EncoderConfig {
        src_width: info.width,
        src_height: info.height,
        enc_width: info.width,
        enc_height: info.height,
        fps,
        bitrate_kbps: PROXY_BITRATE_KBPS,
        codec: fthrust_core::capture_config::Codec::H264,
        encoder: fthrust_core::capture_config::EncoderPref::Software,
        preset: 1,
    };
    let mut encoder = Encoder::open_with_gop(&cfg, PROXY_GOP)?;
    let mut sink = ProxyMux {
        mux: None,
        written: 0,
        fps,
        width: info.width,
        height: info.height,
    };
    let mut out = Vec::new();
    let mut bgra: Vec<u8> = Vec::new();
    while let Some(frame) = dec.next_frame()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Invalid("cancelled".into()));
        }
        bgra.clear();
        bgra.extend(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]]),
        );
        let pts_ns = frame.pts_ms as i64 * 1_000_000;
        encoder.encode(
            &bgra,
            (frame.width * 4) as usize,
            frame.width,
            frame.height,
            pts_ns,
            &mut out,
        )?;
        sink.write(temp, &encoder, &mut out)?;
    }
    encoder.flush(&mut out)?;
    sink.write(temp, &encoder, &mut out)?;
    let Some((mut m, _)) = sink.mux else {
        return Err(MediaError::Invalid("no video packets".into()));
    };
    if sink.written == 0 {
        return Err(MediaError::Invalid("no video packets".into()));
    }
    m.finish()?;
    Ok(())
}

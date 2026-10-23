//! The webcam recorder (`core/camera_recorder.py`): a capture thread keeps
//! the last 90 s of the camera as MJPEG packets stamped with their capture
//! instant, and writes a window of them as an MJPEG Matroska file whose
//! timestamps place each frame on the saved clip's timeline (the
//! reference wrote every frame at a fixed 30 fps).
//!
//! Capture goes through libavdevice: `v4l2` for webcams (MJPEG requested,
//! raw formats encoded to MJPEG), `lavfi` for the synthetic test camera.

mod devices;

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ffmpeg_next as ff;

pub use devices::{Device, devices_from, list_devices};

/// `KEEP_SECONDS`.
pub const KEEP: Duration = Duration::from_secs(90);

/// How long [`CameraRecorder::stop`] waits for the capture thread.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// MJPEG quality as an FFmpeg `qscale` (about JPEG quality 85).
const QSCALE: i32 = 3;

/// Where frames come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// libavdevice input format name.
    pub format: &'static str,
    /// Device path or lavfi graph.
    pub url: String,
    /// Input options.
    pub options: Vec<(String, String)>,
}

impl Source {
    /// A V4L2 webcam, MJPEG requested.
    pub fn v4l2(path: &Path) -> Source {
        Source {
            format: "v4l2",
            url: path.to_string_lossy().into_owned(),
            options: vec![("input_format".into(), "mjpeg".into())],
        }
    }

    /// A paced test pattern (tests and the bench).
    pub fn synthetic(width: u32, height: u32, fps: u32) -> Source {
        Source {
            format: "lavfi",
            url: format!("testsrc=size={width}x{height}:rate={fps},realtime"),
            options: Vec::new(),
        }
    }
}

struct Entry {
    at: Instant,
    jpeg: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Ring {
    entries: VecDeque<Entry>,
    size: Option<(u32, u32)>,
}

struct Shared {
    ring: Mutex<Ring>,
    keep: Duration,
    stop: AtomicBool,
    running: AtomicBool,
    frames: AtomicU64,
    latest: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
}

impl Shared {
    fn ring(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A running camera.
pub struct CameraRecorder {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn input_format(name: &str) -> Option<ff::format::Input> {
    ff::device::input::video().find_map(|f| match f {
        ff::Format::Input(i) if i.name().split(',').any(|n| n == name) => Some(i),
        _ => None,
    })
}

fn open(source: &Source, with_options: bool) -> Result<ff::format::context::Input, String> {
    let fmt = input_format(source.format)
        .ok_or_else(|| format!("{} input is unavailable", source.format))?;
    let mut opts = ff::Dictionary::new();
    if with_options {
        for (k, v) in &source.options {
            opts.set(k, v);
        }
    }
    Ok(
        ff::format::open_with(&source.url, &ff::Format::Input(fmt), opts)
            .map_err(|e| e.to_string())?
            .input(),
    )
}

/// Turns the input's packets into JPEG frames: MJPEG packets as they are,
/// anything else decoded, converted and encoded.
enum Jpeg {
    Copy,
    Encode {
        decoder: ff::decoder::Video,
        scaler: Option<ff::software::scaling::Context>,
        encoder: Option<ff::encoder::Video>,
        next_pts: i64,
    },
}

impl Jpeg {
    fn frames(&mut self, packet: &ff::Packet, out: &mut Vec<Vec<u8>>) -> Result<(), String> {
        match self {
            Jpeg::Copy => {
                if let Some(d) = packet.data() {
                    out.push(d.to_vec());
                }
                Ok(())
            }
            Jpeg::Encode {
                decoder,
                scaler,
                encoder,
                next_pts,
            } => {
                decoder.send_packet(packet).map_err(|e| e.to_string())?;
                let mut frame = ff::frame::Video::empty();
                while decoder.receive_frame(&mut frame).is_ok() {
                    let (w, h) = (frame.width(), frame.height());
                    if encoder.is_none() {
                        let codec =
                            ff::encoder::find(ff::codec::Id::MJPEG).ok_or("no MJPEG encoder")?;
                        let mut ctx = ff::codec::context::Context::new_with_codec(codec)
                            .encoder()
                            .video()
                            .map_err(|e| e.to_string())?;
                        ctx.set_width(w);
                        ctx.set_height(h);
                        ctx.set_format(ff::format::Pixel::YUVJ420P);
                        ctx.set_time_base(ff::Rational::new(1, 30));
                        ctx.set_flags(ff::codec::Flags::QSCALE);
                        ctx.set_global_quality(QSCALE * ff::ffi::FF_QP2LAMBDA);
                        *encoder = Some(ctx.open_as(codec).map_err(|e| e.to_string())?);
                        *scaler = Some(
                            ff::software::scaling::Context::get(
                                frame.format(),
                                w,
                                h,
                                ff::format::Pixel::YUVJ420P,
                                w,
                                h,
                                ff::software::scaling::Flags::BILINEAR,
                            )
                            .map_err(|e| e.to_string())?,
                        );
                    }
                    let mut yuv = ff::frame::Video::empty();
                    scaler
                        .as_mut()
                        .expect("scaler")
                        .run(&frame, &mut yuv)
                        .map_err(|e| e.to_string())?;
                    yuv.set_pts(Some(*next_pts));
                    *next_pts += 1;
                    let enc = encoder.as_mut().expect("encoder");
                    enc.send_frame(&yuv).map_err(|e| e.to_string())?;
                    let mut pkt = ff::Packet::empty();
                    while enc.receive_packet(&mut pkt).is_ok() {
                        if let Some(d) = pkt.data() {
                            out.push(d.to_vec());
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

impl CameraRecorder {
    /// Opens `source` and starts recording (the reference's blocking
    /// `start`); the error says why the camera could not open.
    pub fn start(source: Source) -> Result<CameraRecorder, String> {
        CameraRecorder::start_keeping(source, KEEP)
    }

    /// [`CameraRecorder::start`] with another history length (tests).
    pub fn start_keeping(source: Source, keep: Duration) -> Result<CameraRecorder, String> {
        crate::init().map_err(|e| e.to_string())?;
        let ictx = open(&source, true).or_else(|first| {
            if source.options.is_empty() {
                Err(first)
            } else {
                // The camera may not offer MJPEG: take its default format.
                open(&source, false).map_err(|_| first)
            }
        })?;
        let stream = ictx
            .streams()
            .best(ff::media::Type::Video)
            .ok_or("the camera has no video stream")?;
        let index = stream.index();
        let params = stream.parameters();
        let copy = params.id() == ff::codec::Id::MJPEG;
        // Fails here, on the caller's thread, if the format cannot decode.
        let decoder = if copy {
            None
        } else {
            Some(
                ff::codec::context::Context::from_parameters(params)
                    .and_then(|c| c.decoder().video())
                    .map_err(|e| e.to_string())?,
            )
        };
        let size = ff::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().video())
            .map(|d| (d.width(), d.height()))
            .ok();
        let shared = Arc::new(Shared {
            ring: Mutex::new(Ring {
                entries: VecDeque::new(),
                size,
            }),
            keep,
            stop: AtomicBool::new(false),
            running: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            latest: Mutex::new(None),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("fthr-camera".into())
            .spawn(move || {
                // The scaler is not `Send`: the converter lives on this thread.
                let jpeg = match decoder {
                    None => Jpeg::Copy,
                    Some(decoder) => Jpeg::Encode {
                        decoder,
                        scaler: None,
                        encoder: None,
                        next_pts: 0,
                    },
                };
                capture(ictx, index, jpeg, &s)
            })
            .map_err(|e| e.to_string())?;
        Ok(CameraRecorder {
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Recording (false after a read error or [`CameraRecorder::stop`]).
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    /// Frames received since start.
    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// Time between the oldest and newest kept frame.
    pub fn span(&self) -> Option<Duration> {
        let ring = self.shared.ring();
        Some(ring.entries.back()?.at - ring.entries.front()?.at)
    }

    /// The newest frame as JPEG with a sequence number (the preview).
    pub fn latest_jpeg(&self) -> Option<(u64, Arc<Vec<u8>>)> {
        self.shared
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Stops the thread and clears the ring.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = thread {
            // A camera that stopped delivering blocks its read; wait at most
            // two seconds (the reference's join timeout), then let the
            // thread end on its own when the read returns.
            let end = Instant::now() + STOP_WAIT;
            while !t.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            } else {
                tracing::warn!("camera read did not return; capture thread detached");
            }
        }
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.ring().entries.clear();
        *self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The frames of `[end − max_s, end]`, kept independently of the ring
    /// (taken at save time; the burn may run much later). `None` when
    /// there are none.
    pub fn snapshot(&self, end: Instant, max_s: f64) -> Option<CameraSnapshot> {
        let start = end
            .checked_sub(Duration::from_secs_f64(max_s.max(0.0)))
            .unwrap_or(end);
        let ring = self.shared.ring();
        let frames: Vec<(Instant, Arc<Vec<u8>>)> = ring
            .entries
            .iter()
            .filter(|e| e.at >= start && e.at <= end)
            .map(|e| (e.at, e.jpeg.clone()))
            .collect();
        let size = ring.size?;
        (!frames.is_empty()).then_some(CameraSnapshot { frames, size })
    }

    /// Writes the frames of `[end − duration_s, end]` to `path` (MJPEG in
    /// Matroska), timed from the window start. Returns the frame count.
    pub fn write_segment(
        &self,
        end: Instant,
        duration_s: f64,
        path: &Path,
    ) -> Result<usize, String> {
        self.snapshot(end, duration_s)
            .ok_or("no camera frames in the clip window")?
            .write(end, duration_s, path)
    }
}

/// Camera frames taken from the ring at save time.
#[derive(Clone)]
pub struct CameraSnapshot {
    frames: Vec<(Instant, Arc<Vec<u8>>)>,
    size: (u32, u32),
}

impl CameraSnapshot {
    /// Writes the frames of `[end − duration_s, end]` (the clip's real
    /// length) as MJPEG Matroska timed from that start; the frame count.
    pub fn write(&self, end: Instant, duration_s: f64, path: &Path) -> Result<usize, String> {
        let start = end
            .checked_sub(Duration::from_secs_f64(duration_s.max(0.0)))
            .unwrap_or(end);
        let frames: Vec<(Duration, Arc<Vec<u8>>)> = self
            .frames
            .iter()
            .filter(|(at, _)| *at >= start && *at <= end)
            .map(|(at, j)| (*at - start, j.clone()))
            .collect();
        if frames.is_empty() {
            return Err("no camera frames in the clip window".into());
        }
        write_mjpeg(path, self.size.0, self.size.1, &frames).map_err(|e| e.to_string())?;
        Ok(frames.len())
    }
}

impl std::fmt::Debug for CameraRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraRecorder")
            .field("running", &self.is_running())
            .field("frames", &self.frames())
            .finish()
    }
}

impl Drop for CameraRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture(mut ictx: ff::format::context::Input, index: usize, mut jpeg: Jpeg, shared: &Shared) {
    let mut out = Vec::new();
    for (stream, packet) in ictx.packets() {
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        if stream.index() != index {
            continue;
        }
        // Stamped when the read returns, as the reference does.
        let at = Instant::now();
        out.clear();
        if let Err(e) = jpeg.frames(&packet, &mut out) {
            tracing::warn!(error = %e, "camera frame dropped");
            continue;
        }
        for data in out.drain(..) {
            let data = Arc::new(data);
            let seq = shared.frames.fetch_add(1, Ordering::Relaxed) + 1;
            *shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some((seq, data.clone()));
            let mut ring = shared.ring();
            ring.entries.push_back(Entry { at, jpeg: data });
            while ring
                .entries
                .front()
                .is_some_and(|e| at.duration_since(e.at) > shared.keep)
            {
                ring.entries.pop_front();
            }
        }
    }
    // A read error or end of stream: the camera is gone.
    shared.running.store(false, Ordering::Relaxed);
}

fn write_mjpeg(
    path: &Path,
    w: u32,
    h: u32,
    frames: &[(Duration, Arc<Vec<u8>>)],
) -> Result<(), ff::Error> {
    let mut octx = ff::format::output(&path)?;
    let codec = ff::encoder::find(ff::codec::Id::MJPEG);
    {
        let mut ctx =
            ff::codec::context::Context::new_with_codec(codec.ok_or(ff::Error::EncoderNotFound)?)
                .encoder()
                .video()?;
        ctx.set_width(w);
        ctx.set_height(h);
        ctx.set_format(ff::format::Pixel::YUVJ420P);
        let mut ost = octx.add_stream(codec)?;
        ost.set_parameters(&ctx);
        ost.set_time_base(ff::Rational::new(1, 1000));
    }
    octx.write_header()?;
    let tb = octx
        .stream(0)
        .map(|s| s.time_base())
        .unwrap_or(ff::Rational::new(1, 1000));
    let mut last = -1i64;
    for (t, data) in frames {
        let ms = (t.as_millis() as i64).max(last + 1);
        last = ms;
        let mut pkt = ff::Packet::copy(data);
        pkt.set_pts(Some(ms));
        pkt.set_dts(Some(ms));
        pkt.set_flags(ff::packet::Flags::KEY);
        pkt.set_stream(0);
        pkt.rescale_ts(ff::Rational::new(1, 1000), tb);
        pkt.write_interleaved(&mut octx)?;
    }
    octx.write_trailer()
}

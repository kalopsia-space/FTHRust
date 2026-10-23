//! One export attempt: decode the inputs the plan names, run its filter
//! graph, encode and mux; or copy packets when the plan says so.
//!
//! The graph keeps the reference's labels: inputs `[0:v]` and `[0:a:N]`
//! become `buffer`/`abuffer` sources of those names; an input used more
//! than once is split first (the CLI does that implicitly). Each output
//! label gets a sink adapter: `fps` (the CLI's constant-frame-rate MP4
//! output) and the encoder's pixel format for video, `aformat` for audio,
//! plus `trim`/`atrim` for a single range's `-t`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use ff::Rescale;
use ffmpeg_next as ff;
use fthrust_core::export::graph::{AudioOut, MapRef, VideoOut};

use super::encode::{h264_config, is_hardware, status_for_encoder};
use super::{INACTIVITY_S, Job, Quality, Stop};
use crate::encoder::Encoder;
use crate::ring::EncodedPacket;

/// One output stream.
enum Out {
    /// Copy packets of an input stream.
    Copy { input: usize, ost: usize },
    /// Encode what a graph sink produces.
    Video {
        sink: String,
        ost: usize,
        enc: VideoEnc,
    },
    Audio {
        sink: String,
        ost: usize,
        enc: ff::encoder::Audio,
        eof: bool,
    },
}

enum VideoEnc {
    H264 { enc: Box<Encoder>, fps: u32 },
    Ffv1(ff::encoder::Video),
}

/// Facts about the input collected before reading packets.
struct Inputs {
    video: Option<usize>,
    /// Audio stream indexes in `0:a:N` order.
    audio: Vec<usize>,
    fps: u32,
}

fn inputs(ictx: &ff::format::context::Input) -> Inputs {
    let video = ictx
        .streams()
        .best(ff::media::Type::Video)
        .map(|s| s.index());
    let audio = ictx
        .streams()
        .filter(|s| s.parameters().medium() == ff::media::Type::Audio)
        .map(|s| s.index())
        .collect();
    let fps = video
        .and_then(|i| ictx.stream(i))
        .map(|s| {
            let r = s.avg_frame_rate();
            if r.numerator() > 0 && r.denominator() > 0 {
                (f64::from(r.numerator()) / f64::from(r.denominator())).round() as u32
            } else {
                30
            }
        })
        .unwrap_or(30)
        .clamp(1, 240);
    Inputs { video, audio, fps }
}

/// Labels like `0:v` and `0:a:1` the graph reads, with their use counts.
fn input_uses(graph: &str) -> BTreeMap<String, usize> {
    let mut uses = BTreeMap::new();
    let mut rest = graph;
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            break;
        };
        let label = &rest[start + 1..start + end];
        if label == "0:v" || label.starts_with("0:a:") {
            *uses.entry(label.to_owned()).or_insert(0) += 1;
        }
        rest = &rest[start + end + 1..];
    }
    uses
}

/// Splits inputs used more than once: `[0:v]` ×3 becomes `[0:v]split=3…`.
fn split_shared_inputs(graph: &str, uses: &BTreeMap<String, usize>) -> String {
    let mut g = graph.to_owned();
    let mut prefix = Vec::new();
    for (label, &n) in uses {
        if n < 2 {
            continue;
        }
        let tag = format!("[{label}]");
        let mut out = String::new();
        let mut i = 0;
        let mut rest = g.as_str();
        while let Some(pos) = rest.find(&tag) {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("[{label}#{i}]"));
            i += 1;
            rest = &rest[pos + tag.len()..];
        }
        out.push_str(rest);
        g = out;
        let filter = if label == "0:v" { "split" } else { "asplit" };
        let outs: String = (0..n).map(|k| format!("[{label}#{k}]")).collect();
        prefix.push(format!("[{label}]{filter}={n}{outs}"));
    }
    if prefix.is_empty() {
        g
    } else {
        format!("{};{g}", prefix.join(";"))
    }
}

/// Slice threads with FFmpeg's automatic count (frame threads delay
/// frames past the graph's end-of-range handling).
fn decoder_threads() -> ff::codec::threading::Config {
    ff::codec::threading::Config {
        kind: ff::codec::threading::Type::Slice,
        count: 0,
    }
}

/// A source refuses frames once everything downstream has finished (the
/// last range's trim ended): that input is done, not broken.
fn feed(r: Result<(), ff::Error>) -> Result<(), Stop> {
    match r {
        Err(ff::Error::Eof) => Ok(()),
        other => Ok(other?),
    }
}

/// Seconds of a timestamp in a time base.
fn secs(ts: i64, tb: ff::Rational) -> f64 {
    ts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator().max(1))
}

/// The `-filter_complex` plus the adapters this attempt runs, the sink
/// names and which outputs copy.
struct Assembly {
    graph: Option<String>,
    video_sink: Option<String>,
    audio_sinks: Vec<String>,
    copy_video: bool,
    copy_audio: Vec<usize>,
}

fn assemble(job: &Job, inp: &Inputs) -> Assembly {
    let p = &job.plan;
    let mut parts: Vec<String> = p.filter_graph.iter().cloned().collect();
    let trim = p.duration_s.map(|d| format!("trim=duration={d:.6},"));
    let atrim = p.duration_s.map(|d| format!("atrim=duration={d:.6},"));
    let pix = if job.quality == Quality::Lossless {
        "yuv420p"
    } else {
        "bgra"
    };
    let sample_fmt = if job.quality == Quality::Lossless {
        "s16"
    } else {
        "fltp"
    };
    let mut video_sink = None;
    let mut copy_video = false;
    if inp.video.is_some() {
        // Lossless always re-encodes (FFV1 and FLAC in Matroska).
        let video_out = if job.quality == Quality::Lossless {
            VideoOut::Encode
        } else {
            p.video
        };
        let label = match (&p.video_map, video_out) {
            (MapRef::Label(l), _) => Some(l.clone()),
            (_, VideoOut::Encode) => {
                parts.push("[0:v]null[__v]".into());
                Some("__v".into())
            }
            (_, VideoOut::Copy) => {
                copy_video = true;
                None
            }
        };
        if let Some(l) = label {
            parts.push(format!(
                "[{l}]{}fps=fps={},format={pix}[__vsink]",
                trim.clone().unwrap_or_default(),
                inp.fps
            ));
            video_sink = Some("__vsink".into());
        }
    }
    let mut audio_sinks = Vec::new();
    let mut copy_audio = Vec::new();
    let adapter = |l: &str, i: usize| {
        format!(
            "[{l}]{}aformat=sample_fmts={sample_fmt}:sample_rates=48000:channel_layouts=stereo[__asink{i}]",
            atrim.clone().unwrap_or_default()
        )
    };
    let audio_out = match p.audio {
        AudioOut::Copy if job.quality == Quality::Lossless => AudioOut::Encode { kbps: 0 },
        other => other,
    };
    match (&p.audio_map, audio_out) {
        (_, AudioOut::None) | (None, _) => {}
        (Some(MapRef::Label(l)), _) => {
            parts.push(adapter(l, 0));
            audio_sinks.push("__asink0".into());
        }
        (Some(_), AudioOut::Copy) => copy_audio = inp.audio.clone(),
        (Some(_), AudioOut::Encode { .. }) => {
            let af = p.audio_filter.clone().unwrap_or_else(|| "anull".into());
            for k in 0..inp.audio.len() {
                parts.push(format!("[0:a:{k}]{af}[__a{k}]"));
                parts.push(adapter(&format!("__a{k}"), k));
                audio_sinks.push(format!("__asink{k}"));
            }
        }
    }
    let graph = (!parts.is_empty()).then(|| parts.join(";"));
    Assembly {
        graph,
        video_sink,
        audio_sinks,
        copy_video,
        copy_audio,
    }
}

struct Decoders {
    video: Option<ff::decoder::Video>,
    audio: BTreeMap<usize, (String, ff::decoder::Audio)>,
}

/// Builds the graph with a source per used input and a sink per output.
fn build_graph(
    graph: &str,
    ictx: &ff::format::context::Input,
    inp: &Inputs,
    sinks: &[String],
) -> Result<(ff::filter::Graph, Decoders), Stop> {
    let uses = input_uses(graph);
    let spec = split_shared_inputs(graph, &uses);
    let mut g = ff::filter::Graph::new();
    let mut decoders = Decoders {
        video: None,
        audio: BTreeMap::new(),
    };
    let buffer = ff::filter::find("buffer").ok_or(Stop::Failed("buffer filter missing".into()))?;
    let abuffer =
        ff::filter::find("abuffer").ok_or(Stop::Failed("abuffer filter missing".into()))?;
    for label in uses.keys() {
        if label == "0:v" {
            let index = inp
                .video
                .ok_or(Stop::Failed("the clip has no video stream".into()))?;
            let stream = ictx
                .stream(index)
                .ok_or(Stop::Failed("video stream missing".into()))?;
            let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
            ctx.set_threading(decoder_threads());
            let dec = ctx.decoder().video()?;
            let tb = stream.time_base();
            let sar = dec.aspect_ratio();
            let (sn, sd) = if sar.numerator() > 0 {
                (sar.numerator(), sar.denominator())
            } else {
                (1, 1)
            };
            let fmt: ff::ffi::AVPixelFormat = dec.format().into();
            let args = format!(
                "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={sn}/{sd}",
                dec.width(),
                dec.height(),
                fmt as i32,
                tb.numerator(),
                tb.denominator()
            );
            g.add(&buffer, label, &args)?;
            decoders.video = Some(dec);
        } else {
            let k: usize = label[4..]
                .parse()
                .map_err(|_| Stop::Failed(format!("bad input {label}")))?;
            let index = *inp
                .audio
                .get(k)
                .ok_or_else(|| Stop::Failed(format!("the clip has no audio stream {k}")))?;
            let stream = ictx
                .stream(index)
                .ok_or(Stop::Failed("audio stream missing".into()))?;
            let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
            ctx.set_threading(decoder_threads());
            let dec = ctx.decoder().audio()?;
            let tb = stream.time_base();
            let layout = format!("0x{:x}", dec.channel_layout().bits());
            let layout = if layout == "0x0" {
                "stereo".to_owned()
            } else {
                layout
            };
            let args = format!(
                "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout={layout}",
                tb.numerator(),
                tb.denominator(),
                dec.rate(),
                dec.format().name(),
            );
            g.add(&abuffer, label, &args)?;
            decoders.audio.insert(index, (label.clone(), dec));
        }
    }
    let vsink = ff::filter::find("buffersink").ok_or(Stop::Failed("buffersink missing".into()))?;
    let asink =
        ff::filter::find("abuffersink").ok_or(Stop::Failed("abuffersink missing".into()))?;
    for s in sinks {
        if s.starts_with("__v") {
            g.add(&vsink, s, "")?;
        } else {
            g.add(&asink, s, "")?;
        }
    }
    {
        let mut names = uses.keys();
        let first = names
            .next()
            .ok_or(Stop::Failed("the graph reads no input".into()))?;
        let mut parser = g.output(first, 0)?;
        for n in names {
            parser = parser.output(n, 0)?;
        }
        for s in sinks {
            parser = parser.input(s, 0)?;
        }
        parser.parse(&spec)?;
    }
    g.validate()?;
    Ok((g, decoders))
}

/// `(width, height, time_base)` of a configured buffersink.
fn sink_props(g: &mut ff::filter::Graph, name: &str) -> (u32, u32, ff::Rational) {
    let mut ctx = g.get(name).expect("sink exists");
    crate::ffi::export::buffersink_props(&mut ctx)
}

/// Fills an H.264 stream's codec parameters from the capture encoder.
fn set_h264_parameters(ost: &mut ff::format::stream::StreamMut<'_>, enc: &Encoder, fps: u32) {
    crate::ffi::export::set_h264_stream(
        ost,
        enc.codec_id(),
        enc.width(),
        enc.height(),
        fps,
        &enc.extradata(),
    );
}

fn write_h264(
    octx: &mut ff::format::context::Output,
    ost: usize,
    fps: u32,
    packets: &mut Vec<EncodedPacket>,
) -> Result<(), Stop> {
    let tb = octx
        .stream(ost)
        .map(|s| s.time_base())
        .unwrap_or(ff::Rational::new(1, fps as i32));
    for p in packets.drain(..) {
        let mut pkt = ff::Packet::copy(&p.data);
        pkt.set_pts(Some(p.pts));
        pkt.set_dts(Some(p.dts));
        pkt.set_duration(1);
        if p.keyframe {
            pkt.set_flags(ff::packet::Flags::KEY);
        }
        pkt.set_stream(ost);
        pkt.rescale_ts(ff::Rational::new(1, fps as i32), tb);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

/// Writes every packet the encoder has ready.
fn drain_packets(
    mut receive: impl FnMut(&mut ff::Packet) -> bool,
    octx: &mut ff::format::context::Output,
    ost: usize,
    enc_tb: ff::Rational,
) -> Result<(), Stop> {
    let tb = octx.stream(ost).map(|s| s.time_base()).unwrap_or(enc_tb);
    let mut pkt = ff::Packet::empty();
    while receive(&mut pkt) {
        pkt.set_stream(ost);
        pkt.rescale_ts(enc_tb, tb);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

/// Runs one attempt into `staged`.
#[allow(clippy::too_many_arguments)]
pub(super) fn attempt(
    job: &Job,
    staged: &Path,
    software: bool,
    inject_failure: bool,
    cancel: &AtomicBool,
    on_status: &mut dyn FnMut(&str),
    on_progress: &mut dyn FnMut(f64),
) -> Result<(), Stop> {
    let mut ictx = ff::format::input(&job.source)?;
    let mut inp = inputs(&ictx);
    // A preset that lowers the rate: the sink adapter and the encoder use
    // the output rate, or `fps=` would duplicate frames back up.
    if let Some(f) = job.output_fps.filter(|f| *f > 0.0) {
        inp.fps = (f.round() as u32).clamp(1, 240);
    }
    let asm = assemble(job, &inp);
    let mut sinks: Vec<String> = asm.video_sink.iter().cloned().collect();
    sinks.extend(asm.audio_sinks.iter().cloned());
    let (mut graph, mut decoders) = match &asm.graph {
        Some(g) => {
            let (g, d) = build_graph(g, &ictx, &inp, &sinks)?;
            (Some(g), d)
        }
        None => (
            None,
            Decoders {
                video: None,
                audio: BTreeMap::new(),
            },
        ),
    };
    let mut octx = ff::format::output(&staged)?;
    let global_header = octx
        .format()
        .flags()
        .contains(ff::format::Flags::GLOBAL_HEADER);
    let mut outs: Vec<Out> = Vec::new();
    // Video.
    if asm.copy_video
        && let Some(v) = inp.video
    {
        let ist = ictx
            .stream(v)
            .ok_or(Stop::Failed("video stream missing".into()))?;
        let mut ost = octx.add_stream(ff::encoder::find(ff::codec::Id::None))?;
        ost.set_parameters(ist.parameters());
        crate::ffi::export::clear_codec_tag(&mut ost);
        ost.set_time_base(ist.time_base());
        outs.push(Out::Copy {
            input: v,
            ost: ost.index(),
        });
        on_status("Exporting · stream copy");
    }
    if let (Some(sink), Some(g)) = (&asm.video_sink, graph.as_mut()) {
        let (w, h, _) = sink_props(g, sink);
        let enc = if job.quality == Quality::Lossless {
            let codec = ff::encoder::find(ff::codec::Id::FFV1)
                .ok_or(Stop::Failed("the FFV1 encoder is missing".into()))?;
            let mut ctx = ff::codec::context::Context::new_with_codec(codec)
                .encoder()
                .video()?;
            ctx.set_width(w);
            ctx.set_height(h);
            ctx.set_format(ff::format::Pixel::YUV420P);
            ctx.set_time_base(ff::Rational::new(1, inp.fps as i32));
            if global_header {
                ctx.set_flags(ff::codec::Flags::GLOBAL_HEADER);
            }
            let mut opts = ff::Dictionary::new();
            opts.set("level", "3");
            opts.set("coder", "1");
            opts.set("context", "1");
            on_status(status_for_encoder("ffv1"));
            VideoEnc::Ffv1(ctx.open_as_with(codec, opts)?)
        } else {
            let cfg = h264_config(w, h, inp.fps, job.quality, job.video_bitrate_kbps, software);
            let enc = Encoder::open_with_gop(&cfg, 30).map_err(|e| Stop::Encoder {
                hardware: !software,
                message: e.to_string(),
            })?;
            if inject_failure {
                return Err(Stop::Encoder {
                    hardware: true,
                    message: "injected encoder failure".into(),
                });
            }
            on_status(status_for_encoder(enc.active_codec()));
            VideoEnc::H264 {
                enc: Box::new(enc),
                fps: inp.fps,
            }
        };
        let ost = match &enc {
            VideoEnc::H264 { enc, fps } => {
                let mut ost = octx.add_stream(ff::encoder::find(ff::codec::Id::H264))?;
                set_h264_parameters(&mut ost, enc, *fps);
                ost.index()
            }
            VideoEnc::Ffv1(e) => {
                let mut ost = octx.add_stream(ff::encoder::find(ff::codec::Id::FFV1))?;
                ost.set_parameters(e);
                ost.set_time_base(ff::Rational::new(1, inp.fps as i32));
                ost.index()
            }
        };
        outs.push(Out::Video {
            sink: sink.clone(),
            ost,
            enc,
        });
    }
    // Audio.
    for &a in &asm.copy_audio {
        let ist = ictx
            .stream(a)
            .ok_or(Stop::Failed("audio stream missing".into()))?;
        let mut ost = octx.add_stream(ff::encoder::find(ff::codec::Id::None))?;
        ost.set_parameters(ist.parameters());
        crate::ffi::export::clear_codec_tag(&mut ost);
        ost.set_time_base(ist.time_base());
        outs.push(Out::Copy {
            input: a,
            ost: ost.index(),
        });
    }
    let kbps = match job.plan.audio {
        AudioOut::Encode { kbps } => kbps,
        _ => 320,
    };
    for sink in &asm.audio_sinks {
        let lossless = job.quality == Quality::Lossless;
        let id = if lossless {
            ff::codec::Id::FLAC
        } else {
            ff::codec::Id::AAC
        };
        let codec =
            ff::encoder::find(id).ok_or(Stop::Failed(format!("the {id:?} encoder is missing")))?;
        let mut ctx = ff::codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        ctx.set_rate(48_000);
        ctx.set_channel_layout(ff::ChannelLayout::STEREO);
        ctx.set_format(if lossless {
            ff::format::Sample::I16(ff::format::sample::Type::Packed)
        } else {
            ff::format::Sample::F32(ff::format::sample::Type::Planar)
        });
        if !lossless {
            ctx.set_bit_rate(kbps as usize * 1000);
        }
        ctx.set_time_base(ff::Rational::new(1, 48_000));
        if global_header {
            ctx.set_flags(ff::codec::Flags::GLOBAL_HEADER);
        }
        let enc = ctx.open_as(codec)?;
        let frame_size = enc.frame_size();
        if let Some(g) = graph.as_mut()
            && frame_size > 0
            && let Some(mut c) = g.get(sink)
        {
            c.sink().set_frame_size(frame_size);
        }
        let mut ost = octx.add_stream(codec)?;
        ost.set_parameters(&enc);
        ost.set_time_base(ff::Rational::new(1, 48_000));
        outs.push(Out::Audio {
            sink: sink.clone(),
            ost: ost.index(),
            enc,
            eof: false,
        });
    }
    if outs.is_empty() {
        return Err(Stop::Failed("nothing to export".into()));
    }
    octx.write_header()?;
    // Timestamp handling: inputs start at 0 (minus the stream start time),
    // a single range is shifted by its seek point.
    let seek = job.plan.seek_s.unwrap_or(0.0);
    let limit = job.plan.duration_s;
    if seek > 0.0 {
        let ts = (seek * f64::from(ff::ffi::AV_TIME_BASE)) as i64;
        ictx.seek(ts, ..ts)?;
    } else if job.decode_from_s > 0.0 {
        // Several ranges: start at the keyframe before the first one; the
        // trims use source timestamps, so nothing is shifted.
        let ts = (job.decode_from_s * f64::from(ff::ffi::AV_TIME_BASE)) as i64;
        ictx.seek(ts, ..ts)?;
    }
    let inactivity = job.inactivity_s.unwrap_or(INACTIVITY_S);
    let starts: BTreeMap<usize, (i64, ff::Rational)> = ictx
        .streams()
        .map(|s| {
            let st = s.start_time();
            (
                s.index(),
                (
                    if st == ff::ffi::AV_NOPTS_VALUE { 0 } else { st },
                    s.time_base(),
                ),
            )
        })
        .collect();
    let mut copy_shift: BTreeMap<usize, i64> = BTreeMap::new();
    let mut last_output = Instant::now();
    let mut h264_packets: Vec<EncodedPacket> = Vec::new();
    let mut vframe = ff::frame::Video::empty();
    let mut aframe = ff::frame::Audio::empty();
    let mut video_eof = asm.video_sink.is_none();
    let expected = job.expected_duration_s.max(0.1);

    let pump = |graph: &mut Option<ff::filter::Graph>,
                outs: &mut Vec<Out>,
                octx: &mut ff::format::context::Output,
                h264_packets: &mut Vec<EncodedPacket>,
                video_eof: &mut bool,
                last_output: &mut Instant,
                on_progress: &mut dyn FnMut(f64)|
     -> Result<(), Stop> {
        let Some(g) = graph.as_mut() else {
            return Ok(());
        };
        for out in outs.iter_mut() {
            match out {
                Out::Video { sink, ost, enc } => loop {
                    let (_, _, tb) = sink_props(g, sink);
                    let mut ctx = g.get(sink).expect("sink");
                    let mut frame = ff::frame::Video::empty();
                    match ctx.sink().frame(&mut frame) {
                        Ok(()) => {
                            *last_output = Instant::now();
                            let t = secs(frame.pts().unwrap_or(0), tb);
                            on_progress((t / expected * 100.0).clamp(0.0, 99.0));
                            match enc {
                                VideoEnc::H264 { enc, fps } => {
                                    let ns = (t * 1e9) as i64;
                                    let (w, h) = (frame.width(), frame.height());
                                    let stride = frame.stride(0);
                                    let hw = is_hardware(enc.active_codec());
                                    enc.encode(frame.data(0), stride, w, h, ns, h264_packets)
                                        .map_err(|e| Stop::Encoder {
                                            hardware: hw,
                                            message: e.to_string(),
                                        })?;
                                    write_h264(octx, *ost, *fps, h264_packets)?;
                                }
                                VideoEnc::Ffv1(e) => {
                                    let etb = e.time_base();
                                    frame.set_pts(Some(
                                        (t * f64::from(etb.denominator())
                                            / f64::from(etb.numerator().max(1)))
                                        .round() as i64,
                                    ));
                                    e.send_frame(&frame)?;
                                    drain_packets(
                                        |p| e.receive_packet(p).is_ok(),
                                        octx,
                                        *ost,
                                        etb,
                                    )?;
                                }
                            }
                        }
                        Err(ff::Error::Eof) => {
                            *video_eof = true;
                            break;
                        }
                        Err(_) => break,
                    }
                },
                Out::Audio {
                    sink,
                    ost,
                    enc,
                    eof,
                } => loop {
                    let (_, _, sink_tb) = sink_props(g, sink);
                    let mut ctx = g.get(sink).expect("sink");
                    let mut frame = ff::frame::Audio::empty();
                    match ctx.sink().frame(&mut frame) {
                        Ok(()) => {
                            *last_output = Instant::now();
                            let etb = enc.time_base();
                            // The sink keeps the input's time base unless a
                            // filter resampled; the encoder counts samples.
                            let pts = frame.pts().map(|p| p.rescale(sink_tb, etb));
                            frame.set_pts(pts);
                            enc.send_frame(&frame)?;
                            drain_packets(|p| enc.receive_packet(p).is_ok(), octx, *ost, etb)?;
                        }
                        Err(ff::Error::Eof) => {
                            *eof = true;
                            break;
                        }
                        Err(_) => break,
                    }
                },
                Out::Copy { .. } => {}
            }
        }
        Ok(())
    };

    let mut reached_end = false;
    for (stream, mut packet) in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        if last_output.elapsed().as_secs_f64() > inactivity {
            return Err(Stop::TimedOut);
        }
        let index = stream.index();
        let (start, tb) = starts
            .get(&index)
            .copied()
            .unwrap_or((0, stream.time_base()));
        // Packet copy.
        if let Some(Out::Copy { ost, .. }) = outs
            .iter()
            .find(|o| matches!(o, Out::Copy { input, .. } if *input == index))
        {
            let ost = *ost;
            let pts = packet.pts().or(packet.dts()).unwrap_or(0);
            let t = secs(pts - start, tb) - seek;
            if limit.is_some_and(|d| t >= d) {
                if Some(index) == inp.video {
                    reached_end = true;
                }
                continue;
            }
            let shift = *copy_shift
                .entry(index)
                .or_insert(((seek) / secs(1, tb)).round() as i64 + start);
            let otb = octx.stream(ost).map(|s| s.time_base()).unwrap_or(tb);
            if let Some(p) = packet.pts() {
                packet.set_pts(Some(p - shift));
            }
            if let Some(d) = packet.dts() {
                packet.set_dts(Some(d - shift));
            }
            packet.set_position(-1);
            packet.set_stream(ost);
            packet.rescale_ts(tb, otb);
            packet.write_interleaved(&mut octx)?;
            last_output = Instant::now();
            if Some(index) == inp.video {
                on_progress((t / expected * 100.0).clamp(0.0, 99.0));
            }
            if reached_end && asm.copy_audio.is_empty() {
                break;
            }
            continue;
        }
        let Some(g) = graph.as_mut() else {
            continue;
        };
        if Some(index) == inp.video
            && let Some(dec) = decoders.video.as_mut()
        {
            if let Err(e) = dec.send_packet(&packet) {
                tracing::warn!(error = %e, "export: skipped an undecodable video packet");
            }
            while dec.receive_frame(&mut vframe).is_ok() {
                last_output = Instant::now();
                let pts = vframe.timestamp().or(vframe.pts()).unwrap_or(0);
                let t = secs(pts - start, tb);
                if t + 1e-6 < seek {
                    continue;
                }
                let shifted = pts - start - (seek / secs(1, tb)).round() as i64;
                vframe.set_pts(Some(shifted));
                feed(g.get("0:v").expect("source").source().add(&vframe))?;
            }
        } else if let Some((label, dec)) = decoders.audio.get_mut(&index) {
            if let Err(e) = dec.send_packet(&packet) {
                tracing::warn!(error = %e, "export: skipped an undecodable audio packet");
            }
            while dec.receive_frame(&mut aframe).is_ok() {
                last_output = Instant::now();
                let pts = aframe.timestamp().or(aframe.pts()).unwrap_or(0);
                let dur = aframe.samples() as f64 / f64::from(dec.rate().max(1));
                let t = secs(pts - start, tb);
                if t + dur < seek {
                    continue;
                }
                let shifted = pts - start - (seek / secs(1, tb)).round() as i64;
                aframe.set_pts(Some(shifted));
                feed(g.get(label).expect("source").source().add(&aframe))?;
            }
        } else {
            continue;
        }
        pump(
            &mut graph,
            &mut outs,
            &mut octx,
            &mut h264_packets,
            &mut video_eof,
            &mut last_output,
            on_progress,
        )?;
        let audio_done = outs
            .iter()
            .all(|o| !matches!(o, Out::Audio { eof: false, .. }));
        if video_eof && audio_done {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Stop::Cancelled);
    }
    // Flush decoders and sources.
    if let Some(g) = graph.as_mut() {
        if let Some(dec) = decoders.video.as_mut() {
            let _ = dec.send_eof();
            while dec.receive_frame(&mut vframe).is_ok() {
                let index = inp.video.unwrap_or(0);
                let (start, tb) = starts
                    .get(&index)
                    .copied()
                    .unwrap_or((0, ff::Rational::new(1, 1)));
                let pts = vframe.timestamp().or(vframe.pts()).unwrap_or(0);
                if secs(pts - start, tb) + 1e-6 < seek {
                    continue;
                }
                vframe.set_pts(Some(pts - start - (seek / secs(1, tb)).round() as i64));
                let _ = g.get("0:v").expect("source").source().add(&vframe);
            }
            let _ = g.get("0:v").expect("source").source().flush();
        }
        for (label, dec) in decoders.audio.values_mut() {
            let _ = dec.send_eof();
            while dec.receive_frame(&mut aframe).is_ok() {
                let _ = g.get(label).expect("source").source().add(&aframe);
            }
            let _ = g.get(label).expect("source").source().flush();
        }
    }
    pump(
        &mut graph,
        &mut outs,
        &mut octx,
        &mut h264_packets,
        &mut video_eof,
        &mut last_output,
        on_progress,
    )?;
    // Flush encoders.
    for out in outs.iter_mut() {
        match out {
            Out::Video { ost, enc, .. } => match enc {
                VideoEnc::H264 { enc, fps } => {
                    let hw = is_hardware(enc.active_codec());
                    enc.flush(&mut h264_packets).map_err(|e| Stop::Encoder {
                        hardware: hw,
                        message: e.to_string(),
                    })?;
                    write_h264(&mut octx, *ost, *fps, &mut h264_packets)?;
                }
                VideoEnc::Ffv1(e) => {
                    e.send_eof()?;
                    let etb = e.time_base();
                    drain_packets(|p| e.receive_packet(p).is_ok(), &mut octx, *ost, etb)?;
                }
            },
            Out::Audio { ost, enc, .. } => {
                enc.send_eof()?;
                let etb = enc.time_base();
                drain_packets(|p| enc.receive_packet(p).is_ok(), &mut octx, *ost, etb)?;
            }
            Out::Copy { .. } => {}
        }
    }
    octx.write_trailer()?;
    Ok(())
}

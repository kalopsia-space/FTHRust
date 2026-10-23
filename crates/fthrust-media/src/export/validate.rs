//! `_validate_export_output`: the staged file must have a video stream and
//! a decodable first frame, within 15 s.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ffmpeg_next as ff;

/// Validation bound.
const LIMIT: Duration = Duration::from_secs(15);

/// `Ok` when the file probes and its first video frame decodes.
pub fn check(path: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let started = Instant::now();
    let mut ictx = ff::format::input(&path).map_err(|e| e.to_string())?;
    let stream = ictx
        .streams()
        .best(ff::media::Type::Video)
        .ok_or("Export has no video stream")?;
    let index = stream.index();
    let mut decoder = ff::codec::context::Context::from_parameters(stream.parameters())
        .and_then(|c| c.decoder().video())
        .map_err(|e| e.to_string())?;
    let mut frame = ff::frame::Video::empty();
    for (s, packet) in ictx.packets() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Export validation cancelled".into());
        }
        if started.elapsed() > LIMIT {
            return Err("Export validation timed out".into());
        }
        if s.index() != index {
            continue;
        }
        decoder.send_packet(&packet).map_err(|e| e.to_string())?;
        if decoder.receive_frame(&mut frame).is_ok() {
            return Ok(());
        }
    }
    let _ = decoder.send_eof();
    if decoder.receive_frame(&mut frame).is_ok() {
        return Ok(());
    }
    Err("first-frame decode failed".into())
}

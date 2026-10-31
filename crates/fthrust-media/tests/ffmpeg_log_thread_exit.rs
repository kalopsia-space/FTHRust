//! FFmpeg's repeat counts survive the thread that logged them. Its own
//! binary: the test needs the process-wide subscriber and the queue of
//! counts to itself.

use std::io::Write;
use std::ptr;
use std::sync::{Arc, Mutex};

use ffmpeg_next::ffi;

/// A `tracing` writer that keeps everything it is given.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_thread_that_exits_still_reports_its_repeats() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    // The count outlives the thread, so a subscriber scoped to it would be
    // gone: a process-wide one catches what follows.
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    // An FFmpeg worker thread that repeats a line and ends; the process
    // must not abort while its thread locals go.
    std::thread::spawn(|| {
        for _ in 0..3 {
            // SAFETY: an av_log call with a constant message.
            unsafe {
                ffi::av_log(
                    ptr::null_mut(),
                    ffi::AV_LOG_ERROR,
                    c"an exiting thread repeats this\n".as_ptr(),
                );
            }
        }
    })
    .join()
    .unwrap();
    // The next message, from any thread, brings the count.
    // SAFETY: an av_log call with a constant message.
    unsafe {
        ffi::av_log(ptr::null_mut(), ffi::AV_LOG_ERROR, c"later\n".as_ptr());
    }
    let text = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert_eq!(
        text.matches("an exiting thread repeats this").count(),
        2,
        "{text}"
    );
    assert!(
        text.contains(
            "(repeated 2 more times before its thread ended: an exiting thread repeats this)"
        ),
        "{text}"
    );
    let note = text.find("repeated 2 more times").unwrap();
    assert!(note < text.find("later").unwrap(), "{text}");
}

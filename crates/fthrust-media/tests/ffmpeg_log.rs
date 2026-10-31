//! FFmpeg's log routed into `tracing`. Its own binary: the callback and the
//! level are process-wide.

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

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// Runs `f` with this thread's `tracing` events captured; their text.
fn capture(f: impl FnOnce()) -> String {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    captured.text()
}

#[test]
fn warnings_and_errors_reach_tracing_and_info_does_not() {
    fthrust_media::init().unwrap();
    // FTHRUST_FFMPEG_LOG unset: the default is warnings.
    assert!(std::env::var_os("FTHRUST_FFMPEG_LOG").is_none());
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        // SAFETY: plain av_log calls; every format argument matches its
        // conversion.
        unsafe {
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_WARNING,
                c"a warning with %d in it\n".as_ptr(),
                7 as std::ffi::c_int,
            );
            ffi::av_log(ptr::null_mut(), ffi::AV_LOG_INFO, c"chatty info\n".as_ptr());
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_DEBUG,
                c"debug noise\n".as_ptr(),
            );
            // FFmpeg writes some lines in pieces.
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"an error in %s".as_ptr(),
                c"two".as_ptr(),
            );
            ffi::av_log(ptr::null_mut(), ffi::AV_LOG_ERROR, c" parts\n".as_ptr());
        }
    });
    assert!(text.contains("a warning with 7 in it"), "{text}");
    assert!(text.contains("an error in two parts"), "{text}");
    assert!(text.contains("fthrust_media::ffmpeg"), "{text}");
    assert!(!text.contains("chatty info"), "{text}");
    assert!(!text.contains("debug noise"), "{text}");
}

#[test]
fn the_mp4_udta_retry_note_stays_out_of_the_warnings() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        // SAFETY: an av_log call with a constant message.
        unsafe {
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_WARNING,
                c"UDTA parsing failed retrying raw\n".as_ptr(),
            );
            ffi::av_log(ptr::null_mut(), ffi::AV_LOG_WARNING, c"after\n".as_ptr());
        }
    });
    assert!(!text.contains("UDTA"), "{text}");
    assert!(text.contains("after"), "{text}");
}

#[test]
fn messages_name_the_component_that_logged_them() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        // SAFETY: the codec context is allocated, logged against and freed
        // here; its first field is the AVClass av_log reads.
        unsafe {
            let codec = ffi::avcodec_find_encoder_by_name(c"mjpeg".as_ptr());
            let mut ctx = ffi::avcodec_alloc_context3(codec);
            assert!(!ctx.is_null());
            ffi::av_log(ctx.cast(), ffi::AV_LOG_ERROR, c"from a codec\n".as_ptr());
            ffi::avcodec_free_context(&mut ctx);
        }
    });
    assert!(text.contains("from a codec"), "{text}");
    assert!(text.contains("mjpeg"), "{text}");
}

#[test]
fn repeated_lines_are_counted_not_repeated() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        // SAFETY: av_log calls with constant messages.
        unsafe {
            for _ in 0..5 {
                ffi::av_log(ptr::null_mut(), ffi::AV_LOG_ERROR, c"same again\n".as_ptr());
            }
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"something else\n".as_ptr(),
            );
        }
    });
    assert_eq!(text.matches("same again").count(), 1, "{text}");
    assert!(text.contains("repeated 4 more times"), "{text}");
    assert!(text.contains("something else"), "{text}");
}

#[test]
fn a_line_repeated_after_a_pause_is_shown_again() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        // SAFETY: av_log calls with a constant message.
        unsafe {
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"now and then\n".as_ptr(),
            );
            std::thread::sleep(std::time::Duration::from_millis(1100));
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"now and then\n".as_ptr(),
            );
        }
    });
    assert_eq!(text.matches("now and then").count(), 2, "{text}");
}

#[test]
fn an_overlong_message_is_cut_but_stays_its_own_line() {
    fthrust_media::init().unwrap();
    fthrust_media::init_ffmpeg_logging();
    let long = std::ffi::CString::new(format!("{}\n", "x".repeat(2000))).unwrap();
    let text = capture(|| {
        // SAFETY: av_log calls whose format arguments match.
        unsafe {
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"%s".as_ptr(),
                long.as_ptr(),
            );
            ffi::av_log(
                ptr::null_mut(),
                ffi::AV_LOG_ERROR,
                c"the next one\n".as_ptr(),
            );
        }
    });
    assert!(text.lines().any(|l| l.contains("xxxx")), "{text}");
    let next = text
        .lines()
        .find(|l| l.contains("the next one"))
        .unwrap_or_else(|| panic!("the next message is missing: {text}"));
    assert!(!next.contains("xxxx"), "merged into the cut line: {next}");
}

#[path = "support/mod.rs"]
mod support;

#[test]
fn opening_a_clip_with_titled_tracks_warns_about_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("titled.mp4");
    if support::write_test_clip(&path, 1, true).is_none() {
        return;
    }
    fthrust_media::init_ffmpeg_logging();
    let text = capture(|| {
        let ictx = ffmpeg_next::format::input(&path).unwrap();
        assert!(ictx.streams().count() >= 2);
    });
    assert!(!text.contains("WARN"), "{text}");
}

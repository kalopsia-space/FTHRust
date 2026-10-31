//! FFmpeg's log, routed into `tracing` under the target
//! `fthrust_media::ffmpeg`.

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};

use ffmpeg_next::ffi;

/// The `va_list` FFmpeg hands its log callback, as the bindings spell it.
#[cfg(target_arch = "x86_64")]
type VaList = *mut ffi::__va_list_tag;
#[cfg(not(target_arch = "x86_64"))]
type VaList = ffi::va_list;

/// The level FFmpeg logs at unless told otherwise.
pub(crate) const DEFAULT_LEVEL: c_int = ffi::AV_LOG_WARNING;

/// Warnings that are harmless here, handled at verbose instead: the MP4
/// demuxer's note on every clip whose tracks carry titles, which ours do.
const DEMOTED: [&CStr; 1] = [c"UDTA parsing failed retrying raw\n"];

/// The FFmpeg level a level name (`warning`, `debug`…) or number names.
pub(crate) fn parse_level(value: &str) -> Option<c_int> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "quiet" => ffi::AV_LOG_QUIET,
        "panic" => ffi::AV_LOG_PANIC,
        "fatal" => ffi::AV_LOG_FATAL,
        "error" => ffi::AV_LOG_ERROR,
        "warning" | "warn" => ffi::AV_LOG_WARNING,
        "info" => ffi::AV_LOG_INFO,
        "verbose" => ffi::AV_LOG_VERBOSE,
        "debug" => ffi::AV_LOG_DEBUG,
        "trace" => ffi::AV_LOG_TRACE,
        number => number.parse().ok()?,
    })
}

/// Logs FFmpeg's messages up to `level` through [`callback`].
pub(crate) fn install(level: c_int) {
    // SAFETY: both calls only store their argument; the callback is a plain
    // function, valid for the life of the process and safe on any thread.
    unsafe {
        ffi::av_log_set_level(level);
        ffi::av_log_set_callback(Some(callback));
    }
}

/// The level a message is handled at: [`DEMOTED`] warnings drop to verbose.
fn effective_level(level: c_int, fmt: &CStr) -> c_int {
    if level > ffi::AV_LOG_ERROR && level <= ffi::AV_LOG_WARNING && DEMOTED.contains(&fmt) {
        ffi::AV_LOG_VERBOSE
    } else {
        level
    }
}

/// Per thread: FFmpeg writes some lines in pieces and repeats others.
#[derive(Default)]
struct Lines {
    partial: String,
    /// The last line sent on, and its level.
    last: Option<(c_int, String)>,
    repeats: usize,
}

thread_local! {
    static LINES: RefCell<Lines> = RefCell::new(Lines::default());
}

unsafe extern "C" fn callback(avcl: *mut c_void, level: c_int, fmt: *const c_char, vl: VaList) {
    // Never unwind into FFmpeg.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: FFmpeg passes the context, format and arguments of one
        // av_log call.
        unsafe { handle(avcl, level, fmt, vl) }
    }));
}

/// # Safety
/// The arguments of one av_log call, as the callback receives them.
unsafe fn handle(avcl: *mut c_void, level: c_int, fmt: *const c_char, vl: VaList) {
    if fmt.is_null() {
        return;
    }
    // SAFETY: FFmpeg's format strings are NUL-terminated.
    let level = effective_level(level, unsafe { CStr::from_ptr(fmt) });
    // SAFETY: no preconditions.
    if level > unsafe { ffi::av_log_get_level() } {
        return;
    }
    let mut buf = [0 as c_char; 1024];
    // No "[name @ 0x…]" prefix: the name is added below, without the address.
    let mut print_prefix: c_int = 0;
    // SAFETY: `buf` is writable for its length; `vl` holds the arguments
    // `fmt` asks for and is consumed once, here.
    let written = unsafe {
        ffi::av_log_format_line2(
            avcl,
            level,
            fmt,
            vl,
            buf.as_mut_ptr(),
            buf.len() as c_int,
            &mut print_prefix,
        )
    };
    if written < 0 {
        return;
    }
    // SAFETY: av_log_format_line2 NUL-terminates what it writes.
    let piece = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    // SAFETY: `avcl` is av_log's context argument.
    let name = unsafe { component(avcl) };
    LINES.with(|lines| {
        let Ok(mut lines) = lines.try_borrow_mut() else {
            return;
        };
        lines.partial.push_str(&piece);
        if !lines.partial.ends_with('\n') {
            return;
        }
        let text = std::mem::take(&mut lines.partial);
        let text = text.trim_end();
        if text.is_empty() {
            return;
        }
        let line = match &name {
            Some(name) => format!("[{name}] {text}"),
            None => text.to_owned(),
        };
        if lines.last.as_ref() == Some(&(level, line.clone())) {
            lines.repeats += 1;
            return;
        }
        if let Some((last_level, _)) = lines.last
            && lines.repeats > 0
        {
            let note = format!("(the last message repeated {} more times)", lines.repeats);
            emit(last_level, &note);
        }
        lines.repeats = 0;
        emit(level, &line);
        lines.last = Some((level, line));
    });
}

/// The name of the context a message came from (`mjpeg`, `SwrContext`,
/// `mov,mp4,m4a,3gp,3g2,mj2`…), when it has one.
///
/// # Safety
/// `avcl` is null or points to a struct whose first field is its AVClass,
/// as av_log requires.
unsafe fn component(avcl: *mut c_void) -> Option<String> {
    if avcl.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller contract above.
    let class = unsafe { *avcl.cast::<*const ffi::AVClass>() };
    if class.is_null() {
        return None;
    }
    // SAFETY: an AVClass is static; item_name takes the context it
    // describes and returns a NUL-terminated name.
    let name = unsafe {
        match (*class).item_name {
            Some(item_name) => item_name(avcl),
            None => (*class).class_name,
        }
    };
    // SAFETY: a non-null name is NUL-terminated.
    (!name.is_null()).then(|| {
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    })
}

fn emit(level: c_int, line: &str) {
    const TARGET: &str = "fthrust_media::ffmpeg";
    if level <= ffi::AV_LOG_ERROR {
        tracing::error!(target: TARGET, "{line}");
    } else if level <= ffi::AV_LOG_WARNING {
        tracing::warn!(target: TARGET, "{line}");
    } else if level <= ffi::AV_LOG_INFO {
        tracing::info!(target: TARGET, "{line}");
    } else if level <= ffi::AV_LOG_DEBUG {
        tracing::debug!(target: TARGET, "{line}");
    } else {
        tracing::trace!(target: TARGET, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_by_name_and_number() {
        assert_eq!(parse_level("warning"), Some(ffi::AV_LOG_WARNING));
        assert_eq!(parse_level(" Debug "), Some(ffi::AV_LOG_DEBUG));
        assert_eq!(parse_level("quiet"), Some(ffi::AV_LOG_QUIET));
        assert_eq!(parse_level("trace"), Some(ffi::AV_LOG_TRACE));
        assert_eq!(parse_level("32"), Some(ffi::AV_LOG_INFO));
        assert_eq!(parse_level("loud"), None);
        assert_eq!(parse_level(""), None);
    }

    #[test]
    fn only_the_known_harmless_warning_is_demoted() {
        let udta = c"UDTA parsing failed retrying raw\n";
        assert_eq!(
            effective_level(ffi::AV_LOG_WARNING, udta),
            ffi::AV_LOG_VERBOSE
        );
        assert_eq!(effective_level(ffi::AV_LOG_ERROR, udta), ffi::AV_LOG_ERROR);
        let other = c"Invalid timestamps\n";
        assert_eq!(
            effective_level(ffi::AV_LOG_WARNING, other),
            ffi::AV_LOG_WARNING
        );
    }
}

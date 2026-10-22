//! The redacting, rotating application log and the crash log.
//!
//! `fthr.log` rotates at 2 MiB keeping two backups, every line passes through
//! [`super::redact::redact`], and the format matches the reference's
//! `%(asctime)s %(levelname)-8s [%(threadName)s] %(name)s: %(message)s`.
//! `fthr-crash.log` receives a start banner and every panic with its
//! backtrace.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

use time::OffsetDateTime;
use time::macros::format_description;
use tracing::Subscriber;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

use super::redact::redact;
use crate::error::{CoreError, Result};
use crate::paths::FthrPaths;

const DATE_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

fn local_now_text() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.format(DATE_FORMAT)
        .unwrap_or_else(|_| "0000-00-00 00:00:00".into())
}

/// Size-rotating log file that redacts each line and mirrors it to stderr.
#[derive(Debug)]
pub struct RotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    backups: u32,
    file: File,
    len: u64,
    pending: String,
    mirror_stderr: bool,
}

impl RotatingWriter {
    /// Opens `path` for appending, creating parent directories.
    pub fn open(
        path: &Path,
        max_bytes: u64,
        backups: u32,
        mirror_stderr: bool,
    ) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            backups,
            file,
            len,
            pending: String::new(),
            mirror_stderr,
        })
    }

    /// Writes one redacted line, rotating first when it would cross the
    /// size limit (the reference's `RotatingFileHandler` rule).
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let line = redact(line.trim_end_matches('\n'));
        let bytes = line.len() as u64 + 1;
        if self.max_bytes > 0 && self.len + bytes >= self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.len += bytes;
        if self.mirror_stderr {
            let _ = writeln!(io::stderr(), "{line}");
        }
        Ok(())
    }

    fn backup_path(&self, index: u32) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        if self.backups > 0 {
            for index in (1..self.backups).rev() {
                let from = self.backup_path(index);
                if from.exists() {
                    fs::rename(&from, self.backup_path(index + 1))?;
                }
            }
            fs::rename(&self.path, self.backup_path(1))?;
        }
        self.file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.push_str(&String::from_utf8_lossy(buf));
        while let Some(pos) = self.pending.find('\n') {
            let line = self.pending[..pos].to_string();
            self.write_line(&line)?;
            self.pending.drain(..=pos);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.write_line(&line)?;
        }
        self.file.flush()
    }
}

/// Cloneable handle to a shared [`RotatingWriter`]; usable as a
/// `tracing_subscriber` writer.
#[derive(Clone, Debug)]
pub struct SharedWriter(Arc<Mutex<RotatingWriter>>);

impl SharedWriter {
    fn lock(&self) -> std::sync::MutexGuard<'_, RotatingWriter> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
}

impl<'a> MakeWriter<'a> for SharedWriter {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Event formatter producing the reference's line layout.
struct FthrFormat;

impl<S, N> FormatEvent<S, N> for FthrFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let thread = std::thread::current();
        let thread_name = thread
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{:?}", thread.id()));
        write!(
            writer,
            "{} {:<8} [{}] {}: ",
            local_now_text(),
            event.metadata().level().as_str(),
            thread_name,
            event.metadata().target()
        )?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// The application log: opens `logs/fthr.log`, writes the session banner and
/// builds `tracing` subscribers that write through it.
#[derive(Debug)]
pub struct Logger {
    writer: SharedWriter,
}

impl Logger {
    /// Rotation threshold of the reference's handler.
    pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;
    /// Backup files kept by the reference's handler.
    pub const DEFAULT_BACKUPS: u32 = 2;

    /// Opens the log with the reference's limits and writes the banner.
    pub fn open(paths: &FthrPaths, app_version: &str) -> Result<Self> {
        Self::open_with_limits(
            paths,
            app_version,
            Self::DEFAULT_MAX_BYTES,
            Self::DEFAULT_BACKUPS,
        )
    }

    /// Opens the log with explicit rotation limits and writes the banner
    /// `===== FTHR Clips <version> started <timestamp> (linux) =====`.
    pub fn open_with_limits(
        paths: &FthrPaths,
        app_version: &str,
        max_bytes: u64,
        backups: u32,
    ) -> Result<Self> {
        let mut writer = RotatingWriter::open(&paths.app_log(), max_bytes, backups, true)?;
        writer.write_line(&format!(
            "===== FTHR Clips {app_version} started {} (linux) =====",
            local_now_text()
        ))?;
        Ok(Self {
            writer: SharedWriter(Arc::new(Mutex::new(writer))),
        })
    }

    /// A `tracing` subscriber that writes every event through this log.
    pub fn subscriber(&self) -> impl Subscriber + Send + Sync + 'static {
        tracing_subscriber::fmt()
            .event_format(FthrFormat)
            .with_writer(self.writer.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish()
    }

    /// Flushes buffered output.
    pub fn flush(&self) {
        let _ = self.writer.clone().flush();
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Keeps the global log alive and flushes it on drop.
#[derive(Debug)]
pub struct LogGuard {
    logger: Logger,
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        self.logger.flush();
    }
}

/// Opens the application log and installs it as the global `tracing`
/// subscriber. Fails if a global subscriber is already installed.
pub fn init_logging(paths: &FthrPaths, app_version: &str) -> Result<LogGuard> {
    let logger = Logger::open(paths, app_version)?;
    tracing::subscriber::set_global_default(logger.subscriber())
        .map_err(|e| CoreError::Invalid(format!("global logger already installed: {e}")))?;
    Ok(LogGuard { logger })
}

/// Largest crash log kept before it is rotated to `.old` at startup.
pub const CRASH_LOG_MAX_BYTES: u64 = 1024 * 1024;

static CRASH_LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static PANIC_HOOK: Once = Once::new();

fn crash_log_path() -> Option<PathBuf> {
    CRASH_LOG_PATH.lock().ok().and_then(|p| p.clone())
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = crash_log_path() {
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".into());
            let thread = std::thread::current();
            let thread_name = thread.name().unwrap_or("<unnamed>").to_string();
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown>".into());
            let backtrace = std::backtrace::Backtrace::force_capture();
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(
                    file,
                    "panic: {} (thread {}) at {}\n{}",
                    redact(&message),
                    thread_name,
                    location,
                    backtrace
                );
            }
        }
        previous(info);
    }));
}

/// Rotates an oversized `fthr-crash.log` to `.old`, writes the start banner
/// `===== started <timestamp> pid <N> =====` and installs a panic hook that
/// appends every panic with its backtrace.
///
/// Native signal crashes (SIGSEGV and friends) are left to the runtime's own
/// handler; the launcher captures stderr into the crash log for those.
pub fn init_crash_log(paths: &FthrPaths) -> Result<()> {
    fs::create_dir_all(paths.logs_dir())?;
    let path = paths.crash_log();
    if let Ok(meta) = fs::metadata(&path)
        && meta.len() > CRASH_LOG_MAX_BYTES
    {
        let mut old = path.as_os_str().to_owned();
        old.push(".old");
        fs::rename(&path, PathBuf::from(old))?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(
        file,
        "===== started {} pid {} =====",
        local_now_text(),
        std::process::id()
    )?;
    file.flush()?;
    if let Ok(mut slot) = CRASH_LOG_PATH.lock() {
        *slot = Some(path);
    }
    PANIC_HOOK.call_once(install_panic_hook);
    Ok(())
}

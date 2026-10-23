//! The `~/.fthr` path map and the private runtime directory.

use crate::error::{CoreError, Result};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Locations of every file and directory FTHR keeps under the user's home.
#[derive(Debug, Clone)]
pub struct FthrPaths {
    home: PathBuf,
}

impl FthrPaths {
    /// Builds a path map rooted at `home`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// Builds a path map from `$HOME`; errors if it is unset.
    pub fn from_env() -> Result<Self> {
        match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => Ok(Self::new(h)),
            _ => Err(CoreError::Invalid("HOME is not set".into())),
        }
    }

    /// The home directory itself.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `uploader/known_hosts` (host keys the uploader verified).
    pub fn uploader_known_hosts_file(&self) -> PathBuf {
        self.uploader_dir().join("known_hosts")
    }

    /// `~/.fthr`.
    pub fn root(&self) -> PathBuf {
        self.home.join(".fthr")
    }
    /// `settings.json`.
    pub fn settings_file(&self) -> PathBuf {
        self.root().join("settings.json")
    }
    /// `presets.json`.
    pub fn presets_file(&self) -> PathBuf {
        self.root().join("presets.json")
    }
    /// `export_presets.json`.
    pub fn export_presets_file(&self) -> PathBuf {
        self.root().join("export_presets.json")
    }
    /// `encoder_capabilities.json`.
    pub fn encoder_capabilities_file(&self) -> PathBuf {
        self.root().join("encoder_capabilities.json")
    }
    /// `clip_metadata.sqlite3`.
    pub fn clip_metadata_db(&self) -> PathBuf {
        self.root().join("clip_metadata.sqlite3")
    }
    /// Legacy `clip_metadata.json`.
    pub fn legacy_clip_metadata_json(&self) -> PathBuf {
        self.root().join("clip_metadata.json")
    }
    /// `thumbnails/`.
    pub fn thumbnails_dir(&self) -> PathBuf {
        self.root().join("thumbnails")
    }
    /// `theme/`.
    pub fn theme_dir(&self) -> PathBuf {
        self.root().join("theme")
    }
    /// `theme/theme.json`.
    pub fn theme_file(&self) -> PathBuf {
        self.theme_dir().join("theme.json")
    }
    /// `theme/icons/`.
    pub fn theme_icons_dir(&self) -> PathBuf {
        self.theme_dir().join("icons")
    }
    /// `theme/sounds/`.
    pub fn theme_sounds_dir(&self) -> PathBuf {
        self.theme_dir().join("sounds")
    }
    /// `theme/fonts/`.
    pub fn theme_fonts_dir(&self) -> PathBuf {
        self.theme_dir().join("fonts")
    }
    /// `logs/`.
    pub fn logs_dir(&self) -> PathBuf {
        self.root().join("logs")
    }
    /// `logs/fthr.log`.
    pub fn app_log(&self) -> PathBuf {
        self.logs_dir().join("fthr.log")
    }
    /// `logs/fthr-crash.log`.
    pub fn crash_log(&self) -> PathBuf {
        self.logs_dir().join("fthr-crash.log")
    }
    /// `logs/engine.log`.
    pub fn engine_log(&self) -> PathBuf {
        self.logs_dir().join("engine.log")
    }
    /// `diagnostics/`.
    pub fn diagnostics_dir(&self) -> PathBuf {
        self.root().join("diagnostics")
    }
    /// `uploader/`.
    pub fn uploader_dir(&self) -> PathBuf {
        self.root().join("uploader")
    }
    /// `uploader/settings.json`.
    pub fn uploader_settings_file(&self) -> PathBuf {
        self.uploader_dir().join("settings.json")
    }
    /// `uploader/upload_history.json`.
    pub fn upload_history_file(&self) -> PathBuf {
        self.uploader_dir().join("upload_history.json")
    }
    /// Legacy `upload_history.json` directly under the root.
    pub fn legacy_upload_history_file(&self) -> PathBuf {
        self.root().join("upload_history.json")
    }
    /// `playback/` cache directory.
    pub fn playback_cache_dir(&self) -> PathBuf {
        self.root().join("playback")
    }
    /// `hotkeys.json`.
    pub fn hotkeys_file(&self) -> PathBuf {
        self.root().join("hotkeys.json")
    }
    /// `hotkeys-portal-cleared.json`.
    pub fn hotkeys_portal_cleared_file(&self) -> PathBuf {
        self.root().join("hotkeys-portal-cleared.json")
    }
    /// Default clips directory, `~/Videos/FTHR Clips`.
    pub fn default_clips_directory(&self) -> PathBuf {
        self.home.join("Videos").join("FTHR Clips")
    }
    /// Legacy clips directory, `~/FTHR_Clips`.
    pub fn legacy_clips_directory(&self) -> PathBuf {
        self.home.join("FTHR_Clips")
    }
}

/// Returns the private runtime directory, creating it (mode 0700) if absent.
///
/// This is `$XDG_RUNTIME_DIR/fthrust` when that variable is set, else
/// `~/.fthr/run`. An existing path is refused with [`CoreError::RuntimeDir`]
/// if it is a symlink, not a directory, owned by another user, or accessible
/// by group or others.
pub fn runtime_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(x) if !x.is_empty() => PathBuf::from(x).join("fthrust"),
        _ => FthrPaths::from_env()?.root().join("run"),
    };
    match std::fs::symlink_metadata(&dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(e.into()),
    }
    verify_private_dir(&dir)?;
    Ok(dir)
}

fn verify_private_dir(dir: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    let bad = |why: &str| Err(CoreError::RuntimeDir(format!("{}: {why}", dir.display())));
    if meta.file_type().is_symlink() {
        return bad("is a symlink");
    }
    if !meta.is_dir() {
        return bad("is not a directory");
    }
    if meta.uid() != nix::unistd::getuid().as_raw() {
        return bad("is owned by another user");
    }
    if meta.mode() & 0o077 != 0 {
        return bad("has group/other permission bits");
    }
    Ok(())
}

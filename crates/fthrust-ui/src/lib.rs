//! The FTHRust application shell: tokens, theme, fonts, icons, widgets,
//! the frameless window, settings pages and dialogs, verified against the
//! Qt reference screenshots.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod fidelity;
pub mod fonts;
pub mod icons;
pub mod theme;
pub mod tokens;

/// Crate version, printed by `fthrust --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the bundled assets live, relative to the repository (development)
/// or next to the executable (packaged).
pub fn assets_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FTHRUST_ASSETS") {
        return std::path::PathBuf::from(dir);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if dev.is_dir() {
        return dev.canonicalize().unwrap_or(dev);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets")))
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}

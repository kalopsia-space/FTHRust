//! The library model (`ClipGrid` state): records from the scan, the
//! filter and sort, day sections, the heading and the empty state.

use std::path::{Path, PathBuf};

use fthrust_core::clip_readiness::ClipReadinessRegistry;
use fthrust_core::library_index::{Filter, LibraryRecord, MediaKind, Sort, apply};
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

/// `_filter_heading`.
pub fn heading(filter: Filter) -> &'static str {
    match filter {
        Filter::All => "ALL CLIPS",
        Filter::Clips => "CLIPS",
        Filter::Screenshots => "SCREENSHOTS",
        Filter::Imported => "IMPORTED CLIPS",
    }
}

fn local_offset() -> UtcOffset {
    static OFFSET: std::sync::OnceLock<UtcOffset> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
}

/// `_section_label_for`: `%a, %b %d` in local time.
pub fn section_label(mtime_ns: i128) -> String {
    let when = OffsetDateTime::from_unix_timestamp_nanos(mtime_ns)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .to_offset(local_offset());
    when.format(format_description!(
        "[weekday repr:short], [month repr:short] [day]"
    ))
    .unwrap_or_default()
}

/// `_game_name_from_path`: the parent folder, `Desktop` at the root.
pub fn game_name(path: &Path, clips_root: &Path) -> String {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root_name = clips_root
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if parent.is_empty() || parent == root_name {
        "Desktop".into()
    } else {
        parent
    }
}

/// `_clip_title_from_filename`: the stem.
pub fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One day section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// `Fri, Jan 15`.
    pub label: String,
    /// The newest file's folder name.
    pub game: Option<String>,
    /// Paths in display order.
    pub paths: Vec<PathBuf>,
}

/// The empty-state texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyState {
    /// `No clips yet` / `No screenshots yet` / `No imported clips`.
    pub title: String,
    /// `PRESS <key> TO TAKE A SCREENSHOT` for the screenshots filter.
    pub detail: Option<String>,
}

/// The library.
#[derive(Clone, Debug)]
pub struct Library {
    /// The clips root.
    pub clips_root: PathBuf,
    /// Every known record (unfiltered).
    pub records: Vec<LibraryRecord>,
    /// Current filter.
    pub filter: Filter,
    /// Current sort.
    pub sort: Sort,
    /// Sections after `rebuild`.
    pub sections: Vec<Section>,
    /// Visible record count.
    pub count: usize,
    /// Set when nothing is visible.
    pub empty: Option<EmptyState>,
    /// A scan has completed (before that the heading is the reference's
    /// initial `All clips`).
    pub scanned: bool,
}

fn kind_of(path: &Path) -> MediaKind {
    if fthrust_core::clip_files::is_completed_video_path(path) {
        MediaKind::Video
    } else {
        MediaKind::Image
    }
}

impl Library {
    /// An empty library for `clips_root`.
    pub fn new(clips_root: PathBuf) -> Library {
        Library {
            clips_root,
            records: Vec::new(),
            filter: Filter::All,
            sort: Sort::Newest,
            sections: Vec::new(),
            count: 0,
            empty: None,
            scanned: false,
        }
    }

    /// Rebuilds the sections for the current filter and sort.
    pub fn rebuild(&mut self, screenshot_key: &str) {
        let visible = apply(&self.records, self.filter, self.sort);
        self.count = visible.len();
        self.sections.clear();
        if visible.is_empty() {
            let (title, detail) = match self.filter {
                Filter::Screenshots => (
                    "No screenshots yet",
                    Some(format!("PRESS {screenshot_key} TO TAKE A SCREENSHOT")),
                ),
                Filter::Imported => ("No imported clips", None),
                Filter::All | Filter::Clips => ("No clips yet", None),
            };
            self.empty = Some(EmptyState {
                title: title.into(),
                detail,
            });
            return;
        }
        self.empty = None;
        for r in visible {
            let label = section_label(r.mtime_ns);
            match self.sections.iter_mut().find(|s| s.label == label) {
                Some(s) => s.paths.push(r.path.clone()),
                None => self.sections.push(Section {
                    label,
                    game: Some(game_name(&r.path, &self.clips_root)),
                    paths: vec![r.path.clone()],
                }),
            }
        }
    }

    /// `ALL CLIPS  (12)`; the bare heading when empty.
    pub fn heading_text(&self) -> String {
        if self.count == 0 && !self.scanned {
            return "All clips".into();
        }
        if self.count == 0 {
            heading(self.filter).to_owned()
        } else {
            format!("{}  ({})", heading(self.filter), self.count)
        }
    }

    /// Adds or refreshes one file from disk; `false` when the file is
    /// missing or unchanged.
    pub fn upsert(&mut self, path: &Path, imported: bool) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let record = LibraryRecord {
            path: path.to_path_buf(),
            identity: identity.clone(),
            root_ids: vec![usize::from(imported)],
            imported,
            kind: kind_of(path),
            size: meta.len(),
            mtime_ns,
        };
        self.scanned = true;
        match self.records.iter_mut().find(|r| r.identity == identity) {
            Some(existing) if *existing == record => false,
            Some(existing) => {
                *existing = record;
                true
            }
            None => {
                self.records.push(record);
                true
            }
        }
    }

    /// Drops a file.
    pub fn remove(&mut self, path: &Path) {
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.records
            .retain(|r| r.identity != identity && r.path != path);
    }

    /// Renames a file in place (keeps its position and metadata).
    pub fn rename(&mut self, old: &Path, new: &Path) {
        let new_identity = std::fs::canonicalize(new).unwrap_or_else(|_| new.to_path_buf());
        for r in &mut self.records {
            if r.path == old {
                r.path = new.to_path_buf();
                r.identity = new_identity.clone();
            }
        }
    }

    /// Every displayed path in order.
    pub fn navigation_paths(&self, videos_only: bool) -> Vec<PathBuf> {
        self.sections
            .iter()
            .flat_map(|s| s.paths.iter().cloned())
            .filter(|p| !videos_only || kind_of(p) == MediaKind::Video)
            .collect()
    }

    /// Displayed, ready videos in order.
    pub fn navigation_paths_videos(&self, ready: &ClipReadinessRegistry) -> Vec<PathBuf> {
        self.navigation_paths(true)
            .into_iter()
            .filter(|p| ready.can_access(p))
            .collect()
    }

    /// The next (`step` 1) or previous (`-1`) ready video after `path`.
    pub fn adjacent(
        &self,
        path: &Path,
        step: i32,
        ready: &ClipReadinessRegistry,
    ) -> Option<PathBuf> {
        let all = self.navigation_paths(false);
        let index = all.iter().position(|p| p == path)? as i64;
        let mut i = index + i64::from(step);
        while i >= 0 && (i as usize) < all.len() {
            let candidate = &all[i as usize];
            if kind_of(candidate) == MediaKind::Video
                && ready.can_access(candidate)
                && candidate.is_file()
            {
                return Some(candidate.clone());
            }
            i += i64::from(step);
        }
        None
    }
}

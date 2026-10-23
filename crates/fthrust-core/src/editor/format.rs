//! Editor text formatting (`_fmt`, the timeline labels, file details).

use time::OffsetDateTime;
use time::macros::format_description;

/// `m:ss` with unbounded minutes and no hours.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The range label while trimming: `m:ss → m:ss`.
pub fn range_label(start_ms: u64, end_ms: u64) -> String {
    format!("{} → {}", clock(start_ms), clock(end_ms))
}

/// `"{kept} kept  ·  {n} cut(s)"`.
pub fn kept_label(kept_ms: u64, cuts: usize) -> String {
    let unit = if cuts == 1 { "cut" } else { "cuts" };
    format!("{} kept  ·  {cuts} {unit}", clock(kept_ms))
}

/// The "Created" value: `%b %d, %Y  %H:%M`.
pub fn created(when: OffsetDateTime) -> String {
    when.format(format_description!(
        "[month repr:short] [day], [year]  [hour]:[minute]"
    ))
    .unwrap_or_default()
}

/// The "Location" value: `…` plus the last 34 characters when over 36.
pub fn location(dir: &str) -> String {
    let n = dir.chars().count();
    if n > 36 {
        format!("…{}", dir.chars().skip(n - 34).collect::<String>())
    } else {
        dir.to_owned()
    }
}

/// The "Title" value: 36 characters plus `…` when over 38.
pub fn title(stem: &str) -> String {
    if stem.chars().count() > 38 {
        format!("{}…", stem.chars().take(36).collect::<String>())
    } else {
        stem.to_owned()
    }
}

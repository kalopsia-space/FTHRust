//! Export model: the reference's filter chains and graphs (verbatim
//! strings), output names and the size-preset plan. The media crate runs
//! the graphs in-process; nothing here touches FFmpeg.

pub mod filters;
pub mod graph;
pub mod naming;
pub mod plan;
pub mod watermark;

/// `QUALITY_OPTIONS`: label and `export_quality` value.
pub const QUALITY_OPTIONS: [(&str, &str); 3] = [
    ("Very fast", "fast"),
    ("High quality", "quality"),
    ("Lossless · MKV", "lossless"),
];

/// `normalize_quality`: anything unknown is `quality`.
pub fn normalize_quality(value: &str) -> &'static str {
    match value {
        "fast" => "fast",
        "lossless" => "lossless",
        _ => "quality",
    }
}

/// The display label of a quality value.
pub fn quality_label(value: &str) -> &'static str {
    QUALITY_OPTIONS
        .iter()
        .find(|(_, v)| *v == normalize_quality(value))
        .map_or("High quality", |(l, _)| l)
}

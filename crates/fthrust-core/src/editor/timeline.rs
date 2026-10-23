//! The trim timeline model (`TrimSlider`, `ui/clip_viewer.py:1045`).
//!
//! Positions are fractions 0..1 of the source duration. Segments are
//! contiguous and cover the whole clip; deleting only flags a segment. The
//! trim handles are independent of the segments: kept ranges are the
//! non-deleted segments clipped to the trim. With `hide_deleted` the
//! display axis compresses deleted spans out; the model, playback and
//! seeking always use source positions.

/// Kept pieces this short or shorter are dropped.
const MIN_KEPT_PIECE: f64 = 0.0001;

/// One timeline segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// Start fraction.
    pub start: f64,
    /// End fraction.
    pub end: f64,
    /// Removed from the edit.
    pub deleted: bool,
}

/// The editable timeline of one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    /// Source duration in milliseconds (at least 1).
    pub duration_ms: u64,
    /// Contiguous segments covering 0..1.
    pub segments: Vec<Segment>,
    /// Trim start handle.
    pub trim_start: f64,
    /// Trim end handle.
    pub trim_end: f64,
    /// Playhead (source fraction).
    pub playhead: f64,
    /// Selected segment index.
    pub selected: usize,
    /// Compress deleted spans out of the display axis.
    pub hide_deleted: bool,
    /// Zoom factor (1 = whole clip).
    pub zoom: f64,
    /// Display fraction at the left edge of the view.
    pub view_start: f64,
}

impl Timeline {
    /// A fresh timeline: one segment, full trim, 100 % zoom.
    pub fn new(duration_ms: u64) -> Timeline {
        Timeline {
            duration_ms: duration_ms.max(1),
            segments: vec![Segment {
                start: 0.0,
                end: 1.0,
                deleted: false,
            }],
            trim_start: 0.0,
            trim_end: 1.0,
            playhead: 0.0,
            selected: 0,
            hide_deleted: false,
            zoom: 1.0,
            view_start: 0.0,
        }
    }

    /// The minimum gap for splits and trim handles: 250 ms, at least 0.001.
    pub fn minimum(&self) -> f64 {
        (250.0 / self.duration_ms as f64).max(0.001)
    }

    /// About a five-second window for long sources, 8× for short ones.
    pub fn max_zoom(&self) -> f64 {
        (self.duration_ms as f64 / 5000.0).clamp(8.0, 2048.0)
    }

    /// Visible display span.
    pub fn span(&self) -> f64 {
        1.0 / self.zoom.max(1.0)
    }

    /// `_segment_index_at`.
    pub fn segment_index_at(&self, pct: f64) -> Option<usize> {
        for (i, s) in self.segments.iter().enumerate() {
            if self.hide_deleted && (s.deleted || pct == s.end) {
                continue;
            }
            if s.start <= pct && pct <= s.end {
                return Some(i);
            }
        }
        if self.hide_deleted {
            return self
                .segments
                .iter()
                .rposition(|s| !s.deleted && (pct - s.end).abs() < 1e-9);
        }
        None
    }

    /// Whether `split_at(pct)` would succeed.
    pub fn can_split(&self, pct: f64) -> bool {
        let Some(i) = self.segment_index_at(pct) else {
            return false;
        };
        let s = self.segments[i];
        let minimum = self.minimum();
        !s.deleted && pct - s.start >= minimum && s.end - pct >= minimum
    }

    /// Splits the segment under `pct`; the right half becomes selected.
    pub fn split_at(&mut self, pct: f64) -> bool {
        if !self.can_split(pct) {
            return false;
        }
        let i = self.segment_index_at(pct).unwrap_or(0);
        let s = self.segments[i];
        self.segments.splice(
            i..=i,
            [Segment { end: pct, ..s }, Segment { start: pct, ..s }],
        );
        self.selected = i + 1;
        true
    }

    /// Restoring is always allowed; deleting needs another active segment.
    pub fn can_toggle_selected(&self) -> bool {
        let Some(s) = self.segments.get(self.selected) else {
            return false;
        };
        if s.deleted {
            return true;
        }
        let active = self.segments.iter().filter(|s| !s.deleted).count();
        self.segments.len() >= 2 && active > 1
    }

    /// `toggle_selected_deleted`.
    pub fn toggle_selected(&mut self) -> bool {
        if !self.can_toggle_selected() {
            return false;
        }
        let s = &mut self.segments[self.selected];
        s.deleted = !s.deleted;
        true
    }

    /// Moves the start handle, kept at least `minimum` before the end.
    pub fn set_trim_start(&mut self, pct: f64) {
        self.trim_start = pct.min(self.trim_end - self.minimum()).max(0.0);
    }

    /// Moves the end handle, kept at least `minimum` after the start.
    pub fn set_trim_end(&mut self, pct: f64) {
        self.trim_end = pct.max(self.trim_start + self.minimum()).min(1.0);
    }

    /// The ranges that play and export.
    pub fn kept_ranges(&self) -> Vec<(f64, f64)> {
        self.segments
            .iter()
            .filter(|s| !s.deleted)
            .filter_map(|s| {
                let start = s.start.max(self.trim_start);
                let end = s.end.min(self.trim_end);
                (end - start > MIN_KEPT_PIECE).then_some((start, end))
            })
            .collect()
    }

    /// Kept ranges in milliseconds (`int(pct * duration)`).
    pub fn kept_ranges_ms(&self) -> Vec<(u64, u64)> {
        let d = self.duration_ms as f64;
        self.kept_ranges()
            .into_iter()
            .map(|(s, e)| ((s * d) as u64, (e * d) as u64))
            .collect()
    }

    /// Total kept fraction of the source.
    pub fn kept_fraction(&self) -> f64 {
        self.kept_ranges().iter().map(|(s, e)| e - s).sum()
    }

    /// Deleted segments.
    pub fn cut_count(&self) -> usize {
        self.segments.iter().filter(|s| s.deleted).count()
    }

    /// Non-deleted segment ranges, never empty.
    fn mapped(&self) -> Vec<(f64, f64)> {
        let r: Vec<(f64, f64)> = self
            .segments
            .iter()
            .filter(|s| !s.deleted)
            .map(|s| (s.start, s.end))
            .collect();
        if r.is_empty() { vec![(0.0, 1.0)] } else { r }
    }

    /// `_source_to_display`.
    pub fn source_to_display(&self, pct: f64) -> f64 {
        if !self.hide_deleted {
            return pct;
        }
        let ranges = self.mapped();
        let mut offsets = vec![0.0];
        for (s, e) in &ranges {
            offsets.push(offsets[offsets.len() - 1] + (e - s));
        }
        let index = ranges.partition_point(|&(_, e)| e <= pct);
        let mut offset = offsets[index];
        if let Some(&(start, end)) = ranges.get(index) {
            offset += (pct.min(end) - start).max(0.0);
        }
        offset / offsets[offsets.len() - 1]
    }

    /// `_display_to_source`.
    pub fn display_to_source(&self, d: f64) -> f64 {
        if !self.hide_deleted {
            return d;
        }
        let ranges = self.mapped();
        let mut offsets = vec![0.0];
        for (s, e) in &ranges {
            offsets.push(offsets[offsets.len() - 1] + (e - s));
        }
        let offset = d.clamp(0.0, 1.0) * offsets[offsets.len() - 1];
        let index = offsets[1..].partition_point(|&c| c <= offset + 1e-9);
        match ranges.get(index) {
            Some(&(start, _)) => start + offset - offsets[index],
            None => ranges[ranges.len() - 1].1,
        }
    }

    /// `set_zoom`: the anchor (source fraction, the playhead when `None`)
    /// keeps its place in the view.
    pub fn set_zoom(&mut self, factor: f64, anchor: Option<f64>) {
        let old_span = self.span();
        let factor = factor.clamp(1.0, self.max_zoom());
        let anchor = self.source_to_display(anchor.unwrap_or(self.playhead));
        let local = (anchor - self.view_start) / old_span.max(1e-6);
        self.zoom = factor;
        let span = self.span();
        self.view_start = (anchor - local * span).min(1.0 - span).max(0.0);
    }

    /// Pans by 0.7 of the view span in `direction` (±1).
    pub fn pan(&mut self, direction: f64) {
        let span = self.span();
        self.view_start = (self.view_start + direction * span * 0.7)
            .min(1.0 - span)
            .max(0.0);
    }

    /// Scrolls `pct` into view when it is outside.
    pub fn ensure_visible(&mut self, pct: f64, align_start: bool) {
        let pct = self.source_to_display(pct);
        let span = self.span();
        if self.view_start <= pct && pct <= self.view_start + span {
            return;
        }
        let offset = span * if align_start { 0.08 } else { 0.5 };
        self.view_start = (pct - offset).min(1.0 - span).max(0.0);
    }

    /// Index of the kept range playing at `pos_ms`: `start - slack <= pos <
    /// end - slack` (`_update_playhead`).
    pub fn kept_range_at_ms(&self, pos_ms: u64, slack_ms: u64) -> Option<usize> {
        let pos = pos_ms as i64;
        let slack = slack_ms as i64;
        self.kept_ranges_ms()
            .iter()
            .position(|&(s, e)| s as i64 - slack <= pos && pos < e as i64 - slack)
    }

    /// Start of the first kept range starting after `pos_ms`.
    pub fn next_kept_start_ms(&self, pos_ms: u64) -> Option<u64> {
        self.kept_ranges_ms()
            .iter()
            .map(|&(s, _)| s)
            .find(|&s| s > pos_ms)
    }
}

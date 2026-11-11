//! The bench's view of the loop, in the source's own frames (ADR 0041: 1×, 2×, 4× and 8× with
//! horizontal scrolling, preserving the center source position when the zoom changes where the
//! scroll bounds allow; zoom and scrolling never change stored positions). Independent of the
//! session rate and the display scale.

use super::state::Loop;
use crate::tokens::sampler as tk;

/// The zoom (an index into `sampler.bench.zooms`) and the source frame the view centers on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Viewport {
    pub zoom: usize,
    /// The center the view keeps (`None`: the loop's middle). Selecting a slice puts it on the
    /// slice's middle, so the next zoom keeps the selected slice centered (README, Slices view);
    /// a scroll puts it on the scrolled view's middle.
    pub center: Option<f64>,
}

impl Viewport {
    pub fn factor(&self) -> f32 {
        tk::BENCH_ZOOMS[self.zoom.min(tk::BENCH_ZOOMS.len() - 1)]
    }

    /// Frames the view shows.
    pub fn span(&self, l: &Loop) -> f64 {
        l.frames() / f64::from(self.factor())
    }

    /// The first and last frame the view shows: around the center, inside the loop.
    pub fn range(&self, l: &Loop) -> (f64, f64) {
        let span = self.span(l);
        let (lo, hi) = (l.start as f64, (l.end as f64 - span).max(l.start as f64));
        let center = self.center.unwrap_or((l.start as f64 + l.end as f64) / 2.0);
        let a = (center - span / 2.0).clamp(lo, hi);
        (a, a + span)
    }

    /// Centers on the middle of the frames `from..to` (a slice), for the next zoom.
    pub fn select(&mut self, from: u64, to: u64) {
        self.center = Some((from as f64 + to as f64) / 2.0);
    }

    pub fn zoom_to(&mut self, zoom: usize) {
        self.zoom = zoom.min(tk::BENCH_ZOOMS.len() - 1);
    }

    /// Scrolls the view so it starts at `start` (clamped to the loop).
    pub fn scroll_to(&mut self, l: &Loop, start: f64) {
        let span = self.span(l);
        let a = start.clamp(l.start as f64, (l.end as f64 - span).max(l.start as f64));
        self.center = Some(a + span / 2.0);
    }

    /// The x of `frame` across a well `left..left + width` wide.
    pub fn x_of(&self, l: &Loop, frame: f64, left: f32, width: f32) -> f32 {
        let (a, b) = self.range(l);
        left + ((frame - a) / (b - a).max(f64::MIN_POSITIVE) * f64::from(width)) as f32
    }

    /// The frame under `x` across the same well, rounded to the nearest frame (ties upward, as the
    /// ADR rounds a requested position) and kept in the loop.
    pub fn frame_at(&self, l: &Loop, x: f32, left: f32, width: f32) -> u64 {
        let (a, b) = self.range(l);
        let f = a + f64::from((x - left) / width.max(f32::MIN_POSITIVE)) * (b - a);
        (f + 0.5).floor().clamp(l.start as f64, l.end as f64) as u64
    }
}

/// Milliseconds from S to `frame`, at the source's own rate (the pointer readout, ADR 0041).
pub fn ms_from_start(l: &Loop, frame: u64) -> f64 {
    l.seconds(frame as f64) * 1000.0
}

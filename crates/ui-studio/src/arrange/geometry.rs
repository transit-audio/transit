//! Arrangement coordinate mapping and row layout.

use super::*;

/// Timeline mapping: x ↔ samples, from the session's shared zoom (`edit.zoom`).
#[derive(Clone, Copy)]
pub(crate) struct Map {
    pub(crate) x0: f32,
    pub(crate) scroll: Samples,
    pub(crate) spp: f64,
    /// The grid settings, so every consumer of a map snaps to the lines drawn.
    pub(crate) grid: crate::grid::GridView,
}

impl Map {
    pub(crate) fn x(&self, at: Samples) -> f32 {
        self.x0 + ((at - self.scroll) as f64 / self.spp) as f32
    }
    pub(crate) fn at(&self, x: f32) -> Samples {
        self.scroll + (f64::from(x - self.x0) * self.spp).round() as Samples
    }
}

/// Height of one take lane (slice 6, #25).
pub const TAKE_H: f32 = 22.0;

/// Row geometry: each row's top (content coordinates, before scrolling) and full height. A row is
/// the track's main lane plus its take lanes, then its automation lanes.
pub(crate) struct RowGeo {
    pub tops: Vec<f32>,
    pub heights: Vec<f32>,
    pub main: Vec<f32>,
    pub take: Vec<f32>,
    pub take_count: Vec<usize>,
    pub auto_views: Vec<Vec<crate::automation::LaneView>>,
    pub automation: Vec<Vec<f32>>,
}

impl RowGeo {
    pub(crate) fn new(app: &StudioApp, rows: &[&Track]) -> RowGeo {
        let mut tops = Vec::with_capacity(rows.len());
        let mut heights = Vec::with_capacity(rows.len());
        let mut main = Vec::with_capacity(rows.len());
        let mut take = Vec::with_capacity(rows.len());
        let mut automation = Vec::with_capacity(rows.len());
        let mut take_count = Vec::with_capacity(rows.len());
        let mut auto_views = Vec::with_capacity(rows.len());
        let mut acc = 0.0;
        for t in rows {
            let rh = t.height.points();
            let th = (TAKE_H * rh / 46.0).clamp(22.0, 220.0);
            let views = app.lane_views(t);
            let ah: Vec<f32> = views.iter().map(|v| crate::automation::lane_height::display(v.height, rh)).collect();
            let count = takes_shown(app, t);
            let h = rh + count as f32 * th + ah.iter().sum::<f32>();
            take_count.push(count);
            auto_views.push(views);
            main.push(rh);
            take.push(th);
            automation.push(ah);
            tops.push(acc);
            heights.push(h);
            acc += h;
        }
        RowGeo { tops, heights, main, take, take_count, auto_views, automation }
    }
    pub(crate) fn total(&self) -> f32 {
        self.tops.last().zip(self.heights.last()).map_or(0.0, |(t, h)| t + h)
    }
    /// The row at content height `y`.
    pub(crate) fn index_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        (0..self.tops.len()).find(|i| y >= self.tops[*i] && y < self.tops[*i] + self.heights[*i])
    }
}

/// A row in the track list: not hidden, and not in a collapsed group (view state, slice 2c).
pub(crate) fn shown(app: &StudioApp, s: &Session, t: &Track) -> bool {
    !t.hidden && !app.tucked(s, t)
}

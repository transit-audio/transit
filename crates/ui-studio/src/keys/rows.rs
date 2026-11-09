//! The row map under a gesture (#421). The layout holds still from a press in the pane's body to
//! its release; a dragged note sits on the row under the pointer, folded or not; new notes take the
//! row's pitch; the folded view draws a line on every row boundary.

use super::*;

impl StudioApp {
    /// This frame's row map: `fresh`, or the map from the frame a press landed in `body` (the
    /// keyboard, the grid and the velocity lane), held through that press's release frame.
    pub(super) fn keys_rows(&mut self, ui: &Ui, body: Rect, fresh: scale::Rows) -> scale::Rows {
        let (pressed, down, released, at) =
            ui.input(|i| (i.pointer.any_pressed(), i.pointer.any_down(), i.pointer.any_released(), i.pointer.interact_pos()));
        if !down && !released {
            self.keys.rows_held = None;
        }
        if pressed && self.keys.rows_held.is_none() && at.is_some_and(|p| body.contains(p)) {
            self.keys.rows_held = Some(fresh);
        }
        self.keys.rows_held.unwrap_or(fresh)
    }
}

/// The pointer's height in rank units: row `r` spans `(r - 1, r]`, its center at `r - 0.5`.
fn rank_at(map: &Map, y: f32) -> f32 {
    map.rows.rank(map.top) - (y - map.gy0) / map.kh
}

/// Where a vertical note drag puts `pitches` with the pointer at `y`; `anchor` is the grabbed note.
///
/// Folded, the rows are the units: the anchor goes to the row under the pointer and every note
/// moves by the same number of rows. Unfolded with Scale mode off, the same in semitones. Unfolded
/// with Scale mode on, the anchor goes to the scale row nearest the pointer (it moves when the
/// pointer passes the midpoint between two scale rows) and the group keeps the existing rule of
/// [`transit_midi::scale::KeyScale::transpose_group`]: every note moves by the same number of
/// degrees and an accidental keeps its offset above its scale tone.
pub(super) fn drag_pitches(map: &Map, pitches: &[u8], anchor: u8, y: f32) -> Vec<u8> {
    let rows = &map.rows;
    let lowest = pitches.iter().copied().min().unwrap_or(0);
    let anchor = if pitches.contains(&anchor) { anchor } else { lowest };
    let u = rank_at(map, y);
    if rows.folded() {
        let rank = |p: u8| rows.rank(f32::from(p)) as i32;
        let lo = pitches.iter().map(|p| rank(*p)).min().unwrap_or(0);
        let hi = pitches.iter().map(|p| rank(*p)).max().unwrap_or(0);
        let dr = (u.ceil() as i32 - rank(anchor)).clamp(-lo, (rows.count() - 1 - hi).max(-lo));
        return pitches.iter().map(|p| rows.pitch((rank(*p) + dr) as f32).clamp(0.0, 127.0) as u8).collect();
    }
    if !map.scale.enabled {
        let ds = (u.ceil() as i64 - i64::from(anchor)).clamp(-i64::from(lowest), 127 - i64::from(pitches.iter().copied().max().unwrap_or(127)));
        return pitches.iter().map(|p| (i64::from(*p) + ds) as u8).collect();
    }
    let key = map.scale.key;
    let tones: Vec<i64> = (0..=127u8).filter(|p| key.contains(*p)).map(i64::from).collect();
    if tones.is_empty() {
        return pitches.to_vec();
    }
    // The anchor as a scale tone plus a chromatic offset (as `transpose_group` splits it).
    let index = tones.partition_point(|t| *t <= i64::from(anchor)).saturating_sub(1);
    let offset = i64::from(anchor) - tones[index];
    // The pointer in pitch units: row p spans (p - 0.5, p + 0.5].
    let q = f64::from(u) + 0.5;
    let best = (0..tones.len())
        .filter(|j| (0..=127).contains(&(tones[*j] + offset)))
        .min_by(|a, b| {
            let d = |j: usize| ((tones[j] + offset) as f64 - q).abs();
            // A pointer exactly between two scale rows leaves the note on its own side.
            d(*a).total_cmp(&d(*b)).then(a.abs_diff(index).cmp(&b.abs_diff(index)))
        })
        .unwrap_or(index);
    key.transpose_group(pitches, anchor, best as i64 - index as i64)
}

/// The pitch a click or draw on `pitch`'s row enters: the row's own pitch, except an out-of-scale
/// row of the unfolded view with Scale mode on, which gives the nearest scale tone.
pub(super) fn entry_pitch(map: &Map, pitch: u8) -> u8 {
    if map.scale.enabled && !map.rows.folded() { map.scale.key.nearest(pitch) } else { pitch }
}

/// The line under `pitch`'s row: the octave line under the lowest visible row of each octave (C in
/// the unfolded view, as before), and in the folded view a row line under every other row, in the
/// keyboard column's row ink. The unfolded view tells rows apart by their fills and has no row lines.
pub(super) fn row_line(rows: &scale::Rows, pitch: u8) -> Option<Stroke> {
    let below = (0..pitch).rev().find(|p| rows.contains(*p));
    if below.is_none_or(|b| b / 12 != pitch / 12) {
        Some(Stroke::new(1.0, TOKENS.grid_bar))
    } else if rows.folded() {
        Some(Stroke::new(1.0, TOKENS.line.gamma_multiply(0.5)))
    } else {
        None
    }
}

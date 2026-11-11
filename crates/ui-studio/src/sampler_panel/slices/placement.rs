//! The placement tick `at` the Slices view passes to Detect, the state query and Slice to MIDI
//! (ADR 0041 amendment 2026-11-08, items 5 and 9: the engine never guesses UI state, so the UI
//! computes it and passes it explicitly, always). In order:
//!
//! 1. the start of the arrangement's time selection when it includes the target track (a
//!    time-range selection's start; with only clips selected, the earliest start among the
//!    track's selected clips);
//! 2. else the edit cursor (the selection when it is a point);
//! 3. else, when there is no edit cursor (a time range that leaves the track out), the
//!    transport's current position, the playhead.
//!
//! The third step is the coordinator's ruling of 2026-11-10 for the ADR's clarifying amendment
//! (the amendment's draft said "bar 1"). The placement line names which one was used.
//!
//! While the transport runs, a playhead placement moves on in whole sixteenths (the placement
//! line's own unit, floored), so the state query and the previews that read it are asked again at
//! most once a sixteenth, never every frame.

use crate::StudioApp;
use transit_model::TrackId;

/// Which rule gave the placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    SelectionStart,
    EditCursor,
    Playhead,
}

impl Origin {
    /// Its words in the placement line.
    pub fn text(self) -> &'static str {
        match self {
            Origin::SelectionStart => "selection start",
            Origin::EditCursor => "edit cursor",
            Origin::Playhead => "playhead",
        }
    }
}

/// The placement: a tick on the session's timeline and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub tick: i64,
    pub origin: Origin,
}

/// The placement for the instrument on `track`.
pub fn of(app: &StudioApp, track: TrackId) -> Placement {
    let s = app.engine.session();
    let e = &s.edit;
    let sel = e.selection;
    let clip_start = e.selected_clips.iter().filter_map(|c| s.find_clip(*c)).filter(|(t, _)| *t == track).map(|(_, c)| c.start).min();
    let (at, origin) = if sel.end > sel.start && e.selected_tracks.contains(&track) {
        (sel.start, Origin::SelectionStart)
    } else if let Some(start) = clip_start {
        (start, Origin::SelectionStart)
    } else if sel.end == sel.start {
        (sel.start, Origin::EditCursor)
    } else {
        (app.position(), Origin::Playhead)
    };
    let mut tick = s.tempo.samples_to_ticks(at.max(0), s.sample_rate).max(0);
    if origin == Origin::Playhead && app.is_playing() {
        let sixteenth = transit_time::TICKS_PER_QUARTER / 4;
        tick -= tick.rem_euclid(sixteenth);
    }
    Placement { tick, origin }
}

/// The placement line: `at 5.1.1 · selection start · active playlist` (bar, beat, sixteenth).
pub fn line(app: &StudioApp, p: Placement) -> String {
    let s = app.engine.session();
    let bb = s.tempo.bar_beat_at_tick(p.tick);
    let sixteenth = transit_time::TICKS_PER_QUARTER / 4;
    format!("at {}.{}.{} · {} · active playlist", bb.bar, bb.beat, bb.tick / sixteenth + 1, p.origin.text())
}

/// The bar, beat and sixteenth of `tick` alone (`5.1.1`), for the Success line.
pub fn position(app: &StudioApp, tick: i64) -> String {
    let s = app.engine.session();
    let bb = s.tempo.bar_beat_at_tick(tick);
    let sixteenth = transit_time::TICKS_PER_QUARTER / 4;
    format!("{}.{}.{}", bb.bar, bb.beat, bb.tick / sixteenth + 1)
}

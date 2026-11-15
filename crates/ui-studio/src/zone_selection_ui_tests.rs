//! The UI's readers of the stored selection and the pre-count zone (ADR 0052 decision 7, slice
//! 7): the Bounce dialog's selection range, Play Edit and Step input read a range in a zone as it
//! is, and floor one that starts before the session start (its zone gone) at the session start.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::{Services, StudioApp};
use serde_json::json;
use transit_midi::Sequence;
use transit_model::{ChannelFormat, Clip, ClipId, Session, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (the default session).
const BAR: Samples = 96_000;
const BEAT: Samples = BAR / 4;

/// A studio with a one-bar MIDI clip at `zone` when given (before bar 1), else at bar 1.
fn app(zone: Option<Samples>) -> (StudioApp, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    transit_engine::edit::place_clip(&mut s, t, Clip::midi(id, "m", zone.unwrap_or(0), BAR, Sequence::default()));
    (StudioApp::new(transit_engine::Engine::new(s), None, Services::default()), id)
}

#[test]
fn the_bounce_dialogs_selection_range_reads_from_the_session_start() {
    use crate::finish::bounce_range;
    let (mut gone, _) = app(None);
    gone.engine.session_mut().edit.selection = Range::new(-BAR, BAR);
    assert_eq!(bounce_range(gone.engine.session()), Range::new(0, BAR), "the zone has gone: from bar 1");
    let (mut zone, _) = app(Some(-BAR));
    zone.engine.session_mut().edit.selection = Range::new(-BEAT, BAR);
    assert_eq!(bounce_range(zone.engine.session()), Range::new(-BEAT, BAR), "a selection in the zone bounces from there");
}

#[test]
fn play_edit_plays_the_selection_from_the_session_start() {
    let (mut app, _) = app(None);
    app.engine.session_mut().edit.selection = Range::new(-BAR, BAR);
    app.run("transport.play_selection", json!({})).unwrap();
    app.handle_transport(0.016);
    assert!(app.engine.transport.playing);
    // The simulated transport has run one 16 ms frame (768 samples) from where Play Edit started.
    assert_eq!(app.engine.transport.position, 768, "Play Edit starts at bar 1 once the zone has gone");
}

/// Step input in a clip in the zone starts at the arrangement's insertion point inside it.
#[test]
fn step_input_in_a_zone_clip_starts_at_the_insertion_point() {
    let (mut app, id) = app(Some(-BAR));
    app.engine.session_mut().edit.selection = Range::point(-BAR + BEAT);
    app.keys.clip = Some(id);
    app.step_toggle();
    assert!(app.keys.step.on);
    assert_eq!(app.keys.step.cursor, Q, "one beat into the clip");
}

//! #707: after a recording whose take starts before the view's left edge because of notes played
//! during the count-in (the pickups), the view scrolls back to the take's start, on the take that
//! makes the pre-count zone and on a later one (ADR 0052 decision 4). A take that starts in sight,
//! or one that starts where recording began, leaves the view where it is. The count-in pass is
//! synthetic, as in `midi_pickup_tests`.

use crate::StudioApp;
use crate::midi_pickup_tests::{B, BEAT, LATCH, app, off, on, pass, record};
use serde_json::json;
use transit_model::TrackId;
use transit_time::Samples;

/// The view's left edge as the arrangement draws it: the stored scroll, floored at the session
/// start (`arrange::arrangement`).
fn left_edge(app: &StudioApp) -> Samples {
    let s = app.engine.session();
    s.edit.zoom.scroll.max(s.start())
}

/// Record from `start` after a one-bar count-in with a pickup on the count-in's first beat and a
/// note on the downbeat; the take starts at the count-in start, `start - 4 * BEAT`.
fn pickup_take(app: &mut StudioApp, t: TrackId, start: Samples, key: u8) {
    let events = vec![on(LATCH - 4 * B, t, key), off(LATCH - 3 * B, t, key), on(LATCH, t, 72), off(LATCH + B, t, 72)];
    record(app, start, 4 * BEAT, None, start + 2 * BEAT, pass(t, start, events));
}

fn take_start(app: &StudioApp, t: TrackId) -> Samples {
    app.engine.session().track(t).unwrap().clips().iter().map(|c| c.start).min().unwrap()
}

#[test]
fn the_take_that_makes_the_zone_brings_its_pickup_into_view() {
    let (mut app, t) = app();
    assert_eq!(left_edge(&app), 0, "a new session shows bar 1");
    pickup_take(&mut app, t, 0, 60);
    assert_eq!(take_start(&app, t), -4 * BEAT, "the take starts at the count-in start");
    assert_eq!(left_edge(&app), -4 * BEAT, "the view shows the pickup, not bar 1");
}

#[test]
fn a_later_take_in_the_zone_brings_its_pickup_into_view() {
    let (mut app, t) = app();
    pickup_take(&mut app, t, 0, 60);
    // The user scrolls back to bar 1 and records over it again.
    app.run("view.scroll", json!({"to": 0})).unwrap();
    assert_eq!(left_edge(&app), 0);
    pickup_take(&mut app, t, 0, 62);
    assert_eq!(left_edge(&app), -4 * BEAT, "the second take's pickup is in sight too");
}

#[test]
fn a_take_that_starts_in_sight_or_at_the_record_start_leaves_the_view() {
    // The pickup lands after the left edge: nothing to bring into view.
    let (mut app, t) = app();
    pickup_take(&mut app, t, 8 * BEAT, 60);
    assert_eq!(take_start(&app, t), 4 * BEAT);
    assert_eq!(left_edge(&app), 0, "the take's start was already in sight");

    // A take with no pickup starts where recording began, which the playhead passed: the view the
    // playhead paged to stays, as before.
    let (mut app, t) = self::app();
    app.run("view.scroll", json!({"to": 16 * BEAT})).unwrap();
    let events = vec![on(LATCH, t, 60), off(LATCH + B, t, 60)];
    record(&mut app, 0, 4 * BEAT, None, 20 * BEAT, pass(t, 0, events));
    assert_eq!(take_start(&app, t), 0);
    assert_eq!(left_edge(&app), 16 * BEAT, "no pickup: the view is not moved");
}

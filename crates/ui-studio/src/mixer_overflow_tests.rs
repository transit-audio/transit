//! A full-height section past its rows (#326, Strip details (12)): the rows scroll inside the
//! fixed box, the add row stays pinned with the hidden count, fades mark the clipped edges, the
//! floating scroll bar, and the wheel scrolls the box only while it overflows.

use crate::tests::*;
use crate::theme::DARK;

fn wheel(app: &mut StudioApp, at: (f32, f32), dy: f32) {
    send_control(app, "ui.scroll", json!({"x": at.0, "y": at.1, "dy": dy, "unit": "point", "steps": 1}));
}

/// Full mixer with track `t` holding `inserts` gain inserts and `sends` sends, plus enough other
/// strips that the mixer itself scrolls sideways.
fn rig(inserts: usize, sends: usize) -> (StudioApp, egui::Context, f64, TrackId) {
    let (mut app, ctx, mut time, t) = mixer_app();
    for _ in 0..inserts {
        app.add_device(t, "gain").unwrap();
    }
    for _ in 0..sends {
        app.new_fx_channel(Some(t), 0.0).unwrap();
    }
    for _ in 0..30 {
        app.run("track.new", json!({"kind": "audio", "format": "stereo"})).unwrap();
    }
    full_mixer(&mut app, &ctx, &mut time);
    (app, ctx, time, t)
}

#[test]
fn five_inserts_scroll_inside_the_fixed_box_with_the_add_row_pinned() {
    let (mut app, ctx, mut time, t) = rig(5, 0);
    let s = strip(&app, t);
    let ov = s.inserts_overflow.clone().expect("five inserts overflow");
    let body = s.inserts_section;
    // The box keeps its size: every fader still lines up with a strip that has none.
    let other = app.mixer.layout.strips.iter().find(|x| x.track != t.0).unwrap().clone();
    assert_eq!(
        (body[3] - body[1], s.fader),
        (other.inserts_section[3] - other.inserts_section[1], [s.fader[0], other.fader[1], s.fader[2], other.fader[3]])
    );
    assert!((ov.view[3] - ov.view[1] - 72.0).abs() < 0.01, "the view holds three rows (3 x 22 + 2 x 3)");
    assert_eq!((ov.hidden, ov.count_text.as_str()), (2, "2 more"));
    assert_eq!(s.inserts.len(), 5, "every insert is a row, two of them clipped");
    let add = s.add_insert;
    assert!(add[1] >= ov.view[3], "the add row sits under the view");
    assert!(ov.count_rect[0] > add[0] && ov.count_rect[2] <= add[2] - 5.9, "the count right-aligned in the add row");
    assert!(ov.fade_bottom.is_some() && ov.fade_top.is_none(), "at rest only the bottom edge fades");
    assert_eq!(ov.fade_bottom.unwrap()[3] - ov.fade_bottom.unwrap()[1], 12.0, "a 12 px fade");
    assert_eq!((ov.thumb_width, ov.thumb_color, ov.gutter), (4.0, DARK.ink_ghost, 6.0), "the floating bar at rest");
    // The wheel over the box scrolls its rows and not the mixer.
    let at = ((ov.view[0] + ov.view[2]) / 2.0, (ov.view[1] + ov.view[3]) / 2.0);
    wheel(&mut app, at, -200.0);
    frames(&mut app, &ctx, 8, &mut time);
    let s = strip(&app, t);
    let ov = s.inserts_overflow.clone().unwrap();
    assert!((ov.offset - ov.max).abs() < 0.01 && ov.max > 0.0, "scrolled to the end: {ov:?}");
    assert_eq!(app.mixer.hscroll, 0.0, "the mixer did not scroll");
    assert!(ov.fade_top.is_some() && ov.fade_bottom.is_none(), "scrolled to the end, only the top fades");
    assert_eq!((ov.thumb_width, ov.thumb_color, ov.gutter), (8.0, DARK.ink_arrow, 10.0), "hovered: 8 px `ink.arrow`");
    assert!((s.inserts[4].rect[3] - ov.view[3]).abs() < 0.01, "the last insert sits at the view's bottom");
    assert_eq!(s.add_insert, add, "the add row stays pinned");
}

#[test]
fn three_sends_scroll_and_the_send_add_row_counts_the_hidden_one() {
    let (app, _ctx, _time, t) = rig(0, 3);
    let s = strip(&app, t);
    let ov = s.sends_overflow.clone().expect("three sends overflow");
    assert!((ov.view[3] - ov.view[1] - 47.0).abs() < 0.01, "the view holds two rows");
    assert_eq!((ov.hidden, ov.count_text.as_str(), s.sends.len()), (1, "1 more", 3));
    assert!(s.add_send[1] >= ov.view[3]);
    assert!(s.inserts_overflow.is_none(), "no inserts, no inserts overflow");
}

#[test]
fn a_section_that_fits_builds_no_box_and_the_wheel_goes_to_the_mixer() {
    // Negative control: three inserts and two sends fit, so neither section scrolls, nothing
    // counts and the wheel over the inserts scrolls the strips.
    let (mut app, ctx, mut time, t) = rig(3, 2);
    let s = strip(&app, t);
    assert!(s.inserts_overflow.is_none() && s.sends_overflow.is_none());
    assert_eq!((s.inserts.len(), s.sends.len()), (3, 2));
    let body = s.inserts_section;
    let at = ((body[0] + body[2]) / 2.0, (s.inserts[1].rect[1] + s.inserts[1].rect[3]) / 2.0);
    wheel(&mut app, at, -150.0);
    frames(&mut app, &ctx, 8, &mut time);
    assert!(app.mixer.hscroll > 0.0, "a box that fits leaves the wheel to the mixer");
}

fn ctrl_wheel(app: &mut StudioApp, at: (f32, f32), dy: f32) {
    send_control(app, "ui.scroll", json!({"x": at.0, "y": at.1, "dy": dy, "unit": "point", "steps": 1, "ctrl": true}));
}

#[test]
fn the_plain_wheel_over_a_send_level_in_an_overflowing_box_scrolls_the_box_and_leaves_the_send() {
    // Negative control: before the box consumed the plain wheel, the row's control read the same
    // events and the send moved 0.0 to -0.08 dB while the box scrolled.
    let (mut app, ctx, mut time, t) = rig(0, 3);
    let s = strip(&app, t);
    assert_eq!(s.sends_overflow.as_ref().unwrap().offset, 0.0);
    let (level, readout) = (s.sends[0].level_rect, s.sends[0].readout_rect);
    for at in [mid(level), mid(readout)] {
        wheel(&mut app, at, -10.0);
        frames(&mut app, &ctx, 8, &mut time);
    }
    let s = strip(&app, t);
    let ov = s.sends_overflow.clone().unwrap();
    assert!(ov.offset > 0.0, "the box scrolled: {ov:?}");
    assert!(s.sends.iter().all(|r| r.level_db == 0.0), "no send moved: {:?}", s.sends.iter().map(|r| r.level_db).collect::<Vec<_>>());
    assert_eq!(app.mixer.hscroll, 0.0, "the mixer did not scroll");
}

#[test]
fn ctrl_wheel_over_a_send_level_in_an_overflowing_box_edits_the_send_and_does_not_scroll() {
    let (mut app, ctx, mut time, t) = rig(0, 3);
    let level = strip(&app, t).sends[0].level_rect;
    ctrl_wheel(&mut app, mid(level), -40.0);
    frames(&mut app, &ctx, 8, &mut time);
    let s = strip(&app, t);
    assert_eq!(s.sends_overflow.as_ref().unwrap().offset, 0.0, "Ctrl+wheel does not scroll the box");
    assert!(s.sends[0].level_db < 0.0, "Ctrl+wheel nudged the send: {}", s.sends[0].level_db);
    assert_eq!(app.mixer.hscroll, 0.0, "the mixer did not scroll");
}

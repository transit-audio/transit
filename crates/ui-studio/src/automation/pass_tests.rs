use super::*;

#[test]
fn write_records_the_fader_while_playing_as_one_undo_step() {
    let mut t = T::new();
    t.app.ui.thin_after_write = false;
    let a = t.with_lane();
    t.click(mid(t.lane(a, "volume").mode));
    t.menu("Write");
    t.app.ui.show_mixer = true;
    t.frames(3);
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == a.0).unwrap().clone();
    let before = t.undo_len();
    t.key("Space");
    assert!(t.app.is_playing());
    let f = strip.fader;
    t.input("ui.drag", json!({"x": (f[0] + f[2]) / 2.0, "y": f[3] - 30.0, "to_x": (f[0] + f[2]) / 2.0, "to_y": f[1] + 10.0, "steps": 20}));
    let writing = t.app.auto.layout.writing;
    t.key("Space");
    assert!(!t.app.is_playing());
    t.frames(3);
    assert!(writing, "the pass was writing");
    let pts = t.points(a, &AutoParam::Volume);
    assert!(pts.len() >= 10, "points every 20 ms of playback: {}", pts.len());
    let lo = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let hi = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    assert!(hi - lo > 3.0, "the curve follows the fader: {lo}..{hi}");
    assert!(pts.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(t.undo_len(), before + 1, "the pass and the fader drag are one undo step");
    assert_eq!(t.lane(a, "volume").points.len(), pts.len(), "the arrangement shows the written curve");
    // One undo removes the pass.
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.points(a, &AutoParam::Volume).is_empty());
}

#[test]
fn touch_writes_only_while_the_control_is_held_and_read_writes_nothing() {
    let mut t = T::new();
    let a = t.with_lane();
    // Read: playing writes nothing.
    t.key("Space");
    t.frames(20);
    t.key("Space");
    assert!(t.points(a, &AutoParam::Volume).is_empty(), "Read never writes");
    t.click(mid(t.lane(a, "volume").mode));
    t.menu("Touch");
    t.key("Space");
    t.frames(20);
    assert!(t.points(a, &AutoParam::Volume).is_empty(), "Touch writes nothing until a control is held");
    let v = t.row(a).volume;
    t.input("ui.drag", json!({"x": v[0] + 10.0, "y": (v[1] + v[3]) / 2.0, "to_x": v[0] + 60.0, "to_y": (v[1] + v[3]) / 2.0, "steps": 20}));
    let n = t.points(a, &AutoParam::Volume).len();
    assert!(n >= 5, "the header volume drag wrote points: {n}");
    t.frames(30);
    assert_eq!(t.points(a, &AutoParam::Volume).len(), n, "released: Touch stops writing");
    t.key("Space");
}

fn write_mode(t: &mut T) -> TrackId {
    let a = t.with_lane();
    t.click(mid(t.lane(a, "volume").mode));
    t.menu("Write");
    a
}

#[test]
fn a_forward_locate_during_write_keeps_the_points_it_skips() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    let ahead = [400_000_i64, 450_000, 500_000, 550_000];
    for &at in &ahead {
        set_point(&mut t, a, at, -3.0);
    }
    t.key("Space");
    t.frames(30);
    t.app.run("transport.locate", json!({"at": 900_000})).unwrap();
    t.frames(30);
    t.key("Space");
    t.frames(3);
    let pts = t.points(a, &AutoParam::Volume);
    for at in ahead {
        assert!(pts.iter().any(|p| p.0 == at), "point at {at} survived the locate: {pts:?}");
    }
    assert!(pts.iter().any(|p| p.0 >= 900_000), "the pass continued from the new position");
}

#[test]
fn a_still_fader_writes_no_points_after_the_first() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    t.key("Space");
    t.frames(60);
    t.key("Space");
    t.frames(3);
    let pts = t.points(a, &AutoParam::Volume);
    assert!(!pts.is_empty(), "the first point is written");
    assert!(pts.len() <= 2, "a still fader adds nothing (first point and the hold's end): {}", pts.len());
}

/// A ramp of the fader, a straight line in dB, while playing: the control moves a little every
/// other frame, as a hand on it would.
fn ramp(t: &mut T, a: TrackId, n: usize) {
    for i in 0..n {
        t.app.set_value_merged(crate::fine::Target::Fader(a), -30.0 + i as f32 * 0.1, None).unwrap();
        t.frames(2);
    }
}

/// A Write pass with a ramp. Returns (points written, undo steps added).
fn write_pass_with_ramp(t: &mut T, a: TrackId) -> (usize, usize) {
    let before = t.undo_len();
    t.key("Space");
    ramp(t, a, 120);
    let written = t.app.auto.layout.points_written;
    t.key("Space");
    t.frames(3);
    (written, t.undo_len() - before)
}

#[test]
fn a_write_pass_is_thinned_when_it_ends_as_part_of_the_pass() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    assert!(t.app.ui.thin_after_write, "thinning is on by default");
    let (written, steps) = write_pass_with_ramp(&mut t, a);
    let pts = t.points(a, &AutoParam::Volume);
    assert!(written >= 60, "a dense pass: {written}");
    assert!(pts.len() < written / 3 && pts.len() >= 2, "thinned to a few: {} of {written}", pts.len());
    assert_eq!(steps, 1, "the pass and its thinning are one undo step");
    assert!(t.app.auto.thinned > 0);
    // One undo removes the whole pass.
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.points(a, &AutoParam::Volume).is_empty(), "undo takes the pass and the thinning together");
    // Redo brings back the thinned curve, not the dense one.
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume).len(), pts.len());
}

#[test]
fn thin_after_write_off_keeps_the_dense_pass_and_the_menu_toggles_it() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    t.app.ui.thin_after_write = false;
    let (written, steps) = write_pass_with_ramp(&mut t, a);
    assert_eq!(steps, 1);
    assert!(t.points(a, &AutoParam::Volume).len() >= written.saturating_sub(2), "every point of the pass stays");
    // The menu entry flips the preference and Thin Focused Track thins on demand, as its own step.
    let r = t.app.arrange.bar_menu.iter().find(|m| m.label == "Automation").unwrap().rect;
    t.click(mid(r));
    let on = t.app.arrange.bar_menu.iter().find(|m| m.label == "Thin after write").unwrap().rect;
    t.click(mid(on));
    assert!(t.app.ui.thin_after_write, "the menu turned it on");
    assert!(!t.app.engine.session().track(a).unwrap().automation.is_empty());
    t.app.focus(a);
    let before = t.undo_len();
    let n = t.app.thin_focused().unwrap();
    assert!(n > 0);
    assert_eq!(t.undo_len(), before + 1);
}

#[test]
fn leaving_write_mid_pass_thins_that_track_and_a_toggle_survives_ui_json() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    t.key("Space");
    ramp(&mut t, a, 120);
    let dense = t.points(a, &AutoParam::Volume).len();
    t.app.run("mix.automation_mode", json!({"tracks": [a.0], "mode": "read"})).unwrap();
    t.frames(5);
    let thin = t.points(a, &AutoParam::Volume).len();
    t.key("Space");
    assert!(thin < dense / 3, "thinned when Write ended: {thin} of {dense}");
    let json = serde_json::to_string(&t.app.ui).unwrap();
    assert!(json.contains("\"thin_after_write\":true"));
    let old: crate::UiState = serde_json::from_str("{}").unwrap();
    assert!(old.thin_after_write, "an older ui.json thins by default");
}

// ---- Review fixes: thin only what the pass wrote, one undo step, follow suspended by the header ----

/// A second audio track in Write mode, without a lane open.
fn write_track(t: &mut T, name: &str) -> TrackId {
    let b = t.track("audio", name);
    t.app.set_automation_mode(b, AutomationMode::Write).unwrap();
    t.frames(3);
    b
}

#[test]
fn a_default_on_pass_keeps_the_curve_shape() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    assert!(t.app.ui.thin_after_write);
    let (written, _) = write_pass_with_ramp(&mut t, a);
    let pts = t.points(a, &AutoParam::Volume);
    assert!(pts.len() >= 2 && pts.len() < written / 3);
    // The pass starts at the static 0 dB, then the ramp runs -30 .. -18; both ends of it survive.
    assert!(pts[0].1.abs() < 0.2, "the first point survives: {}", pts[0].1);
    assert!((pts.last().unwrap().1 - -18.1).abs() < 0.5, "the last point survives: {}", pts.last().unwrap().1);
    assert!(pts.iter().any(|p| (p.1 - -30.0).abs() < 0.2), "the ramp's start survives: {pts:?}");
    assert!(pts.iter().all(|p| p.1 >= -30.2 && p.1 <= 0.2), "values stay inside the pass's range: {pts:?}");
}

#[test]
fn thinning_leaves_hand_points_outside_the_pass_and_other_lanes_alone() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    // Collinear hand points a whole-lane thin would drop: later on the Volume lane, and on Pan.
    for at in [8_000_000_i64, 8_500_000, 9_000_000] {
        set_point(&mut t, a, at, -12.0);
        t.app.run("automation.set_point", json!({"track": a.0, "param": "pan", "at": at, "value": 0.25})).unwrap();
    }
    t.frames(3);
    write_pass_with_ramp(&mut t, a);
    let vol = t.points(a, &AutoParam::Volume);
    for at in [8_000_000_i64, 8_500_000, 9_000_000] {
        assert!(vol.iter().any(|p| p.0 == at), "hand point {at} on the written lane survives: {vol:?}");
    }
    let pan = t.points(a, &AutoParam::Pan(0));
    assert_eq!(pan.len(), 3, "the Pan lane was not written, so it is not thinned: {pan:?}");
}

#[test]
fn leaving_write_through_the_ui_mid_pass_is_still_one_undo_step() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    let before = t.undo_len();
    t.key("Space");
    ramp(&mut t, a, 120);
    let dense = t.points(a, &AutoParam::Volume).len();
    t.app.set_automation_mode(a, AutomationMode::Read).unwrap();
    t.frames(5);
    t.key("Space");
    t.frames(3);
    assert!(t.points(a, &AutoParam::Volume).len() < dense / 3, "thinned");
    assert_eq!(t.undo_len(), before + 1, "pass, mode change and thinning are one step: {:?}", t.app.engine.undo_history());
}

#[test]
fn an_unrelated_edit_before_stop_leaves_the_thin_as_its_own_labeled_step() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    let before = t.undo_len();
    t.key("Space");
    ramp(&mut t, a, 120);
    let dense = t.points(a, &AutoParam::Volume).len();
    t.app.run("track.rename", json!({"track": a.0, "name": "Lead"})).unwrap();
    t.key("Space");
    t.frames(3);
    let h = t.app.engine.undo_history();
    assert_eq!(h.len(), before + 3, "pass, rename, thin: {h:?}");
    assert!(h.last().unwrap().contains("Thin"), "the thin step is labeled: {h:?}");
    assert!(t.app.status.contains("own step"), "{}", t.app.status);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume).len(), dense, "undoing the thin restores the dense pass");
}

#[test]
fn stop_after_a_pass_is_one_step_even_with_the_pass_labeled() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    let (_, steps) = write_pass_with_ramp(&mut t, a);
    assert_eq!(steps, 1);
    assert_eq!(t.app.engine.undo_history().last().unwrap(), "Write Automation");
}

#[test]
fn a_track_deleted_as_the_pass_ends_does_not_stop_the_others_thinning() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    let b = write_track(&mut t, "Gone");
    t.key("Space");
    ramp(&mut t, a, 120);
    let dense = t.points(a, &AutoParam::Volume).len();
    assert!(!t.points(b, &AutoParam::Volume).is_empty(), "the second track was written too");
    t.app.run("track.delete", json!({"tracks": [b.0]})).unwrap();
    t.key("Space");
    t.frames(3);
    assert!(t.points(a, &AutoParam::Volume).len() < dense / 3, "track a still thinned");
}

#[test]
fn thin_focused_track_waits_while_a_pass_is_running_on_it() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    t.app.focus(a);
    t.key("Space");
    ramp(&mut t, a, 40);
    let n = t.points(a, &AutoParam::Volume).len();
    assert!(t.app.thin_focused().is_err(), "refused while the pass writes to the track");
    assert_eq!(t.points(a, &AutoParam::Volume).len(), n);
    t.key("Space");
    t.frames(3);
}

// ---- Re-review: undo during a pass, the second touch ----

fn touch_mode(t: &mut T) -> TrackId {
    let a = t.with_lane();
    t.app.set_automation_mode(a, AutomationMode::Touch).unwrap();
    t.frames(3);
    a
}

/// Hold the track header's volume and drag it sideways for `n` steps, then release.
fn touch_drag(t: &mut T, a: TrackId, dx: f32, n: usize) {
    let v = t.row(a).volume;
    let (x, y) = ((v[0] + v[2]) / 2.0, (v[1] + v[3]) / 2.0);
    t.input("ui.drag", json!({"x": x, "y": y, "to_x": x + dx, "to_y": y, "steps": n}));
    // One synthetic event per frame: let the release arrive.
    t.frames(n + 5);
}

#[test]
fn undoing_a_pass_while_playing_then_stopping_adds_no_thin_step_and_keeps_redo() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    let before = t.undo_len();
    t.key("Space");
    touch_drag(&mut t, a, 50.0, 30);
    let written = t.points(a, &AutoParam::Volume);
    assert!(written.len() >= 5);
    assert_eq!(t.undo_len(), before + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    let len = t.undo_len();
    assert!(t.app.engine.can_redo());
    t.key("Space");
    t.frames(3);
    assert_eq!(t.undo_len(), len, "Stop adds no step: {:?}", t.app.engine.undo_history());
    assert!(t.app.engine.can_redo(), "the redo stack survives Stop");
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume), written, "Redo restores the pass as written");
}

#[test]
fn a_thin_that_removes_nothing_leaves_history_and_redo_alone() {
    let mut t = T::new();
    let a = write_mode(&mut t);
    // A still fader: a pass of two points has nothing to thin.
    let before = t.undo_len();
    t.key("Space");
    t.frames(30);
    t.key("Space");
    t.frames(3);
    assert_eq!(t.undo_len(), before + 1, "{:?}", t.app.engine.undo_history());
    // On demand too: sparse lanes, with a redo step waiting.
    set_point(&mut t, a, 500_000, -6.0);
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.app.engine.can_redo());
    t.app.focus(a);
    let len = t.undo_len();
    assert_eq!(t.app.thin_focused().unwrap(), 0);
    assert_eq!(t.undo_len(), len, "no empty Thin step");
    assert!(t.app.engine.can_redo(), "Redo is still there");
}

/// Hand points every 2,000 samples, alternating -3 and -9 dB.
fn comb(t: &mut T, a: TrackId) -> Vec<(i64, f32)> {
    let pts: Vec<(i64, f32)> = (1..=100).map(|k| (k * 2_000, if k % 2 == 0 { -3.0 } else { -9.0 })).collect();
    for (at, v) in &pts {
        set_point(t, a, *at, *v);
    }
    t.frames(3);
    pts
}

#[test]
fn a_second_touch_leaves_the_automation_between_the_touches_alone() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    let hand = comb(&mut t, a);
    t.key("Space");
    touch_drag(&mut t, a, 40.0, 10);
    let released = t.app.pass_spans(a)[0].1;
    t.frames(40);
    touch_drag(&mut t, a, -40.0, 10);
    let spans = t.app.pass_spans(a);
    assert_eq!(spans.len(), 2, "two touches, two spans: {spans:?}");
    let retouched = spans[1].0;
    assert!(retouched - released > 20_000, "a real gap: {released}..{retouched}");
    let gap: Vec<_> = hand.iter().filter(|p| p.0 > released + 1_000 && p.0 < retouched - 1_000).copied().collect();
    assert!(gap.len() >= 8, "{} hand points in the gap", gap.len());
    let alive = |t: &T| t.points(a, &AutoParam::Volume);
    for g in &gap {
        assert!(alive(&t).contains(g), "hand point {g:?} survived the second touch before Stop");
    }
    t.key("Space");
    t.frames(3);
    for g in &gap {
        assert!(alive(&t).contains(g), "hand point {g:?} survived the thin");
    }
}

#[test]
fn latch_keeps_writing_after_release_by_design() {
    let mut t = T::new();
    let a = t.with_lane();
    t.app.set_automation_mode(a, AutomationMode::Latch).unwrap();
    t.frames(3);
    t.key("Space");
    touch_drag(&mut t, a, 40.0, 10);
    let n = t.points(a, &AutoParam::Volume).len();
    t.frames(60);
    assert!(t.points(a, &AutoParam::Volume).len() >= n, "Latch goes on writing (or holding) after release");
    assert_eq!(t.app.pass_spans(a).len(), 1, "one continuous span");
    t.key("Space");
}

/// Drag the strip fader up or down by `dy` points for `n` steps, then let the release arrive.
fn fader_drag(t: &mut T, a: TrackId, dy: f32, n: usize) {
    let f = strip_of(t, a).fader;
    let (x, y) = ((f[0] + f[2]) / 2.0, (f[1] + f[3]) / 2.0);
    t.input("ui.drag", json!({"x": x, "y": y, "to_x": x, "to_y": y + dy, "steps": n}));
    t.frames(n + 5);
}

fn assert_one_pass_step(t: &mut T, a: TrackId, before: usize, thinned_ok: bool) {
    let h = t.app.engine.undo_history();
    assert_eq!(h.len(), before + 1, "a multi-touch pass is one undo step: {h:?}");
    assert_eq!(h.last().unwrap(), "Write Automation", "{h:?}");
    assert!(!t.app.status.contains("because an edit"), "the status text must not blame an edit: {}", t.app.status);
    let thin = t.points(a, &AutoParam::Volume);
    assert!(thinned_ok);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.undo_len(), before, "one Undo removes the whole pass");
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume), thin, "Redo restores the thinned curves");
}

#[test]
fn a_two_touch_pass_on_the_header_volume_is_one_step_after_stop() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    let before = t.undo_len();
    t.key("Space");
    touch_drag(&mut t, a, 100.0, 100);
    t.frames(30);
    touch_drag(&mut t, a, -100.0, 100);
    assert_eq!(t.app.pass_spans(a).len(), 2);
    t.key("Space");
    t.frames(3);
    assert_one_pass_step(&mut t, a, before, true);
}

#[test]
fn a_two_touch_pass_on_the_strip_fader_is_one_step_after_stop() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    t.app.ui.show_mixer = true;
    t.frames(3);
    let before = t.undo_len();
    t.key("Space");
    fader_drag(&mut t, a, -60.0, 100);
    t.frames(30);
    fader_drag(&mut t, a, 40.0, 100);
    assert_eq!(t.app.pass_spans(a).len(), 2, "{:?}", t.app.pass_spans(a));
    t.key("Space");
    t.frames(3);
    assert_one_pass_step(&mut t, a, before, true);
}

#[test]
fn a_latch_pass_with_a_regrab_is_one_step_after_stop() {
    let mut t = T::new();
    let a = t.with_lane();
    t.app.set_automation_mode(a, AutomationMode::Latch).unwrap();
    t.app.ui.show_mixer = true;
    t.frames(3);
    let before = t.undo_len();
    t.key("Space");
    fader_drag(&mut t, a, -60.0, 100);
    t.frames(30);
    fader_drag(&mut t, a, 40.0, 100);
    t.key("Space");
    t.frames(3);
    assert_one_pass_step(&mut t, a, before, true);
}

#[test]
fn a_pass_at_the_history_cap_is_still_one_step() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    for i in 0..205 {
        set_point(&mut t, a, 1_000_000 + i * 1_000, -6.0 - (i % 7) as f32);
    }
    assert_eq!(t.undo_len(), t.app.engine.history_limit, "the history is full");
    t.key("Space");
    touch_drag(&mut t, a, 100.0, 12);
    t.frames(30);
    touch_drag(&mut t, a, -100.0, 12);
    assert_eq!(t.app.pass_spans(a).len(), 2);
    let marker = t.app.engine.undo_history()[t.undo_len() - 2].clone();
    t.key("Space");
    t.frames(3);
    let h = t.app.engine.undo_history();
    assert_eq!(h.last().unwrap(), "Write Automation", "{:?}", &h[h.len() - 3..]);
    assert_ne!(h[h.len() - 2], "Thin", "no Thin step is left under the pass");
    let _ = marker;
    assert!(t.app.auto.thinned > 0, "the pass was thinned at the cap too");
    let thin = t.points(a, &AutoParam::Volume);
    t.app.run("edit.undo", json!({})).unwrap();
    let pts = t.points(a, &AutoParam::Volume);
    assert_eq!(pts.len(), 205, "one Undo removes the whole pass and nothing else: {}", pts.len());
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume), thin);
}

#[test]
fn a_pass_with_nothing_to_thin_is_still_labeled_as_a_pass() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    t.app.ui.show_mixer = true;
    t.frames(3);
    let before = t.undo_len();
    t.key("Space");
    fader_drag(&mut t, a, -8.0, 6);
    t.key("Space");
    t.frames(3);
    assert_eq!(t.undo_len(), before + 1);
    assert_eq!(t.app.engine.undo_history().last().unwrap(), "Write Automation");
}

#[test]
fn a_thin_that_is_its_own_push_at_the_cap_still_folds_into_the_pass() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    for i in 0..205 {
        set_point(&mut t, a, 1_000_000 + i * 1_000, -6.0 - (i % 7) as f32);
    }
    t.key("Space");
    touch_drag(&mut t, a, 100.0, 12);
    // The merge key is lost without a document change (nothing else ran): the thin cannot continue the pass.
    t.app.engine.end_merge();
    t.key("Space");
    t.frames(3);
    let h = t.app.engine.undo_history();
    assert_eq!(h.last().unwrap(), "Write Automation", "{:?}", &h[h.len() - 3..]);
    assert_ne!(h[h.len() - 2], "Thin");
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.points(a, &AutoParam::Volume).len(), 205, "one Undo removes the pass");
}

/// #188: while the header's volume is held on a Touch track the engine knows it (the mix stops
/// reading the lane), playing or not, and the release clears it. Read tracks hold nothing.
#[test]
fn holding_a_control_on_a_touch_track_tells_the_engine_until_release() {
    let mut t = T::new();
    let a = touch_mode(&mut t);
    let touched = |t: &T| t.app.engine.session().track(a).unwrap().mixer.touched.clone();
    let v = t.row(a).volume;
    let (x, y) = ((v[0] + v[2]) / 2.0, (v[1] + v[3]) / 2.0);
    // 60 steps, one event a frame: `input` returns after 30 frames, mid-drag.
    t.input("ui.drag", json!({"x": x, "y": y, "to_x": x + 40.0, "to_y": y, "steps": 60}));
    assert_eq!(touched(&t), vec![AutoParam::Volume], "held");
    t.frames(40);
    assert!(touched(&t).is_empty(), "released");
    t.app.set_automation_mode(a, AutomationMode::Read).unwrap();
    t.frames(3);
    t.input("ui.drag", json!({"x": x, "y": y, "to_x": x - 40.0, "to_y": y, "steps": 60}));
    assert!(touched(&t).is_empty(), "Read: nothing held");
    t.frames(40);
}

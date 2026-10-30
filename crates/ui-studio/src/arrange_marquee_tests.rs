//! Marquee selection across empty lane space (#217). Every wait is on the queue of fed events;
//! results are read from the session's selection set and the marquee's own state.

use crate::arrange::marquee::{Mode, combine, mode};
use crate::tests::*;

const NONE: egui::Modifiers = egui::Modifiers::NONE;

fn shift() -> egui::Modifiers {
    egui::Modifiers { shift: true, ..Default::default() }
}

fn ctrl() -> egui::Modifiers {
    egui::Modifiers { ctrl: true, command: true, ..Default::default() }
}

/// Clips A (track 1, beats 0..4), B (track 1, beats 8..12), C (track 2, beats 4..8).
struct Rig {
    r: CopyRig,
    a: ClipId,
    b: ClipId,
    c: ClipId,
}

impl Rig {
    fn new() -> Rig {
        let mut r = copy_rig();
        let a = ClipId(clips_on(&r.app, r.t)[0].0);
        let bar = 4 * r.beat;
        let mk = |app: &mut crate::StudioApp, t: TrackId, from: i64| {
            let c = app.run("midi.new_clip", json!({"tracks": [t.0], "start": from * bar, "end": (from + 1) * bar})).unwrap();
            ClipId(c["clip"].as_u64().unwrap())
        };
        let b = mk(&mut r.app, r.t, 2);
        let c = mk(&mut r.app, r.t2, 1);
        r.app.run("edit.select_none", json!({})).unwrap();
        frames(&mut r.app, &r.ctx, 3, &mut r.time);
        Rig { r, a, b, c }
    }

    /// The x of a point `beats` into the song (clip A starts at 0).
    fn x(&self, beats: f32) -> f32 {
        let rect = self.r.app.arrange.layout.clips.iter().find(|c| c.id == self.a.0).unwrap().rect;
        rect[0] + beats * self.r.px_per_beat
    }

    /// The vertical middle of a track's lane.
    fn y(&self, t: TrackId) -> f32 {
        let lane = self.r.app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().lane;
        (lane[1] + lane[3]) / 2.0
    }

    fn selected(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.r.app.engine.session().edit.selected_clips.iter().map(|c| c.0).collect();
        v.sort_unstable();
        v
    }

    fn ids(&self, ids: &[ClipId]) -> Vec<u64> {
        let mut v: Vec<u64> = ids.iter().map(|c| c.0).collect();
        v.sort_unstable();
        v
    }

    /// Press in empty space right of C on track 2, drag up and left onto B's lane: touches B and C.
    fn across_two(&mut self, m: egui::Modifiers) {
        let (from, to) = ((self.x(10.0), self.y(self.r.t2)), (self.x(6.0), self.y(self.r.t)));
        hold_drag(&mut self.r.app, &self.r.ctx, &mut self.r.time, from, to, m);
        release_at(&mut self.r.app, &self.r.ctx, &mut self.r.time, to, m);
    }
}

#[test]
fn a_marquee_across_two_tracks_selects_exactly_the_clips_it_touches() {
    let mut g = Rig::new();
    let (from, to) = ((g.x(10.0), g.y(g.r.t2)), (g.x(6.0), g.y(g.r.t)));
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, from, to, NONE);
    let live = g.r.app.arrange.marquee.clone().expect("the marquee is live while the button is down");
    assert!(live.live);
    assert!((live.rect[0] - to.0).abs() < 1.0 && (live.rect[2] - from.0).abs() < 1.0, "{:?}", live.rect);
    assert!(g.selected().is_empty(), "nothing is selected before the release");
    release_at(&mut g.r.app, &g.r.ctx, &mut g.r.time, to, NONE);
    assert!(g.r.app.arrange.marquee.is_none());
    assert_eq!(g.selected(), g.ids(&[g.b, g.c]), "B and C, not A");
}

#[test]
fn the_marquee_selection_is_the_one_a_drag_of_it_moves_and_previews() {
    let mut g = Rig::new();
    g.across_two(NONE);
    assert_eq!(g.selected(), g.ids(&[g.b, g.c]));
    // Grab C (selected) and hold: both selected clips get a ghost (#218), the unselected A does not.
    let c = g.r.app.arrange.layout.clips.iter().find(|c| c.id == g.c.0).unwrap().rect;
    let grab = (c[0] + 20.0, (c[1] + c[3]) / 2.0);
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, grab, (grab.0 + g.r.px_per_beat * 4.0, grab.1), NONE);
    let mut ids: Vec<u64> = g.r.app.arrange.layout.drag_ghosts.iter().map(|x| x.clip).collect();
    ids.sort_unstable();
    assert_eq!(ids, g.ids(&[g.b, g.c]));
}

#[test]
fn shift_adds_to_the_selection() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    g.across_two(shift());
    assert_eq!(g.selected(), g.ids(&[g.a, g.b, g.c]));
}

#[test]
fn ctrl_toggles_the_clips_it_touches() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0, g.b.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    g.across_two(ctrl());
    assert_eq!(g.selected(), g.ids(&[g.a, g.c]), "B left, C joined, A stayed");
}

#[test]
fn a_plain_marquee_replaces_the_selection_and_an_empty_one_clears_it() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    g.across_two(NONE);
    assert_eq!(g.selected(), g.ids(&[g.b, g.c]), "A was replaced");
    // Beyond every clip: nothing touched, nothing selected.
    let (from, to) = ((g.x(20.0), g.y(g.r.t)), (g.x(24.0), g.y(g.r.t2)));
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, from, to, NONE);
    release_at(&mut g.r.app, &g.r.ctx, &mut g.r.time, to, NONE);
    assert!(g.selected().is_empty());
}

#[test]
fn a_click_without_a_drag_sets_the_cursor_and_selects_nothing() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    let at = (g.x(14.0), g.y(g.r.t));
    click(&mut g.r.app, at);
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    assert!(g.selected().is_empty());
    assert!(g.r.app.arrange.marquee.is_none());
    assert!(g.r.app.cursor() > 0, "the click set the cursor");
    assert_eq!(g.r.app.cursor_track(), Some(g.r.t));
}

#[test]
fn a_one_point_jitter_draws_no_marquee_and_selects_nothing() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    let from = (g.x(14.0), g.y(g.r.t));
    // Over a clip's lane but not on a clip; one point to the side.
    let to = (from.0 + 1.0, from.1);
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, from, to, NONE);
    assert!(g.r.app.arrange.marquee.as_ref().is_none_or(|m| !m.live), "not live under the threshold");
    release_at(&mut g.r.app, &g.r.ctx, &mut g.r.time, to, NONE);
    assert!(g.r.app.arrange.marquee.is_none());
    assert!(g.selected().is_empty(), "a click on empty space deselects, as before");
    assert!(g.r.app.cursor() > 0, "and sets the cursor");
}

#[test]
fn escape_cancels_the_marquee_and_keeps_the_selection() {
    let mut g = Rig::new();
    g.r.app.run("edit.select", json!({"clips": [g.a.0]})).unwrap();
    frames(&mut g.r.app, &g.r.ctx, 3, &mut g.r.time);
    let (from, to) = ((g.x(10.0), g.y(g.r.t2)), (g.x(6.0), g.y(g.r.t)));
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, from, to, NONE);
    assert!(g.r.app.arrange.marquee.is_some());
    let (req, _rx) = crate::ControlRequest::new("ui.key", json!({"key": "escape"}));
    let _ = crate::control::handle_for_test(&mut g.r.app, &egui::Context::default(), &req);
    play_queue(&mut g.r.app, &g.r.ctx, &mut g.r.time);
    assert!(g.r.app.arrange.marquee.is_none(), "cancelled");
    release_at(&mut g.r.app, &g.r.ctx, &mut g.r.time, to, NONE);
    assert_eq!(g.selected(), g.ids(&[g.a]), "the selection is as it was, not cleared and not replaced");
}

#[test]
fn dragging_to_the_edge_scrolls_time_and_the_marquee_follows() {
    let mut g = Rig::new();
    let tl = g.r.app.arrange.layout.timeline;
    let from = (g.x(14.0), g.y(g.r.t));
    let to = (tl[2] - 4.0, from.1);
    let scroll0 = g.r.app.engine.session().edit.zoom.scroll;
    hold_drag(&mut g.r.app, &g.r.ctx, &mut g.r.time, from, to, NONE);
    assert!(g.r.app.engine.session().edit.zoom.scroll > scroll0, "the view scrolled toward the pointer");
    let m = g.r.app.arrange.marquee.clone().unwrap();
    assert!(m.rect[0] < from.0 - 1.0, "the anchor moved left with the content: {:?} vs {}", m.rect, from.0);
    assert!((m.rect[2] - to.0).abs() < 1.0, "the far corner stays at the pointer");
}

#[test]
fn modifiers_pick_how_a_marquee_combines_with_the_selection() {
    assert_eq!(mode(NONE), Mode::Replace);
    assert_eq!(mode(shift()), Mode::Add);
    assert_eq!(mode(ctrl()), Mode::Toggle);
    let (a, b, c) = (ClipId(1), ClipId(2), ClipId(3));
    assert_eq!(combine(Mode::Replace, &[a], &[b, c]), vec![b, c]);
    assert_eq!(combine(Mode::Add, &[a, b], &[b, c]), vec![a, b, c]);
    assert_eq!(combine(Mode::Toggle, &[a, b], &[b, c]), vec![a, c]);
}

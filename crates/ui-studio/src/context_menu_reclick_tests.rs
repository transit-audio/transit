//! A right-click on another target while a context menu is open closes that menu and opens the
//! new target's menu in the same click (#769), as Ableton and Logic both do. Each test opens one
//! menu, right-clicks the other target without closing the first, and checks on every following
//! frame that the menu open is the new target's, anchored at the second click.
use crate::context_menu_tests::{T, rect};
use crate::menu_padding_tests::{popup_at, texts};
use egui::Pos2;
use serde_json::json;
use transit_model::{ClipId, TrackId};

/// The empty lane's first track row; the clip menu has none of them (#728).
const ADD: &str = "Add MIDI Track";

fn mid(r: [f32; 4]) -> Pos2 {
    rect(r).center()
}

/// An instrument track with two clips near the start of the timeline. A menu opens right of and
/// below its click (left of it near the screen's right edge), so each test right-clicks its
/// second target left of or above the first.
fn rig() -> (T, TrackId, [ClipId; 2]) {
    let mut t = T::new();
    let keys = t.track("instrument", "Keys");
    t.app.run("midi.new_clip", json!({"track": keys.0, "start": 10_000, "length": 40_000})).unwrap();
    t.app.run("midi.new_clip", json!({"track": keys.0, "start": 100_000, "length": 40_000})).unwrap();
    t.frames(3);
    let clips = t.app.engine.session().track(keys).unwrap().clips().iter().map(|c| c.id).collect::<Vec<_>>();
    (t, keys, [clips[0], clips[1]])
}

fn clip_at(t: &T, clip: ClipId) -> [f32; 4] {
    t.app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect
}

/// A spot on `track`'s empty lane, `from_right` px in from the timeline's right edge.
fn lane(t: &T, track: TrackId, from_right: f32) -> [f32; 4] {
    let (tl, h) = (t.app.arrange.layout.timeline, t.header(track));
    [tl[2] - from_right - 10.0, h[1] + 4.0, tl[2] - from_right + 10.0, h[3] - 4.0]
}

fn header_spot(t: &T, track: TrackId) -> [f32; 4] {
    let h = t.header(track);
    [h[0] + 50.0, h[1], h[0] + 70.0, h[3]]
}

/// What the open menu is: a recorded row of it, and where its popup frame starts.
fn open_row(t: &T, rows: &[crate::mixer::MenuItem], label: &str) -> Option<Pos2> {
    let row = rows.iter().find(|m| m.label == label)?;
    Some(popup_at(&t.ctx, mid(row.rect)).min)
}

/// The arrangement menu shows `label` (a row it records), anchored at `at`.
fn arrange_menu_at(t: &T, label: &str, at: Pos2) -> Result<(), String> {
    match open_row(t, &t.app.arrange.menu, label) {
        None => Err(format!("no open menu with {label:?}: rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>())),
        Some(min) if min.distance(at) > 4.0 => Err(format!("the menu with {label:?} opens at {min:?}, not at the click {at:?}")),
        Some(_) => Ok(()),
    }
}

fn lane_menu(t: &T, at: Pos2) -> Result<(), String> {
    arrange_menu_at(t, "Bounce in Place", at)?;
    if texts(&t.shapes, ADD).is_empty() { Err("the open menu has no track rows: it is not the lane's".into()) } else { Ok(()) }
}

fn clip_menu(t: &T, clip: ClipId, at: Pos2) -> Result<(), String> {
    arrange_menu_at(t, "Bounce in Place", at)?;
    if !texts(&t.shapes, ADD).is_empty() {
        return Err("the open menu has the track rows: it is the lane's".into());
    }
    match t.app.arrange.menu_target {
        Some((c, _)) if c == clip => Ok(()),
        other => Err(format!("the menu's target is {other:?}, not {clip:?}")),
    }
}

fn header_menu(t: &T, at: Pos2) -> Result<(), String> {
    arrange_menu_at(t, "Freeze", at)
}

/// Opens the first menu with a right-click on `first`, checks it, right-clicks `second` with the
/// first menu still open, and checks `want` on the click's frames and on ten frames after.
fn reclick(
    t: &mut T,
    first: [f32; 4],
    opened: impl Fn(&T, Pos2) -> Result<(), String>,
    second: [f32; 4],
    want: impl Fn(&T, Pos2) -> Result<(), String>,
) {
    t.click(first, "secondary");
    opened(t, mid(first)).unwrap_or_else(|e| panic!("the first menu: {e}"));
    // The second target is outside the open menu (a right-click on the menu is not on the target).
    let over = t.ctx.layer_id_at(mid(second)).map(|l| l.order);
    assert_ne!(over, Some(egui::Order::Foreground), "the second spot {second:?} lies under the open menu");
    t.click(second, "secondary");
    for frame in 0..10 {
        want(t, mid(second)).unwrap_or_else(|e| panic!("{frame} frames after the second right-click: {e}"));
        t.frames(1);
    }
}

#[test]
fn a_right_click_on_the_empty_lane_with_the_clip_menu_open_opens_the_lane_menu_in_one_click() {
    let (mut t, keys, [a, _]) = rig();
    let (c, l) = (clip_at(&t, a), lane(&t, keys, 400.0));
    reclick(&mut t, c, |t, at| clip_menu(t, a, at), l, lane_menu);
}

#[test]
fn a_right_click_on_another_clip_with_a_clip_menu_open_opens_that_clips_menu_in_one_click() {
    let (mut t, _, [a, b]) = rig();
    let (ca, cb) = (clip_at(&t, a), clip_at(&t, b));
    reclick(&mut t, cb, |t, at| clip_menu(t, b, at), ca, |t, at| clip_menu(t, a, at));
}

#[test]
fn a_right_click_on_a_clip_with_the_lane_menu_open_opens_the_clip_menu_in_one_click() {
    let (mut t, keys, [a, _]) = rig();
    let (l, c) = (lane(&t, keys, 400.0), clip_at(&t, a));
    reclick(&mut t, l, lane_menu, c, |t, at| clip_menu(t, a, at));
}

#[test]
fn a_right_click_elsewhere_on_the_empty_lane_with_the_lane_menu_open_reopens_it_there_in_one_click() {
    let (mut t, keys, _) = rig();
    let (l1, l2) = (lane(&t, keys, 400.0), lane(&t, keys, 800.0));
    reclick(&mut t, l1, lane_menu, l2, lane_menu);
}

#[test]
fn a_right_click_on_a_track_header_with_the_clip_menu_open_opens_the_track_menu_in_one_click() {
    let (mut t, keys, [a, _]) = rig();
    let (c, h) = (clip_at(&t, a), header_spot(&t, keys));
    reclick(&mut t, c, |t, at| clip_menu(t, a, at), h, header_menu);
}

#[test]
fn a_right_click_on_the_empty_lane_with_a_track_menu_open_opens_the_lane_menu_in_one_click() {
    let (mut t, keys, _) = rig();
    let (h, l) = (header_spot(&t, keys), lane(&t, keys, 400.0));
    reclick(&mut t, h, header_menu, l, lane_menu);
}

#[test]
fn a_right_click_on_another_track_header_with_a_track_menu_open_opens_its_menu_in_one_click() {
    let (mut t, keys, _) = rig();
    let bass = t.track("audio", "Bass");
    let (h1, h2) = (header_spot(&t, keys), header_spot(&t, bass));
    reclick(&mut t, h2, header_menu, h1, header_menu);
}

#[test]
fn a_right_click_on_the_loop_strip_with_the_lane_menu_open_opens_the_loop_menu_in_one_click() {
    let (mut t, keys, _) = rig();
    let l = lane(&t, keys, 400.0);
    let s = t.app.arrange.layout.loop_strip;
    let strip = [s[0] + 200.0, s[1], s[0] + 220.0, s[3]];
    reclick(&mut t, l, lane_menu, strip, |t, at| arrange_menu_at(t, "Clear loop", at));
}

#[test]
fn a_right_click_on_another_mixer_strip_with_a_strip_menu_open_opens_its_menu_in_one_click() {
    let (mut t, keys, _) = rig();
    let bass = t.track("audio", "Bass");
    t.app.ui.show_mixer = true;
    t.frames(3);
    let name = |t: &T, id: TrackId| t.app.mixer.layout.strips.iter().find(|s| s.track == id.0).unwrap().name_rect;
    let (n1, n2) = (name(&t, keys), name(&t, bass));
    let strip_menu = |t: &T, at: Pos2| match open_row(t, &t.app.mixer.layout.menu, "Make Inactive") {
        None => Err("no open strip menu".to_string()),
        Some(min) if min.distance(at) > 4.0 => Err(format!("the strip menu opens at {min:?}, not at the click {at:?}")),
        Some(_) => Ok(()),
    };
    reclick(&mut t, n2, strip_menu, n1, strip_menu);
}

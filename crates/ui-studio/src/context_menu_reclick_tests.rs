//! A right-click on another target while a context menu is open closes that menu and opens the
//! new target's menu in the same click (#769), as Ableton and Logic both do. Each test opens one
//! menu, right-clicks the other target without closing the first, and checks on every following
//! frame that the menu open is the new target's, anchored at the second click.
use crate::context_menu_tests::{T, rect};
use crate::menu_padding_tests::{popup_at, texts};
use egui::{Pos2, Rect};
use serde_json::json;
use transit_model::{ClipId, TrackId};

/// The empty lane's first track row; the clip menu has none of them (#728).
const ADD: &str = "Add MIDI Track";

fn mid(r: [f32; 4]) -> Pos2 {
    rect(r).center()
}

/// An instrument track with two clips near the start of the timeline. A menu opens right of and
/// below its click (left of it near the screen's right edge), so each test right-clicks its
/// second target where the first menu does not cover it.
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

/// The open menu's popup frame, found by a recorded row of it.
fn open_row(t: &T, rows: &[crate::mixer::MenuItem], label: &str) -> Option<Rect> {
    let row = rows.iter().find(|m| m.label == label)?;
    Some(popup_at(&t.ctx, mid(row.rect)))
}

/// The frame's corner nearest `at`: a menu opens with a corner at its click (right and below it,
/// flipped left or up where the screen ends).
fn corner(frame: Rect, at: Pos2) -> Pos2 {
    let x = if (frame.min.x - at.x).abs() <= (frame.max.x - at.x).abs() { frame.min.x } else { frame.max.x };
    let y = if (frame.min.y - at.y).abs() <= (frame.max.y - at.y).abs() { frame.min.y } else { frame.max.y };
    egui::pos2(x, y)
}

/// The arrangement menu shows `label` (a row it records), anchored at `at`.
fn arrange_menu_at(t: &T, label: &str, at: Pos2) -> Result<(), String> {
    match open_row(t, &t.app.arrange.menu, label) {
        None => Err(format!("no open menu with {label:?}: rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>())),
        Some(f) if corner(f, at).distance(at) > 4.0 => Err(format!("the menu with {label:?} opens at {f:?}, not at the click {at:?}")),
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
        Some(f) if corner(f, at).distance(at) > 4.0 => Err(format!("the strip menu opens at {f:?}, not at the click {at:?}")),
        Some(_) => Ok(()),
    };
    reclick(&mut t, n2, strip_menu, n1, strip_menu);
}

/// The menu drawing `text` (a row or label of it) is open and anchored at `at`.
fn text_menu_at(t: &T, text: &str, at: Pos2) -> Result<(), String> {
    let in_popup = texts(&t.shapes, text).into_iter().find(|r| t.ctx.layer_id_at(r.center()).is_some_and(|l| l.order == egui::Order::Foreground));
    match in_popup.map(|r| popup_at(&t.ctx, r.center())) {
        None => Err(format!("no open menu draws {text:?}")),
        Some(f) if corner(f, at).distance(at) > 4.0 => Err(format!("the menu with {text:?} opens at {f:?}, not at the click {at:?}")),
        Some(_) => Ok(()),
    }
}

/// A 4 px spot on `r`'s vertical middle, `at` of the way from its left (0) to its right (1) edge.
fn spot(r: [f32; 4], at: f32) -> [f32; 4] {
    let x = r[0] + (r[2] - r[0]) * at;
    let y = (r[1] + r[3]) / 2.0;
    [x - 2.0, y - 2.0, x + 2.0, y + 2.0]
}

#[test]
fn a_right_click_elsewhere_on_the_same_track_header_reopens_its_menu_there_in_one_click() {
    let (mut t, keys, _) = rig();
    let h = t.header(keys);
    let (right, left) = ([h[0] + 90.0, h[1] + 4.0, h[0] + 94.0, h[1] + 8.0], [h[0] + 20.0, h[1] + 4.0, h[0] + 24.0, h[1] + 8.0]);
    reclick(&mut t, right, header_menu, left, header_menu);
}

#[test]
fn a_right_click_elsewhere_on_the_loop_strip_reopens_its_menu_there_in_one_click() {
    let (mut t, _, _) = rig();
    let s = t.app.arrange.layout.loop_strip;
    let (a, b) = ([s[0] + 500.0, s[1], s[0] + 520.0, s[3]], [s[0] + 200.0, s[1], s[0] + 220.0, s[3]]);
    let loop_menu = |t: &T, at: Pos2| arrange_menu_at(t, "Clear loop", at);
    reclick(&mut t, a, loop_menu, b, loop_menu);
}

#[test]
fn a_right_click_elsewhere_on_the_same_mixer_strip_reopens_its_menu_there_in_one_click() {
    let (mut t, keys, _) = rig();
    t.app.ui.show_mixer = true;
    t.frames(3);
    let n = t.app.mixer.layout.strips.iter().find(|s| s.track == keys.0).unwrap().name_rect;
    let strip_menu = |t: &T, at: Pos2| match open_row(t, &t.app.mixer.layout.menu, "Make Inactive") {
        None => Err("no open strip menu".to_string()),
        Some(f) if corner(f, at).distance(at) > 4.0 => Err(format!("the strip menu opens at {f:?}, not at the click {at:?}")),
        Some(_) => Ok(()),
    };
    reclick(&mut t, spot(n, 0.7), strip_menu, spot(n, 0.1), strip_menu);
}

#[test]
fn a_right_click_higher_on_the_same_fader_reopens_its_midi_learn_menu_there_in_one_click() {
    let (mut t, keys, _) = rig();
    t.app.ui.show_mixer = true;
    t.frames(3);
    let f = t.app.mixer.layout.strips.iter().find(|s| s.track == keys.0).unwrap().fader;
    let x = (f[0] + f[2]) / 2.0;
    let (low, high) = ([x - 2.0, f[3] - 30.0, x + 2.0, f[3] - 26.0], [x - 2.0, f[1] + 10.0, x + 2.0, f[1] + 14.0]);
    let learn = |t: &T, at: Pos2| match t.app.midi_learn.menu.first() {
        None => Err("no open MIDI Learn menu".to_string()),
        Some(row) => match popup_at(&t.ctx, mid(row.rect)) {
            f if corner(f, at).distance(at) > 4.0 => Err(format!("the MIDI Learn menu opens at {f:?}, not at the click {at:?}")),
            _ => Ok(()),
        },
    };
    reclick(&mut t, low, learn, high, learn);
}

#[test]
fn a_right_click_elsewhere_on_the_piano_rolls_step_button_reopens_its_menu_there_in_one_click() {
    let (mut t, _, [a, _]) = rig();
    let c = clip_at(&t, a);
    t.input("ui.click", json!({"x": mid(c).x, "y": mid(c).y, "count": 2}));
    t.frames(10);
    assert!(t.app.keys.layout.visible, "the double-click opened the piano roll");
    let b = t.app.keys.layout.step;
    let step_menu = |t: &T, at: Pos2| text_menu_at(t, "Step input settings", at);
    // The button is at the screen's right edge: its menu opens left of the click.
    reclick(&mut t, spot(b, 0.2), step_menu, spot(b, 0.8), step_menu);
}

/// The lane menu reopened by the second right-click, for the close checks below.
fn reopened_lane_menu() -> (T, TrackId, [f32; 4]) {
    let (mut t, keys, [a, _]) = rig();
    let (c, l) = (clip_at(&t, a), lane(&t, keys, 400.0));
    reclick(&mut t, c, |t, at| clip_menu(t, a, at), l, lane_menu);
    (t, keys, l)
}

fn nothing_open(t: &T) -> bool {
    !egui::Popup::is_any_open(&t.ctx) && t.app.arrange.menu.is_empty()
}

/// The lane menu's "Add MIDI Track" row as drawn in the menu anchored at `at`.
fn add_row(t: &T, at: Pos2) -> [f32; 4] {
    let r = texts(&t.shapes, ADD).into_iter().find(|r| corner(popup_at(&t.ctx, r.center()), at).distance(at) <= 4.0).expect("the row");
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

#[test]
fn after_the_reopen_a_click_outside_closes_the_menu() {
    let (mut t, keys, _) = reopened_lane_menu();
    t.click(header_spot(&t, keys), "primary");
    assert!(nothing_open(&t), "rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>());
}

#[test]
fn after_the_reopen_escape_closes_the_menu() {
    let (mut t, _, _) = reopened_lane_menu();
    t.input("ui.key", json!({"key": "Escape"}));
    assert!(nothing_open(&t));
}

#[test]
fn after_the_reopen_a_row_click_runs_the_row_and_closes_the_menu() {
    let (mut t, _, l) = reopened_lane_menu();
    let row = add_row(&t, mid(l));
    let before = t.app.engine.session().tracks.len();
    t.click(row, "primary");
    assert_eq!(t.app.engine.session().tracks.len(), before + 1, "{}", t.app.status);
    assert!(nothing_open(&t));
}

#[test]
fn after_the_reopen_a_primary_click_elsewhere_on_the_lane_closes_the_menu_and_opens_nothing() {
    let (mut t, keys, _) = reopened_lane_menu();
    t.click(lane(&t, keys, 800.0), "primary");
    assert!(nothing_open(&t));
    t.frames(5);
    assert!(nothing_open(&t), "nothing opens on the frames after");
}

/// A right-click on the open menu itself is a click inside the menu, and the chrome's menus close
/// on any click (egui's default `CloseOnClick`): the menu closes, the row under the pointer does not
/// run (rows act on the primary button) and nothing else opens. The same before and after #769: the
/// widget under the menu gets no secondary click, so the seam does not apply.
#[test]
fn after_the_reopen_a_right_click_on_a_row_closes_the_menu_and_runs_nothing() {
    let (mut t, _, l) = reopened_lane_menu();
    let row = add_row(&t, mid(l));
    let tracks = t.app.engine.session().tracks.len();
    t.click(row, "secondary");
    assert!(nothing_open(&t), "rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>());
    assert_eq!(t.app.engine.session().tracks.len(), tracks, "the row did not run");
}

#[test]
fn after_the_reopen_a_right_click_on_the_menus_frame_closes_the_menu_and_opens_nothing() {
    let (mut t, _, l) = reopened_lane_menu();
    let frame = popup_at(&t.ctx, mid(add_row(&t, mid(l))));
    // The frame's padding above the first row, inside the menu's layer.
    let edge = [frame.min.x + 2.0, frame.min.y + 1.0, frame.min.x + 4.0, frame.min.y + 3.0];
    assert_eq!(t.ctx.layer_id_at(mid(edge)).map(|l| l.order), Some(egui::Order::Foreground), "the spot is on the menu");
    t.click(edge, "secondary");
    assert!(nothing_open(&t), "rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>());
}

/// The same right-click on a row of a menu opened by one right-click (no reopen involved): the
/// behavior the two tests above keep.
#[test]
fn a_right_click_on_a_row_of_a_menu_opened_once_closes_it_and_runs_nothing() {
    let (mut t, keys, _) = rig();
    let l = lane(&t, keys, 400.0);
    t.click(l, "secondary");
    lane_menu(&t, mid(l)).unwrap_or_else(|e| panic!("the menu: {e}"));
    let tracks = t.app.engine.session().tracks.len();
    t.click(add_row(&t, mid(l)), "secondary");
    assert!(nothing_open(&t), "rows {:?}", t.app.arrange.menu.iter().map(|m| &m.label).collect::<Vec<_>>());
    assert_eq!(t.app.engine.session().tracks.len(), tracks, "the row did not run");
}

type Seen = Vec<(bool, bool)>;

/// One plain widget with a context menu, right-clicked at (50, 50), then at (500, 50) with the
/// menu open: (drawn, open after the frame) on each frame.
fn plain_reclick(chrome: bool) -> Seen {
    let ctx = egui::Context::default();
    let mut seen = Seen::new();
    let right = |p: Pos2| {
        let press =
            |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
        vec![egui::Event::PointerMoved(p), press(true), press(false)]
    };
    let script = [vec![], right(egui::pos2(50.0, 50.0)), vec![], vec![], right(egui::pos2(500.0, 50.0)), vec![], vec![]];
    for (i, events) in script.into_iter().enumerate() {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            time: Some(i as f64 / 60.0),
            events,
            ..Default::default()
        };
        let mut o = ctx.run_ui(raw, |ui| {
            let r = ui.allocate_response(egui::vec2(700.0, 500.0), egui::Sense::click());
            let drawn = if chrome {
                crate::chrome::context_menu(&r, false).show(|ui| ui.label("row")).is_some()
            } else {
                egui::Popup::context_menu(&r).show(|ui| ui.label("row")).is_some()
            };
            seen.push((drawn, egui::Popup::is_any_open(ui.ctx())));
        });
        o.textures_delta.clear();
    }
    seen
}

/// The upstream defect that the seam in `chrome::ContextMenu::show` works around (egui 0.36.2):
/// egui's own context menu, right-clicked again on its widget while open, is drawn at the new
/// pointer and closed in the same frame. When this fails, egui has fixed it: delete the seam and
/// this test.
#[test]
fn egui_still_closes_a_reopened_context_menu() {
    let seen = plain_reclick(false);
    assert_eq!(seen[1], (true, true), "the first right-click opens the menu: {seen:?}");
    assert_eq!(
        seen[4],
        (true, false),
        "egui no longer closes a context menu reopened on its own widget: remove the #769 seam in chrome::ContextMenu::show and this test ({seen:?})"
    );
}

#[test]
fn the_chrome_context_menu_on_a_plain_widget_stays_open_after_the_reopening_right_click() {
    let seen = plain_reclick(true);
    assert_eq!(seen, vec![(false, false), (true, true), (true, true), (true, true), (true, true), (true, true), (true, true)]);
}

//! The Sampler panel's context menus follow #769: a right-click on another target, or elsewhere on
//! the same one, while a menu is open replaces the menu in that one click.
use super::test_rig::{T, mid};
use crate::menu_padding_tests::popup_at;
use egui::Pos2;
use serde_json::json;
use transit_model::TrackId;

fn pos(r: [f32; 4]) -> Pos2 {
    let (x, y) = mid(r);
    egui::pos2(x, y)
}

/// The open menu holds a row at `row` and is anchored at `at`.
fn anchored(t: &T, row: Option<[f32; 4]>, at: Pos2) -> Result<(), String> {
    let row = row.ok_or("no open menu")?;
    let min = popup_at(&t.ctx, pos(row)).min;
    if min.distance(at) > 4.0 { Err(format!("the menu opens at {min:?}, not at the click {at:?}")) } else { Ok(()) }
}

fn right_click(t: &mut T, at: Pos2) {
    t.send("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}));
}

/// Right-clicks `first`, checks its menu, right-clicks `second` with the menu open, and checks the
/// menu `row` reads on the frames after.
fn reclick(t: &mut T, first: Pos2, second: Pos2, row: impl Fn(&T) -> Option<[f32; 4]>) {
    right_click(t, first);
    anchored(t, row(t), first).unwrap_or_else(|e| panic!("the first menu: {e}"));
    let over = t.ctx.layer_id_at(second).map(|l| l.order);
    assert_ne!(over, Some(egui::Order::Foreground), "the second spot {second:?} lies under the open menu");
    right_click(t, second);
    for frame in 0..10 {
        anchored(t, row(t), second).unwrap_or_else(|e| panic!("{frame} frames after the second right-click: {e}"));
        t.frames(1);
    }
}

fn reset_row(t: &T, tr: TrackId) -> Option<[f32; 4]> {
    t.panel(tr).bar.start.reset_row
}

fn pad_row(t: &T, tr: TrackId) -> Option<[f32; 4]> {
    t.panel(tr).pad_menu.into_iter().find(|r| r.0 == "Clear pad").map(|r| r.1)
}

#[test]
fn a_right_click_elsewhere_on_the_start_note_field_reopens_its_menu_there_in_one_click() {
    let (mut t, tr) = T::kit();
    let f = t.panel(tr).bar.start.field;
    let y = (f[1] + f[3]) / 2.0;
    let (right, left) = (egui::pos2(f[2] - 4.0, y), egui::pos2(f[0] + 4.0, y));
    reclick(&mut t, right, left, |t| reset_row(t, tr));
}

#[test]
fn a_right_click_on_another_pad_with_a_pad_menu_open_opens_its_menu_in_one_click() {
    let (mut t, tr) = T::kit();
    let (b, a) = (t.pad_at(tr, 2), t.pad_at(tr, 1));
    reclick(&mut t, egui::pos2(b.0, b.1), egui::pos2(a.0, a.1), |t| pad_row(t, tr));
    assert_eq!(t.panel(tr).selected, 1, "the second pad is the menu's pad");
}

#[test]
fn a_right_click_elsewhere_on_the_same_pad_reopens_its_menu_there_in_one_click() {
    let (mut t, tr) = T::kit();
    let r = t.panel(tr).pad(1).unwrap().pad.rect;
    let y = (r[1] + r[3]) / 2.0;
    let (right, left) = (egui::pos2(r[2] - 6.0, y), egui::pos2(r[0] + 6.0, y));
    reclick(&mut t, right, left, |t| pad_row(t, tr));
}

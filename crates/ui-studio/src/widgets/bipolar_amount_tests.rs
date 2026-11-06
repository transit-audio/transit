//! The bipolar amount bar: the fill grows from the center tick toward the value on either side,
//! in its tokens, and a sideways drag changes the amount (Shift finer).

use super::bipolar_amount;
use super::test_support::{Rig, mid, size};
use crate::theme::DARK;
use egui::{Modifiers, vec2};

#[test]
fn the_fill_grows_from_the_center_toward_the_value() {
    let mut rig = Rig::new();
    let pos = rig.idle(|ui| bipolar_amount::show(ui, egui::Id::new("a"), 120.0, 0.5)).layout;
    let neg = rig.idle(|ui| bipolar_amount::show(ui, egui::Id::new("a"), 120.0, -0.25)).layout;
    let zero = rig.idle(|ui| bipolar_amount::show(ui, egui::Id::new("a"), 120.0, 0.0)).layout;
    let cx = mid(pos.tick).x;
    assert_eq!((pos.fill[0], pos.fill[2]), (cx, cx + 30.0), "+0.5 fills half the right side");
    assert_eq!((neg.fill[0], neg.fill[2]), (cx - 15.0, cx), "-0.25 fills a quarter of the left side");
    assert_eq!(zero.fill[2] - zero.fill[0], 0.0, "nothing at zero");
    assert_eq!((size(pos.track).1, size(pos.fill).1), (4.0, 7.0), "4 px track, 7 px fill");
    assert_eq!((pos.track_color, pos.fill_color, pos.tick_color), (DARK.line, DARK.modulation, DARK.text_faint));
}

#[test]
fn dragging_sideways_changes_the_amount_and_shift_is_finer() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| bipolar_amount::show(ui, egui::Id::new("a"), 120.0, 0.0)).layout;
    let run = |rig: &mut Rig| {
        let mut a = 0.0f32;
        rig.drag(mid(l.rect), vec2(30.0, 0.0), 3, |ui| {
            if let Some(n) = bipolar_amount::show(ui, egui::Id::new("a"), 120.0, a).amount {
                a = n;
            }
        });
        a
    };
    assert!((run(&mut rig) - 0.5).abs() < 1e-4);
    rig.modifiers = Modifiers::SHIFT;
    assert!((run(&mut rig) - 0.05).abs() < 1e-4);
    assert_eq!(bipolar_amount::dragged(0.9, 60.0, 120.0, false), 1.0, "clamped");
}

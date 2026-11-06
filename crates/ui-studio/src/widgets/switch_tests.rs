//! The switch: 34 by 20 with the knob at the end it points to, the tokens on and off in both
//! themes, the mono readout, and a click.

use super::switch;
use super::test_support::{Rig, inside, mid, size};
use crate::theme::{DARK, LIGHT};

#[test]
fn on_and_off_place_the_knob_and_paint_their_tokens_in_both_themes() {
    for (theme, t) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        let mut rig = Rig::themed(theme);
        let on = rig.idle(|ui| switch::show(ui, egui::Id::new("s"), true)).layout;
        let off = rig.idle(|ui| switch::show(ui, egui::Id::new("s"), false)).layout;
        for l in [&on, &off] {
            assert_eq!(size(l.track), (34.0, 20.0), "{theme:?}");
            assert!(inside(l.readout, l.rect) && inside(l.track, l.rect));
            assert!(l.readout[2] <= l.track[0], "the readout sits left of the track");
        }
        assert_eq!((on.track_color, on.knob_color, on.readout_text), (t.signal, t.switch_on_knob, "ON"), "{theme:?}");
        assert_eq!((off.track_color, off.knob_color, off.readout_text), (t.line_strong, t.text_muted, "OFF"), "{theme:?}");
        let mid_x = (on.track[0] + on.track[2]) / 2.0;
        assert!(on.knob_center[0] > mid_x && off.knob_center[0] < mid_x, "on right, off left");
        assert_eq!(on.knob_center[0] + 7.0 + 3.0, on.track[2], "3 px inset");
    }
}

#[test]
fn a_click_toggles() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| switch::show(ui, egui::Id::new("s"), false)).layout;
    assert!(rig.click(mid(l.track), |ui| switch::show(ui, egui::Id::new("s"), false)).toggled);
}

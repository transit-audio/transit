//! Toggles: the role table of the design README in both themes, the 24 px height, clicks, and an
//! unavailable toggle that keeps its place but does not fire.

use super::test_support::{Rig, mid};
use super::toggle::{self, Role};
use crate::theme::{DARK, LIGHT, Tokens};

#[test]
fn the_role_table_holds_in_both_themes() {
    for (theme, t) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        crate::theme::set_current(theme);
        let t: &Tokens = t;
        for role in [Role::Mute, Role::Solo, Role::Arm, Role::Monitor, Role::Option] {
            assert_eq!(toggle::colors(role, false, true), (t.surface_raised, t.text_muted, t.line), "{theme:?} {role:?} off");
            assert_eq!(toggle::colors(role, true, false), (t.toggle_unavailable_fill, t.ink_arrow, t.line), "{theme:?} {role:?} unavailable");
        }
        assert_eq!(toggle::colors(Role::Mute, true, true), (t.text, t.bg_window, t.text), "mute inverts");
        assert_eq!(toggle::colors(Role::Solo, true, true), (t.signal, t.surface_inset, t.signal));
        assert_eq!(toggle::colors(Role::Arm, true, true), (t.meter_hot, t.bg_window, t.meter_hot));
        assert_eq!(toggle::colors(Role::Monitor, true, true), (t.toggle_option_fill, t.signal, t.signal));
        assert_eq!(toggle::colors(Role::Option, true, true), toggle::colors(Role::Monitor, true, true));
        // Every on state carries its letter at 4.5:1.
        for role in [Role::Mute, Role::Solo, Role::Arm, Role::Monitor] {
            let (fill, ink, _) = toggle::colors(role, true, true);
            assert!(crate::theme::contrast(fill, ink) >= 4.5, "{theme:?} {role:?}: {}", crate::theme::contrast(fill, ink));
        }
    }
    crate::theme::set_current(egui::Theme::Dark);
}

#[test]
fn a_toggle_is_24_high_and_a_click_reports_it() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| toggle::show(ui, egui::Id::new("m"), "M", Role::Mute, false, true, None)).layout;
    assert_eq!(l.rect[3] - l.rect[1], 24.0);
    assert!(l.rect[2] - l.rect[0] >= 24.0, "at least square");
    assert_eq!((l.fill, l.text.as_str()), (DARK.surface_raised, "M"));
    let out = rig.click(mid(l.rect), |ui| toggle::show(ui, egui::Id::new("m"), "M", Role::Mute, false, true, None));
    assert!(out.toggled);
    let wide = rig.idle(|ui| toggle::show(ui, egui::Id::new("o"), "RETRIG", Role::Option, true, true, Some(60.0))).layout;
    assert_eq!((wide.rect[2] - wide.rect[0], wide.fill), (60.0, DARK.toggle_option_fill));
}

#[test]
fn an_unavailable_toggle_keeps_its_place_and_ignores_clicks() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| toggle::show(ui, egui::Id::new("i"), "I", Role::Monitor, true, false, None)).layout;
    assert_eq!(l.rect[3] - l.rect[1], 24.0, "same layout as an available one");
    assert_eq!((l.fill, l.ink), (DARK.toggle_unavailable_fill, DARK.ink_arrow));
    let out = rig.click(mid(l.rect), |ui| toggle::show(ui, egui::Id::new("i"), "I", Role::Monitor, true, false, None));
    assert!(!out.toggled);
}

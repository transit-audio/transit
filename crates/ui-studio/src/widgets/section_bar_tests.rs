//! The section bar: collapse and enable are separate clicks on separate targets, the middle shows
//! the description open and the summary collapsed, the chevron turns, and a disabled section's
//! body dims without the kit storing anything.

use super::section_bar::{self, Bar};
use super::test_support::{Rig, inside, mid};
use crate::theme::DARK;

fn bar(collapsed: bool, enabled: Option<bool>) -> Bar<'static> {
    Bar {
        title: "Shaper",
        tag: Some("TRANSIT EXTENSION"),
        description: Some("Amp envelope, filter and LFOs"),
        summary: Some("ADSR 5/120/0.8/300 · LP 1.2k"),
        collapsed,
        enabled,
    }
}

fn draw(ui: &mut egui::Ui, b: &Bar) -> section_bar::SectionBarOut {
    section_bar::show(ui, egui::Id::new("sec"), 640.0, b)
}

#[test]
fn the_bar_lays_out_left_middle_and_right() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| draw(ui, &bar(false, Some(true)))).layout;
    assert_eq!(l.bar[3] - l.bar[1], section_bar::HEIGHT);
    let sw = l.switch.clone().expect("a switch for an enable parameter");
    let tag = l.tag.unwrap();
    let (text, middle) = l.middle.clone().unwrap();
    assert_eq!(text, "Amp envelope, filter and LFOs", "open: the description");
    assert!(l.chevron[2] <= l.title[0] && l.title[2] <= tag[0] && tag[2] <= middle[0] && middle[2] <= sw.rect[0], "{l:?}");
    assert!((sw.rect[2] - l.bar[2]).abs() < 0.02, "the switch sits at the right edge");
    for r in [l.button, tag, middle, sw.rect] {
        assert!(inside(r, l.bar), "{r:?}");
    }
    assert_eq!((l.chevron_deg, l.title_color, l.tag_border), (0.0, DARK.text, DARK.line_strong));
    let collapsed = rig.idle(|ui| draw(ui, &bar(true, Some(true)))).layout;
    assert_eq!(collapsed.chevron_deg, -90.0, "collapsed: the chevron points right");
    assert_eq!(collapsed.middle.unwrap().0, "ADSR 5/120/0.8/300 · LP 1.2k", "collapsed: the summary");
    let always_on = rig.idle(|ui| draw(ui, &bar(false, None))).layout;
    assert!(always_on.switch.is_none(), "no enable parameter, no switch");
}

#[test]
fn the_title_only_collapses_and_the_switch_only_enables() {
    let mut rig = Rig::new();
    let b = bar(false, Some(true));
    let l = rig.idle(|ui| draw(ui, &b)).layout;
    let out = rig.click(mid(l.title), |ui| draw(ui, &b));
    assert!(out.toggle_collapse && !out.toggle_enable);
    let out = rig.click(mid(l.chevron), |ui| draw(ui, &b));
    assert!(out.toggle_collapse && !out.toggle_enable);
    let out = rig.click(mid(l.switch.unwrap().track), |ui| draw(ui, &b));
    assert!(out.toggle_enable && !out.toggle_collapse);
    let out = rig.click(mid(l.middle.unwrap().1), |ui| draw(ui, &b));
    assert!(!out.toggle_enable && !out.toggle_collapse, "the middle is not a button");
}

#[test]
fn a_disabled_section_dims_its_body_open_or_not() {
    assert_eq!(section_bar::body_opacity(Some(false)), crate::tokens::alpha::DISABLED_SECTION);
    assert_eq!(section_bar::body_opacity(Some(true)), 1.0);
    assert_eq!(section_bar::body_opacity(None), 1.0);
}

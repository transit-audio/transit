//! The hatch (#326): its tokens, its anchoring to the window, its 16 px floor.

use super::hatch::{self, HatchLayout};
use super::test_support::Rig;
use crate::theme::{DARK, LIGHT};
use crate::tokens::hatch as h;
use egui::{Rect, pos2, vec2};

fn paint(rig: &mut Rig, rect: Rect) -> Option<HatchLayout> {
    rig.idle(|ui| hatch::paint(ui.painter(), rect))
}

#[test]
fn the_hatch_is_text_at_7_percent_in_1_5_px_lines_8_px_apart() {
    let mut rig = Rig::new();
    let l = paint(&mut rig, Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 88.0))).expect("hatched");
    assert_eq!(l.color, DARK.text.gamma_multiply(0.07));
    assert_eq!(l.stroke, 1.5);
    for w in l.lines.windows(2) {
        assert!((w[1] - w[0] - h::PITCH * std::f32::consts::SQRT_2).abs() < 1e-3, "8 px across the lines");
    }
    assert!(l.lines.len() > 10);
    // The light set's hatch is text at 6 percent.
    let mut light = Rig::themed(egui::Theme::Light);
    let ll = light.idle(|ui| {
        crate::theme::set_current(egui::Theme::Light);
        hatch::paint(ui.painter(), Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 88.0)))
    });
    assert_eq!(ll.expect("hatched").color, LIGHT.text.gamma_multiply(0.06));
}

#[test]
fn stripes_are_anchored_to_the_window_so_a_header_and_its_lane_line_up() {
    let mut rig = Rig::new();
    let header = Rect::from_min_max(pos2(20.0, 200.0), pos2(260.0, 288.0));
    let lane = Rect::from_min_max(pos2(260.0, 200.0), pos2(900.0, 288.0));
    let (a, b) = (paint(&mut rig, header).unwrap(), paint(&mut rig, lane).unwrap());
    let step = hatch::step();
    for c in a.lines.iter().chain(&b.lines) {
        let k = c / step;
        assert!((k - k.round()).abs() < 1e-3, "{c} is on the window's grid of lines");
    }
    // A line that leaves the header across the seam enters the lane at the same point.
    let crossing: Vec<f32> = a.lines.iter().filter(|c| b.lines.contains(c)).copied().collect();
    assert!(!crossing.is_empty(), "lines cross the seam");
    // Negative control: a rect moved by half a step paints lines at the same window positions,
    // not shifted with the rect.
    let shifted = paint(&mut rig, header.translate(vec2(step / 2.0, 0.0))).unwrap();
    assert!(shifted.lines.iter().all(|c| ((c / step) - (c / step).round()).abs() < 1e-3));
}

#[test]
fn only_slivers_under_16_px_skip_the_hatch() {
    let mut rig = Rig::new();
    assert_eq!(crate::tokens::hatch::MIN_SIZE, 16.0);
    assert!(paint(&mut rig, Rect::from_min_size(pos2(40.0, 40.0), vec2(400.0, 46.0))).is_some(), "a 46 px row");
    assert!(paint(&mut rig, Rect::from_min_size(pos2(40.0, 40.0), vec2(400.0, 24.0))).is_some(), "a 24 px row (ADR 0044's floor)");
    assert!(paint(&mut rig, Rect::from_min_size(pos2(40.0, 40.0), vec2(400.0, 16.0))).is_some(), "16 px is enough");
    assert!(paint(&mut rig, Rect::from_min_size(pos2(40.0, 40.0), vec2(400.0, 15.0))).is_none(), "a sliver");
    // Negative control: the old 48 px floor is gone, so a 46 px row is no longer refused.
    assert_ne!(crate::tokens::hatch::MIN_SIZE, 48.0);
    assert!(46.0 < 48.0 && hatch::applies(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 46.0))));
}

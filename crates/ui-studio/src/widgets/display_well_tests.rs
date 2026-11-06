//! The display well: padding and the readout row, the curve's tokens and alphas, points placed by
//! their normalized position, the mod band, and a point drag that reports a normalized position.

use super::display_well::{self, Well};
use super::test_support::{Rig, inside, mid};
use crate::theme::DARK;
use crate::tokens::alpha;
use egui::{Rect, pos2, vec2};

const CURVE: [[f32; 2]; 3] = [[0.0, 0.2], [0.5, 0.8], [1.0, 0.4]];
const POINTS: [[f32; 2]; 2] = [[0.5, 0.8], [1.0, 0.4]];

fn well(ui: &mut egui::Ui) -> display_well::WellOut {
    display_well::show(
        ui,
        egui::Id::new("w"),
        vec2(240.0, 120.0),
        &Well { readouts: &["1.20 kHz", "Q 0.71"], curve: &CURVE, points: &POINTS, mod_band: Some([0.4, 0.6]) },
    )
}

#[test]
fn padding_readouts_and_plot_follow_the_spec() {
    let mut rig = Rig::new();
    let l = rig.idle(well).layout;
    assert_eq!(l.plot[0] - l.well[0], 8.0, "8 px padding");
    assert_eq!(l.well[2] - l.plot[2], 8.0);
    assert_eq!(l.well[3] - l.plot[3], 8.0);
    assert_eq!(l.plot[1] - l.well[1], 8.0 + display_well::READOUT_H, "readout row above the plot");
    assert_eq!(l.readouts.len(), 2);
    for r in &l.readouts {
        assert!(r[3] <= l.plot[1] + 0.5 && inside(*r, l.well), "{r:?}");
    }
    assert!(l.readouts[0][2] <= l.readouts[1][0], "readouts left to right");
    let no_readouts = display_well::plot_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 60.0)), false);
    assert_eq!(no_readouts.min.y, 8.0, "no readout row without readouts");
}

#[test]
fn curve_points_and_band_use_their_tokens_and_positions() {
    let mut rig = Rig::new();
    let l = rig.idle(well).layout;
    assert_eq!(l.curve_color, DARK.signal);
    assert_eq!(l.fill_color, DARK.signal.gamma_multiply(alpha::CURVE_FILL));
    assert_eq!(l.band_color, DARK.modulation.gamma_multiply(alpha::MOD_BAND));
    let plot = Rect::from_min_max(pos2(l.plot[0], l.plot[1]), pos2(l.plot[2], l.plot[3]));
    for (r, n) in l.points.iter().zip(POINTS) {
        let want = display_well::to_screen(plot, n);
        let c = mid(*r);
        assert!((c.x - want.x).abs() < 0.02 && (c.y - want.y).abs() < 0.02, "{c:?} vs {want:?}");
        assert!((r[2] - r[0] - 2.0 * display_well::POINT_R).abs() < 0.02);
    }
    let band = l.band.unwrap();
    assert!((band[0] - (plot.min.x + 0.4 * plot.width())).abs() < 0.02 && (band[2] - (plot.min.x + 0.6 * plot.width())).abs() < 0.02);
    assert_eq!((band[1], band[3]), (l.plot[1], l.plot[3]), "the band spans the plot's height");
}

#[test]
fn dragging_a_point_reports_its_new_normalized_position() {
    let mut rig = Rig::new();
    let l = rig.idle(well).layout;
    let from = mid(l.points[0]);
    let plot_h = l.plot[3] - l.plot[1];
    let outs = rig.drag(from, vec2(0.0, plot_h * 0.25), 2, well);
    let got = outs.iter().filter_map(|o| o.dragged).last().expect("a drag reported");
    assert_eq!(got.0, 0);
    assert!((got.1[0] - 0.5).abs() < 0.01 && (got.1[1] - 0.55).abs() < 0.01, "{got:?}");
    let plot = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
    assert_eq!(display_well::to_norm(plot, pos2(-20.0, 140.0)), [0.0, 0.0], "clamped to the plot");
}

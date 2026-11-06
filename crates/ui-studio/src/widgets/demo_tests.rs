//! The kit's demo layout (hidden: a test, not a window): one of every control laid out as a device
//! section would lay them out, in both themes. Nothing overlaps, everything stays on screen, and
//! each control's cell is where the layout put it. The Macro Oscillator migration builds on this.

use super::test_support::Rig;
use super::{bipolar_amount, display_well, fader, knob, level_meter, section_bar, segmented, switch, toggle};
use egui::vec2;

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] - 0.01 && b[0] < a[2] - 0.01 && a[1] < b[3] - 0.01 && b[1] < a[3] - 0.01
}

#[test]
fn one_of_every_control_lays_out_without_overlap_in_both_themes() {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let mut rig = Rig::themed(theme);
        let cells = rig.idle(|ui| {
            let mut cells: Vec<(&str, [f32; 4])> = Vec::new();
            let id = egui::Id::new("demo");
            cells.push((
                "section bar",
                section_bar::show(
                    ui,
                    id.with("bar"),
                    640.0,
                    &section_bar::Bar {
                        title: "Shaper",
                        tag: Some("TRANSIT EXTENSION"),
                        description: Some("Envelope and filter"),
                        summary: None,
                        collapsed: false,
                        enabled: Some(true),
                    },
                )
                .layout
                .bar,
            ));
            cells.push((
                "display well",
                display_well::show(
                    ui,
                    id.with("well"),
                    vec2(320.0, 110.0),
                    &display_well::Well { readouts: &["1.20 kHz"], curve: &[[0.0, 0.3], [1.0, 0.7]], points: &[[1.0, 0.7]], mod_band: None },
                )
                .layout
                .well,
            ));
            ui.horizontal(|ui| {
                for (i, size) in [knob::Size::L, knob::Size::M, knob::Size::S].into_iter().enumerate() {
                    let k = knob::Knob { size, modulation: Some(0.1), ..knob::Knob::new("Cutoff", "1.20 kHz", 0.4) };
                    cells.push(("knob", knob::show(ui, id.with(("knob", i)), &k).layout.rect));
                }
                cells.push(("fader", fader::show(ui, id.with("fader"), fader::Variant::Compact, 120.0, -6.0, false).layout.rect));
                cells.push(("meter", level_meter::show(ui, 10.0, 120.0, &level_meter::Meter::vu(-20.0)).well));
            });
            ui.horizontal(|ui| {
                for (i, (text, role)) in
                    [("M", toggle::Role::Mute), ("S", toggle::Role::Solo), ("R", toggle::Role::Arm), ("I", toggle::Role::Monitor)]
                        .into_iter()
                        .enumerate()
                {
                    cells.push(("toggle", toggle::show(ui, id.with(("t", i)), text, role, i % 2 == 0, true, None).layout.rect));
                }
                cells.push(("switch", switch::show(ui, id.with("sw"), true).layout.rect));
                cells.push(("segmented", segmented::show(ui, id.with("seg"), &["LP", "BP", "HP"], 0, None).layout.track));
                cells.push(("amount", bipolar_amount::show(ui, id.with("amt"), 100.0, -0.4).layout.rect));
            });
            cells
        });
        assert_eq!(cells.len(), 14, "{theme:?}");
        for (name, c) in &cells {
            assert!(c[0] >= 0.0 && c[1] >= 0.0 && c[2] <= 800.0 && c[3] <= 600.0, "{theme:?}: {name} off screen: {c:?}");
            assert!(c[2] > c[0] && c[3] > c[1], "{theme:?}: {name} is empty");
        }
        for (i, (a, ra)) in cells.iter().enumerate() {
            for (b, rb) in &cells[i + 1..] {
                assert!(!overlaps(*ra, *rb), "{theme:?}: {a} {ra:?} overlaps {b} {rb:?}");
            }
        }
    }
}

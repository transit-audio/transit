//! The hero: the model card and readouts, the Timbre/Morph pad and the model picker.

use super::hero::{MODEL_H, NOISE_FROM, PAD, model_step, pad_drag, voice_states, voices_text};
use super::test_rig::{T, info, mid};
use crate::widgets::test_support::size;
use transit_dsp::plugins::macro_osc::MODELS;

#[test]
fn the_card_and_the_scope_readouts_name_the_model_and_the_voices() {
    let (mut t, tr) = T::open();
    let h = t.panel(tr).hero;
    assert_eq!(h.model_name, "Virtual analog");
    assert_eq!(h.readout, "WAVE · Virtual analog");
    assert_eq!(h.voices, "0 voices");
    t.set(tr, "model", 13.0);
    assert_eq!(t.panel(tr).hero.model_name, MODELS[13], "the card follows the model");
    assert_eq!(voice_states(0b0101 | 0b0001 << 16, 4), "H-R-");
    assert_eq!(voices_text("H-R-"), "2 voices");
    assert_eq!(voices_text("-R--"), "1 voice");
    // Negative control: free voices are not counted.
    assert_eq!(voices_text("----"), "0 voices");
}

#[test]
fn dragging_the_pad_sets_timbre_and_morph_in_one_undo_step() {
    let (mut t, tr) = T::open();
    let pad = t.panel(tr).hero.pad;
    assert_eq!(size(pad), (PAD, PAD));
    let before = t.undo_len();
    // To the upper right quarter: Timbre and Morph both rise.
    let to = (pad[0] + PAD * 0.75, pad[1] + PAD * 0.25);
    t.drag(mid(pad), to, false);
    let (timbre, morph) = (t.param(tr, "timbre"), t.param(tr, "morph"));
    assert!((timbre - 75.0).abs() < 1.5 && (morph - 75.0).abs() < 1.5, "{timbre} {morph}");
    assert_eq!(t.undo_len(), before + 1, "one undo step for the drag");
    let h = t.panel(tr).hero;
    assert_eq!(h.timbre_text, info().param("timbre").unwrap().format(timbre));
    assert_eq!(h.morph_text, info().param("morph").unwrap().format(morph));
    assert!((h.pad_point[0] - to.0).abs() < 2.0 && (h.pad_point[1] - to.1).abs() < 2.0, "the point follows");
    // Shift moves a tenth of the pointer's travel (the negative control of the plain drag).
    let from = (h.pad_point[0], h.pad_point[1]);
    t.drag(from, (from.0 - PAD * 0.5, from.1), true);
    let fine = t.param(tr, "timbre");
    assert!((timbre - fine - 5.0).abs() < 1.0, "{timbre} -> {fine}");
    // Double-click resets both.
    let at = (t.panel(tr).hero.pad_point[0], t.panel(tr).hero.pad_point[1]);
    t.click(at, 2);
    assert_eq!((t.param(tr, "timbre"), t.param(tr, "morph")), (50.0, 50.0));
    assert_eq!(pad_drag([0.5, 0.5], [0.9, 0.1], [0.1, -0.1], true), [0.51, 0.49]);
    assert_eq!(pad_drag([0.5, 0.5], [0.9, 0.1], [0.1, -0.1], false), [0.9, 0.1]);
}

#[test]
fn a_route_to_timbre_draws_its_trail_on_the_pad() {
    let (mut t, tr) = T::open();
    assert!(t.panel(tr).hero.pad_trails.is_empty(), "no route: no trail");
    t.set(tr, "mod1_source", 1.0);
    t.set(tr, "mod1_dest", 3.0);
    t.set(tr, "mod1_amount", 30.0);
    let h = t.panel(tr).hero;
    assert_eq!(h.pad_trails.len(), 1);
    let [x0, y0, x1, y1] = h.pad_trails[0];
    assert!((x1 - x0 - (PAD - 2.0) * 0.3).abs() < 1.0 && y0 == y1, "across, 30 percent of the pad: {:?}", h.pad_trails[0]);
}

#[test]
fn the_picker_lists_every_model_numbered_in_two_groups_and_selects_on_click() {
    let (mut t, tr) = T::open();
    let h = t.panel(tr).hero;
    assert_eq!(h.picker.len(), MODELS.len());
    assert_eq!(h.groups.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["Pitched", "Noise · Perc"]);
    assert_eq!(h.picker.iter().filter(|p| p.2).map(|p| p.0).collect::<Vec<_>>(), [8], "the current model only");
    for (m, r, _) in &h.picker {
        assert_eq!(size(*r).1, MODEL_H);
        let group_y = if *m >= NOISE_FROM { h.groups[1].1 } else { h.groups[0].1 };
        assert!(r[1] <= group_y[3] + 2.0 * MODEL_H, "model {m} sits in its group's rows");
    }
    assert_eq!(t.painted("09").0, 1, "numbered in the firmware's order");
    let (_, r, _) = h.picker[21];
    t.click(mid(r), 1);
    assert_eq!(t.param(tr, "model"), 21.0);
    assert_eq!(t.row(tr, "model").value, MODELS[21]);
}

#[test]
fn arrow_keys_move_the_model_while_the_picker_has_the_focus() {
    assert_eq!(model_step(8, egui::Key::ArrowRight), Some(9));
    assert_eq!(model_step(8, egui::Key::ArrowDown), Some(16));
    assert_eq!(model_step(0, egui::Key::ArrowLeft), None, "the first stays");
    assert_eq!(model_step(MODELS.len() - 1, egui::Key::ArrowRight), None, "the last stays");
    let (mut t, tr) = T::open();
    // Negative control: before the picker has the focus, an arrow changes nothing.
    t.key("ArrowRight");
    assert_eq!(t.param(tr, "model"), 8.0);
    let (_, r, _) = t.panel(tr).hero.picker[8];
    t.click(mid(r), 1);
    t.key("ArrowRight");
    assert_eq!(t.param(tr, "model"), 9.0);
    t.key("ArrowDown");
    assert_eq!(t.param(tr, "model"), 17.0);
    t.key("ArrowLeft");
    assert_eq!(t.param(tr, "model"), 16.0);
}

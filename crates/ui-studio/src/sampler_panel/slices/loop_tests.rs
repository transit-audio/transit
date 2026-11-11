//! The per-slice loop (pass G4): the Playback section's loop row, the One-shot ghost and the bench's
//! looping slices, through the panel's rig.

use super::super::test_rig::{T, mid};
use super::loop_row::{LoopRowLayout, ONE_SHOT_LOOP, XF_NONE};
use super::rig::*;
use crate::theme::TOKENS;
use crate::tokens::{self, sampler as tk};
use serde_json::json;
use transit_model::TrackId;
use transit_model::sampler_patch::{LoopMode, PlayMode};

fn row(t: &T, tr: TrackId) -> LoopRowLayout {
    panels(t, tr).loop_row
}

/// Selects slice `index` of the showing set by its cell in the flag strip.
fn select(t: &mut T, tr: TrackId, index: usize) {
    let cell = bench(t, tr).cells.iter().find(|c| c.0 == index).map(|c| c.1).expect("the slice's cell");
    t.click(mid(cell), 1);
    assert_eq!(view(t).selected, index);
}

/// The zone pad `pad` (0-based) plays, 0-based.
fn zone_of(t: &T, tr: TrackId, pad: u8) -> u16 {
    t.patch(tr).pad(pad).zone.expect("the pad's zone")
}

/// A zone edit run directly (fixture setup, not through the panel).
fn zone_edit(t: &mut T, tr: TrackId, zone: u16, p: serde_json::Value) {
    let mut p = p;
    p["track"] = json!(tr.0);
    p["zone"] = json!(zone + 1);
    t.run("mix.sampler_zone_edit", p);
}

/// The sliced fixture with slice 2's zone looping forward as Gate with crossfade `xf` frames, and
/// slice 2 selected.
fn looping(xf: u64) -> (T, TrackId, u16) {
    let (mut t, tr) = sliced();
    let z = zone_of(&t, tr, 1);
    zone_edit(&mut t, tr, z, json!({"region": {"loop_mode": "forward", "crossfade": xf}, "playback": {"mode": "gate"}}));
    select(&mut t, tr, 1);
    (t, tr, z)
}

#[test]
fn loop_press_sends_the_loop_and_gate_in_one_edit() {
    let (mut t, tr) = sliced();
    select(&mut t, tr, 1);
    let z = zone_of(&t, tr, 1);
    assert_eq!(t.patch(tr).zone(z).unwrap().mode, PlayMode::OneShot, "a slice pad is one-shot by default");
    let r = row(&t, tr);
    assert_eq!((r.zone, r.selected), (Some(z + 1), 0), "the selected slice's zone, its loop off");
    assert!(r.one_shot.is_none(), "One-shot is live while the loop is off");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(r.segments[1]), 1);
    assert_eq!(
        sent(&t, log),
        [format!("mix.sampler_zone_edit {{\"playback\":{{\"mode\":\"gate\"}},\"region\":{{\"loop_mode\":\"forward\"}},\"zone\":{}}}", z + 1)],
        "the loop mode and Gate in one edit"
    );
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
    let e = t.patch(tr).zone(z).cloned().unwrap();
    assert_eq!((e.mode, e.loop_mode), (PlayMode::Gate, LoopMode::Forward));
    let r = row(&t, tr);
    assert_eq!(r.selected, 1);
    assert_eq!((r.fills[1], r.inks[1]), (TOKENS.loop_region, TOKENS.ink_dark), "the chosen loop is filled loop with an ink.dark label");
    assert_eq!(r.one_shot.map(|o| o.1), Some(TOKENS.ink_arrow), "One-shot ghosts while the slice loops");
    assert!(r.error.is_none());
}

#[test]
fn ping_pong_press_sends_crossfade_zero_and_its_undo_restores_the_crossfade() {
    let (mut t, tr, z) = looping(441);
    let r = row(&t, tr);
    assert!(r.crossfade_live);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(r.segments[2]), 1);
    assert_eq!(
        sent(&t, log),
        [format!(
            "mix.sampler_zone_edit {{\"playback\":{{\"mode\":\"gate\"}},\"region\":{{\"crossfade\":0,\"loop_mode\":\"ping_pong\"}},\"zone\":{}}}",
            z + 1
        )]
    );
    assert_eq!(t.undo_len(), undo + 1);
    let e = t.patch(tr).zone(z).cloned().unwrap();
    assert_eq!((e.loop_mode, e.crossfade), (LoopMode::PingPong, 0));
    let r = row(&t, tr);
    assert_eq!((r.crossfade_text.as_str(), r.crossfade_ink, r.crossfade_live), (XF_NONE, TOKENS.ink_arrow, false), "xf — under ping-pong");
    t.run("edit.undo", json!({}));
    let e = t.patch(tr).zone(z).cloned().unwrap();
    assert_eq!((e.loop_mode, e.crossfade), (LoopMode::Forward, 441), "the press's undo restores the crossfade");
}

#[test]
fn one_shot_ghosts_while_the_slice_loops_and_its_press_sends_nothing() {
    let (mut t, tr, z) = looping(0);
    let r = row(&t, tr);
    let (seg, ink) = r.one_shot.expect("One-shot is ghosted");
    assert_eq!(ink, TOKENS.ink_arrow);
    assert_eq!(seg, t.panel(tr).mode.unwrap().segments[0], "over the trigger's One-shot segment");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(seg), 1);
    assert_eq!(sent(&t, log), Vec::<String>::new(), "the press sends nothing");
    assert!(t.app.builtin_editor.devices.sampler.views.values().next().unwrap().note.is_none(), "no zone edit was sent and refused");
    assert_eq!(t.undo_len(), undo);
    assert_eq!(t.patch(tr).zone(z).unwrap().mode, PlayMode::Gate);
    assert_eq!(row(&t, tr).error.map(|e| (e.text, e.edge)), Some((ONE_SHOT_LOOP.to_string(), TOKENS.meter_hot)), "the reason as an error banner");
    // The next accepted edit in the row takes the banner away.
    t.click(mid(row(&t, tr).segments[0]), 1);
    assert_eq!(t.patch(tr).zone(z).unwrap().loop_mode, LoopMode::Off);
    let r = row(&t, tr);
    assert!(r.error.is_none() && r.one_shot.is_none());
}

/// Focuses the crossfade field, replaces its text with `text` and presses Enter.
fn type_crossfade(t: &mut T, tr: TrackId, text: &str) {
    let f = row(t, tr).crossfade;
    t.click(mid(f), 1);
    steps(
        t,
        vec![
            key(egui::Key::A, true, egui::Modifiers::COMMAND),
            egui::Event::Text(text.to_string()),
            key(egui::Key::Enter, true, egui::Modifiers::NONE),
        ],
    );
}

#[test]
fn crossfade_field_takes_milliseconds_at_the_source_rate() {
    // The loop's source runs at 44.1 kHz in a 48 kHz session.
    let (mut t, tr, z) = looping(0);
    let r = row(&t, tr);
    assert_eq!((r.crossfade_text.as_str(), r.crossfade_live), ("xf 0.0 ms", true));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_crossfade(&mut t, tr, "5");
    // 5 ms at 44 100 Hz is 220.5 frames: ties upward, 221.
    assert_eq!(sent(&t, log), [format!("mix.sampler_zone_edit {{\"region\":{{\"crossfade\":221}},\"zone\":{}}}", z + 1)], "only the crossfade");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.patch(tr).zone(z).unwrap().crossfade, 221);
    let r = row(&t, tr);
    assert_eq!(r.crossfade_text, "xf 5.0 ms", "221 frames at 44.1 kHz read 5.0 ms");
    assert!(r.crossfade_tip.starts_with("221 frames"), "{}", r.crossfade_tip);
    // A value the engine refuses is its words in the banner (no UI check first).
    let log = view(&t).sent.len();
    type_crossfade(&mut t, tr, "200");
    assert_eq!(sent(&t, log).len(), 1);
    let e = row(&t, tr).error.expect("the engine's refusal");
    assert_eq!(e.text, "Crossfade is at most 125.0 ms (5512 frames) for this loop.");
    assert_eq!(t.patch(tr).zone(z).unwrap().crossfade, 221);
}

#[test]
fn the_warning_says_what_crossfade_is_used() {
    // 100 ms at 44.1 kHz on slice 2 (11 025 frames, 250 ms long).
    let (mut t, tr, z) = looping(4410);
    assert!(row(&t, tr).warning.is_none());
    // Moving marker 3 shortens slice 2 to 5513 frames: playback clamps the crossfade to 2756.
    let id = t.patch(tr).slicing.unwrap().manual.unwrap().markers[2].id;
    t.run("mix.sampler_slice_marker", json!({"track": tr.0, "op": "move", "id": id, "frame": 16_538, "snap": false}));
    assert_eq!(t.patch(tr).zone(z).unwrap().crossfade, 4410, "the stored value is kept");
    let r = row(&t, tr);
    assert_eq!(r.crossfade_text, "xf 100.0 ms", "the field shows the stored value");
    assert_eq!(r.warning.map(|w| w.0).as_deref(), Some("Slice 2 is 125.0 ms long now: 62.5 ms of crossfade is used."));
    let b = bench(&t, tr);
    let m = b.loops.iter().find(|m| m.index == 1).expect("slice 2 loops");
    let (hatch, _) = m.hatch.expect("its crossfade's hatch");
    let px = (b.well[2] - b.well[0] - 2.0) / 44_100.0;
    assert_eq!(h(hatch[2] - hatch[0]), h(2756.0 * px), "the hatch covers what plays");
}

#[test]
fn a_looping_slice_carries_its_band_wash_glyph_and_hatch() {
    let (mut t, tr, _) = looping(441);
    let z3 = zone_of(&t, tr, 2);
    zone_edit(&mut t, tr, z3, json!({"region": {"loop_mode": "ping_pong"}, "playback": {"mode": "gate"}}));
    let b = bench(&t, tr);
    assert_eq!(b.loops.iter().map(|m| m.index).collect::<Vec<_>>(), [1, 2], "slices 2 and 3 loop");
    let band = b.bands.iter().find(|x| x.0 == 1).unwrap().1;
    let m = &b.loops[0];
    assert_eq!(m.band, [band[0], b.strip[3], band[2], b.strip[3] + tk::SLICE_LOOP_BAND], "a 4 px band right under the flag strip");
    assert_eq!(m.wash, band);
    assert_eq!((m.color, m.wash_color), (TOKENS.loop_region, TOKENS.loop_region.gamma_multiply(tk::SLICE_LOOP_WASH_ALPHA)));
    let (hatch, lines) = m.hatch.expect("the crossfade's hatch");
    assert_eq!(h(hatch[2]), h(band[2]), "at the slice's end");
    assert!(lines > 0);
    let flag = |i: usize| b.flags.iter().find(|f| f.index == i).unwrap().text.clone();
    assert_eq!((flag(1), flag(2), flag(3)), ("2 ↻".to_string(), "3 ⇄".to_string(), "4".to_string()));
    assert!(b.loops[1].hatch.is_none(), "a ping-pong loop has no crossfade");
}

#[test]
fn a_one_frame_slice_refuses_its_loop_in_the_engines_words() {
    let (mut t, tr) = sliced();
    t.run("mix.sampler_slice_marker", json!({"track": tr.0, "op": "add", "frame": 11_026, "snap": false}));
    t.run("mix.sampler_slices_to_pads", json!({"track": tr.0}));
    t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices.selected = 1;
    t.frames(3);
    let r = row(&t, tr);
    assert!(r.zone.is_some(), "slice 2 has its pad's zone");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(r.segments[1]), 1);
    assert_eq!(sent(&t, log).len(), 1, "the press is sent: the engine decides");
    assert_eq!(t.undo_len(), undo);
    assert_eq!(row(&t, tr).error.map(|e| e.text).as_deref(), Some("Slice 2 is one frame long, too short to loop."));
}

#[test]
fn without_a_zone_the_loop_row_takes_no_input() {
    let (mut t, tr) = auto();
    let r = row(&t, tr);
    assert_eq!((r.zone, r.opacity), (None, tokens::alpha::DISABLED_SECTION), "no slice is on a pad yet");
    let log = view(&t).sent.len();
    t.click(mid(r.segments[1]), 1);
    assert_eq!(sent(&t, log), Vec::<String>::new());
    assert!(!r.crossfade_live);
}

#[test]
fn the_loop_row_sits_under_the_playback_knobs_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = sliced();
        t.screen = egui::vec2(1400.0, 1400.0);
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let p = t.panel(tr);
        let r = row(&t, tr);
        let play = p.panel("playback").expect("the Playback panel");
        let amp = p.panel("amp").expect("the Amp panel");
        let knob = p.knob("level").unwrap().knob.rect;
        eprintln!("scale {scale}: playback {play:?} track {:?} crossfade {:?}", r.track, r.crossfade);
        let (pv, ph) = (tokens::layout::PANEL_PADDING[0], tokens::layout::PANEL_PADDING[1]);
        assert_eq!(h(play[1] - panels(&t, tr).left[3]), h(tokens::layout::PANEL_GAP), "a panel gap under the panels row");
        assert_eq!((h(r.track[0]), h(r.track[1])), (h(play[0] + ph), h(knob[3] + super::super::zone::ROW_GAP)), "under the knobs");
        assert_eq!(h(r.crossfade[0] - r.track[2]), h(tokens::SPACE[1]));
        assert!(r.crossfade[2] < play[2] - ph, "inside the panel");
        assert_eq!(h(play[3] - r.track[3]), h(pv), "the panel's padding under the row");
        assert_eq!((h(amp[1]), h(amp[3])), (h(play[1]), h(play[3])), "Amp shares the row's height");
    }
}

#[test]
fn leaving_the_crossfade_field_unchanged_sends_nothing() {
    // 223 frames at 44.1 kHz read 5.1 ms, which would convert back to 225 frames.
    let (mut t, tr, z) = looping(223);
    assert_eq!(row(&t, tr).crossfade_text, "xf 5.1 ms");
    let log = view(&t).sent.len();
    t.click(mid(row(&t, tr).crossfade), 1);
    steps(&mut t, vec![key(egui::Key::Enter, true, egui::Modifiers::NONE)]);
    assert_eq!(sent(&t, log), Vec::<String>::new(), "focus and Enter without typing change nothing");
    assert_eq!(t.patch(tr).zone(z).unwrap().crossfade, 223);
}

/// Focuses the crossfade field and replaces its text with `text` (no Enter).
fn type_only(t: &mut T, tr: TrackId, text: &str) {
    t.click(mid(row(t, tr).crossfade), 1);
    steps(t, vec![key(egui::Key::A, true, egui::Modifiers::COMMAND), egui::Event::Text(text.to_string())]);
}

/// Clicks slice `index`'s cell on the bench (the natural next click after typing).
fn click_cell(t: &mut T, tr: TrackId, index: usize) {
    let cell = bench(t, tr).cells.iter().find(|c| c.0 == index).map(|c| c.1).expect("the slice's cell");
    t.click(mid(cell), 1);
    assert_eq!(view(t).selected, index);
}

#[test]
fn a_crossfade_typed_for_one_slice_commits_to_it_when_another_slice_is_clicked() {
    // Slice 2 loops forward with 221 frames, slice 3 forward with none; slice 2 is selected.
    let (mut t, tr, z2) = looping(221);
    let z3 = zone_of(&t, tr, 2);
    zone_edit(&mut t, tr, z3, json!({"region": {"loop_mode": "forward", "crossfade": 0}, "playback": {"mode": "gate"}}));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_only(&mut t, tr, "7");
    click_cell(&mut t, tr, 2);
    // 7 ms at 44 100 Hz is 308.7 frames: 309, on slice 2's zone, the one it was typed for.
    assert_eq!(sent(&t, log), [format!("mix.sampler_zone_edit {{\"region\":{{\"crossfade\":309}},\"zone\":{}}}", z2 + 1)]);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!((t.patch(tr).zone(z2).unwrap().crossfade, t.patch(tr).zone(z3).unwrap().crossfade), (309, 0), "slice 3 keeps its own");
    let r = row(&t, tr);
    assert_eq!((r.zone, r.crossfade_text.as_str()), (Some(z3 + 1), "xf 0.0 ms"), "the field shows slice 3's stored value");
    assert_eq!(view(&t).play.typing, None);
}

#[test]
fn a_crossfade_typed_for_a_slice_is_not_left_on_screen_when_the_next_slice_does_not_loop() {
    let (mut t, tr, z2) = looping(0);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_only(&mut t, tr, "7");
    // Slice 3 does not loop: its field is `xf —`, and the typing commits to slice 2.
    click_cell(&mut t, tr, 2);
    assert_eq!(sent(&t, log), [format!("mix.sampler_zone_edit {{\"region\":{{\"crossfade\":309}},\"zone\":{}}}", z2 + 1)]);
    assert_eq!(t.undo_len(), undo + 1);
    let r = row(&t, tr);
    assert_eq!((r.crossfade_text.as_str(), r.crossfade_live), (XF_NONE, false));
    assert_eq!(view(&t).play.typing, None, "no typed text is kept");
    // Back on slice 2 the field reads the stored value, and a click in and out sends nothing.
    click_cell(&mut t, tr, 1);
    assert_eq!(row(&t, tr).crossfade_text, "xf 7.0 ms");
    let log = view(&t).sent.len();
    t.click(mid(row(&t, tr).crossfade), 1);
    t.click(mid(bench(&t, tr).hint_rect), 1);
    assert_eq!(sent(&t, log), Vec::<String>::new());
    assert_eq!(t.patch(tr).zone(z2).unwrap().crossfade, 309);
}

#[test]
fn a_knob_value_typed_for_one_slice_never_lands_on_the_next() {
    let (mut t, tr) = sliced();
    select(&mut t, tr, 1);
    let (z2, z3) = (zone_of(&t, tr, 1), zone_of(&t, tr, 2));
    let attack = |t: &T, z: u16| t.patch(tr).zone(z).unwrap().attack_ms;
    let (a2, a3) = (attack(&t, z2), attack(&t, z3));
    t.click(mid(t.panel(tr).knob("attack_ms").unwrap().knob.value), 1);
    assert!(t.panel(tr).knob("attack_ms").unwrap().knob.editing, "a click on the value types it");
    steps(&mut t, vec![key(egui::Key::A, true, egui::Modifiers::COMMAND), egui::Event::Text("50".to_string())]);
    click_cell(&mut t, tr, 2);
    assert!(!t.panel(tr).knob("attack_ms").unwrap().knob.editing, "the value typed for slice 2 is dropped");
    let undo = t.undo_len();
    steps(&mut t, vec![key(egui::Key::Enter, true, egui::Modifiers::NONE), key(egui::Key::Enter, false, egui::Modifiers::NONE)]);
    assert_eq!(t.undo_len(), undo, "Enter commits nothing");
    assert_eq!((attack(&t, z2), attack(&t, z3)), (a2, a3), "neither zone changed");
}

#[test]
fn escape_in_the_crossfade_field_cancels_and_keeps_the_window() {
    let (mut t, tr, z) = looping(221);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_only(&mut t, tr, "9");
    assert_eq!(row(&t, tr).crossfade_text, "9");
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.iter().any(|k| k.0 == tr), "the field keeps Escape: the window stays");
    assert_eq!(sent(&t, log), Vec::<String>::new(), "nothing is sent");
    assert_eq!(t.undo_len(), undo);
    assert_eq!(t.patch(tr).zone(z).unwrap().crossfade, 221);
    assert_eq!(row(&t, tr).crossfade_text, "xf 5.0 ms", "the stored value shows again");
    assert_eq!(view(&t).play.typing, None);
    // With nothing typed, Escape closes the window as usual.
    t.key("Escape");
    assert!(!t.app.ui.plugin_windows.iter().any(|k| k.0 == tr));
}

#[test]
fn the_one_shot_sentence_is_the_engines_own_refusal() {
    // The loop row never sends a loop on a one-shot (it sends Gate with it), so the sentence the
    // One-shot press shows is tied here to the engine's refusal of exactly that edit.
    let (mut t, tr) = sliced();
    let z = zone_of(&t, tr, 1);
    let e = t.app.engine.execute("mix.sampler_zone_edit", &json!({"track": tr.0, "zone": z + 1, "region": {"loop_mode": "forward"}}));
    assert_eq!(e.map_err(|e| super::edit::words(&e)).err().as_deref(), Some(ONE_SHOT_LOOP));
}

#[test]
fn the_playback_banner_goes_when_the_document_changes_underneath() {
    let (mut t, tr, z) = looping(0);
    let seg = row(&t, tr).one_shot.expect("One-shot is ghosted").0;
    t.click(mid(seg), 1);
    assert_eq!(row(&t, tr).error.map(|e| e.text).as_deref(), Some(ONE_SHOT_LOOP));
    // An edit from elsewhere (not this row) takes it away.
    zone_edit(&mut t, tr, z, json!({"playback": {"attack_ms": 5.0}}));
    assert!(row(&t, tr).error.is_none(), "an outside edit clears the banner");
    // So does an undo. (The banner's coming and going can scroll the window's body: the segment is
    // read again.)
    let seg = row(&t, tr).one_shot.expect("One-shot is ghosted").0;
    t.click(mid(seg), 1);
    assert!(row(&t, tr).error.is_some());
    t.run("edit.undo", json!({}));
    assert!(row(&t, tr).error.is_none(), "an undo clears the banner");
}

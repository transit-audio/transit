//! The design knob: geometry from tokens.json, the layout under the dial, the tokens per state,
//! and the gestures (drag, Shift drag, double-click, scroll, click the value and type).

use super::knob::{self, Knob, KnobOut, Size, State};
use super::test_support::{Rig, inside, mid, size};
use crate::theme::{DARK, LIGHT};
use egui::{Event, Key, Modifiers, vec2};

fn id() -> egui::Id {
    egui::Id::new("k")
}

fn draw(ui: &mut egui::Ui, k: &Knob) -> KnobOut {
    knob::show(ui, id(), k)
}

#[test]
fn the_sweep_is_270_degrees_from_7_oclock_and_bipolar_arcs_start_at_12() {
    assert_eq!((knob::angle(0.0), knob::angle(0.5), knob::angle(1.0)), (-135.0, 0.0, 135.0));
    assert_eq!(knob::angle(3.0), 135.0, "clamped");
    assert_eq!(knob::arc_span(0.25, false), [-135.0, -67.5], "unipolar from 7 o'clock");
    assert_eq!(knob::arc_span(0.25, true), [-67.5, 0.0], "bipolar from 12 o'clock, toward the value");
    assert_eq!(knob::arc_span(0.75, true), [0.0, 67.5]);
    assert_eq!(knob::mod_span(0.5, 0.25), [0.0, 67.5], "base to base plus depth");
    assert_eq!(knob::mod_span(0.5, -0.25), [-67.5, 0.0], "negative depth runs backward");
    assert_eq!(knob::mod_span(0.9, 0.5), [108.0, 135.0], "clamped at the stop");
}

#[test]
fn sizes_and_radii_scale_the_100_unit_box() {
    let mut rig = Rig::new();
    for (size, d) in [(Size::L, 72.0), (Size::M, 44.0), (Size::S, 38.0)] {
        let out = rig.idle(|ui| draw(ui, &Knob { size, ..Knob::new("Cutoff", "1.20 kHz", 0.5) }));
        let l = out.layout;
        assert_eq!(size_of(l.dial), (d, d), "{size:?}");
        let s = d / 100.0;
        assert_eq!(l.radii, [40.0 * s, 47.0 * s, 28.0 * s], "{size:?}: track, ring, cap radii");
        // Label below the dial, value below the label, all inside the cell.
        assert!(l.label[1] >= l.dial[3] && l.value[1] >= l.label[3], "{l:?}");
        for r in [l.dial, l.label, l.value] {
            assert!(inside(r, l.rect), "{r:?} in {:?}", l.rect);
        }
    }
}

fn size_of(r: [f32; 4]) -> (f32, f32) {
    size(r)
}

#[test]
fn each_state_paints_its_tokens_in_both_themes() {
    for (theme, t) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        let mut rig = Rig::themed(theme);
        let l = rig.idle(|ui| draw(ui, &Knob::new("Cutoff", "1.20 kHz", 0.3))).layout;
        assert_eq!((l.state, l.cap, l.arc, l.pointer), (State::Default, t.knob_cap, t.signal, t.text), "{theme:?}");
        assert_eq!(l.mod_deg, None, "no ring without modulation");
        let l = rig.idle(|ui| draw(ui, &Knob { modulation: Some(0.2), ..Knob::new("Cutoff", "1.20 kHz", 0.3) })).layout;
        assert_eq!(l.state, State::Modulated);
        assert_eq!(l.mod_deg, Some(knob::mod_span(0.3, 0.2)));
        let l = rig.idle(|ui| draw(ui, &Knob { disabled: true, modulation: Some(0.2), ..Knob::new("Cutoff", "1.20 kHz", 0.3) })).layout;
        assert_eq!((l.state, l.arc, l.pointer), (State::Disabled, t.ink_ghost, t.ink_arrow), "{theme:?}");
        assert_eq!(l.mod_deg, None, "a disabled knob hides its ring");
        // Hovered: the cap lifts.
        let at = mid(l.dial);
        let l = rig.frame(vec![Event::PointerMoved(at)], |ui| draw(ui, &Knob::new("Cutoff", "1.20 kHz", 0.3))).layout;
        assert_eq!((l.state, l.cap), (State::Active, t.knob_cap_active), "{theme:?}");
    }
}

#[test]
fn dragging_up_raises_shift_is_ten_times_finer_and_a_disabled_knob_ignores_it() {
    let mut rig = Rig::new();
    let dial = rig.idle(|ui| draw(ui, &Knob::new("Gain", "0.0 dB", 0.5))).layout.dial;
    let moved = |rig: &mut Rig, disabled: bool| -> f32 {
        let mut v = 0.5;
        rig.drag(mid(dial), vec2(0.0, -40.0), 4, |ui| {
            let out = draw(ui, &Knob { disabled, ..Knob::new("Gain", "0.0 dB", v) });
            if let Some(n) = out.value {
                v = n;
            }
        });
        v
    };
    let coarse = moved(&mut rig, false);
    assert!((coarse - (0.5 + 40.0 / knob::DRAG_PX)).abs() < 1e-4, "{coarse}");
    rig.modifiers = Modifiers::SHIFT;
    let fine = moved(&mut rig, false);
    assert!((fine - (0.5 + 40.0 / knob::DRAG_PX / 10.0)).abs() < 1e-4, "{fine}");
    rig.modifiers = Modifiers::NONE;
    assert_eq!(moved(&mut rig, true), 0.5);
}

#[test]
fn double_click_resets_to_the_default() {
    let mut rig = Rig::new();
    let k = |v| Knob { default: 0.25, ..Knob::new("Gain", "0.0 dB", v) };
    let dial = rig.idle(|ui| draw(ui, &k(0.8))).layout.dial;
    let at = mid(dial);
    let mut got = None;
    for events in
        [Rig::press(at, Modifiers::NONE), Rig::release(at, Modifiers::NONE), Rig::press(at, Modifiers::NONE), Rig::release(at, Modifiers::NONE)]
    {
        let out = rig.frame(events, |ui| draw(ui, &k(0.8)));
        got = got.or(out.value);
    }
    assert_eq!(got, Some(0.25));
}

/// A double click soon after any other click is a triple click to egui (within twice
/// `max_double_click_delay` of the click before last); it resets all the same (326-X), as three
/// clicks on the dial do.
#[test]
fn a_triple_click_resets_too() {
    let k = |v| Knob { default: 0.25, ..Knob::new("Gain", "0.0 dB", v) };
    for elsewhere in [true, false] {
        let mut rig = Rig::new();
        let dial = rig.idle(|ui| draw(ui, &k(0.8))).layout.dial;
        let at = mid(dial);
        // The first click: elsewhere on the screen (a track, another control), or on the dial.
        let first = if elsewhere { egui::pos2(700.0, 500.0) } else { at };
        let mut clicks = vec![Rig::press(first, Modifiers::NONE), Rig::release(first, Modifiers::NONE)];
        for _ in 0..2 {
            clicks.push(Rig::press(at, Modifiers::NONE));
            clicks.push(Rig::release(at, Modifiers::NONE));
        }
        let mut on_triple = Vec::new();
        for events in clicks {
            let out = rig.frame(events, |ui| draw(ui, &k(0.8)));
            if out.response.triple_clicked() {
                // Negative control: egui reports this click as a triple and not as a double, so
                // only the triple branch can reset on it.
                assert!(!out.response.double_clicked());
                on_triple.push(out.value);
            }
        }
        assert_eq!(on_triple, [Some(0.25)], "the triple click resets (elsewhere first: {elsewhere})");
    }
}

#[test]
fn ctrl_scrolling_nudges_by_one_step_per_notch_and_a_plain_wheel_does_nothing() {
    let mut rig = Rig::new();
    let dial = rig.idle(|ui| draw(ui, &Knob::new("Gain", "0.0 dB", 0.5))).layout.dial;
    let out = rig.frame(Rig::wheel(mid(dial), 40.0, Modifiers::COMMAND), |ui| draw(ui, &Knob::new("Gain", "0.0 dB", 0.5)));
    assert_eq!(out.value, Some(0.51));
    let out = rig.frame(Rig::wheel(mid(dial), -40.0, Modifiers::CTRL), |ui| draw(ui, &Knob::new("Gain", "0.0 dB", 0.5)));
    assert_eq!(out.value, Some(0.49));
    // A plain wheel is the scroll area's, never the knob's.
    let out = rig.frame(Rig::wheel(mid(dial), 40.0, Modifiers::NONE), |ui| draw(ui, &Knob::new("Gain", "0.0 dB", 0.5)));
    assert_eq!(out.value, None);
    assert!((knob::nudged(0.5, 1.0, true) - (0.5 + 0.001)).abs() < 1e-6, "Shift nudges finer");
}

#[test]
fn clicking_the_value_opens_a_field_and_enter_returns_the_typed_text() {
    let mut rig = Rig::new();
    let k = Knob::new("Cutoff", "1.20 kHz", 0.5);
    let value = rig.idle(|ui| draw(ui, &k)).layout.value;
    let out = rig.click(mid(value), |ui| draw(ui, &k));
    let out = if out.layout.editing { out } else { rig.idle(|ui| draw(ui, &k)) };
    assert!(out.layout.editing, "the value field opens");
    rig.frame(vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }], |ui| draw(ui, &k));
    rig.frame(vec![Event::Text("440 Hz".into())], |ui| draw(ui, &k));
    let out = rig
        .frame(vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], |ui| draw(ui, &k));
    assert_eq!(out.typed.as_deref(), Some("440 Hz"));
    assert!(!out.layout.editing, "Enter closes the field");
    assert_eq!(out.value, None, "typing reports text; the caller parses it");
}

#[test]
fn a_drag_accumulates_so_a_rounding_caller_follows_a_slow_pointer() {
    // A caller that rounds to 4 steps (a Choice) and passes the rounded value back. Each frame's
    // 2 points are 1 percent of the travel, far under the half step of 12.5 percent.
    let round = |n: f32| (n * 4.0).round() / 4.0;
    let mut rig = Rig::new();
    let dial = rig.idle(|ui| draw(ui, &Knob::new("Rate", "1/4", 0.5))).layout.dial;
    let mut shown = 0.5;
    let mut painted = Vec::new();
    rig.drag(mid(dial), vec2(0.0, -160.0), 40, |ui| {
        let out = draw(ui, &Knob::new("Rate", "1/4", shown));
        painted.push(out.layout.pointer_deg);
        if let Some(n) = out.value {
            shown = round(n);
        }
    });
    assert_eq!(shown, 1.0, "160 points up is 80 percent: it reaches the top step");
    // The dial painted only stored values, never the raw position between steps.
    for deg in painted {
        assert!([0.0, 67.5, 135.0, -67.5, -135.0].iter().any(|s| (deg - s).abs() < 0.01), "{deg}");
    }
    // Negative control: without the gesture's memory (each frame from the rounded value) it never moves.
    let stuck = (0..40).fold(0.5, |n, _| round(knob::dragged(n, -4.0, false)));
    assert_eq!(stuck, 0.5);
}

/// The knob and a second text field (the browser's search, say) side by side.
fn with_field(ui: &mut egui::Ui, k: &Knob, other: &mut String) -> (KnobOut, egui::Response) {
    let out = draw(ui, k);
    let field =
        ui.put(egui::Rect::from_min_size(egui::pos2(300.0, 40.0), vec2(120.0, 20.0)), egui::TextEdit::singleline(other).id(egui::Id::new("other")));
    (out, field)
}

#[test]
fn a_click_elsewhere_commits_the_typed_value_and_hands_the_keyboard_on() {
    // #484: the value field asked for the keyboard every frame, so a click in another field never
    // took it, the typing went on into the value field, and the app's shortcuts stayed dead.
    let mut rig = Rig::new();
    let k = Knob::new("Cutoff", "1.20 kHz", 0.5);
    let mut other = String::new();
    let value = rig.idle(|ui| with_field(ui, &k, &mut other)).0.layout.value;
    rig.click(mid(value), |ui| with_field(ui, &k, &mut other));
    let out = rig.idle(|ui| with_field(ui, &k, &mut other)).0;
    assert!(out.layout.editing, "the value field opens");
    rig.frame(vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }], |ui| {
        with_field(ui, &k, &mut other)
    });
    rig.frame(vec![Event::Text("440 Hz".into())], |ui| with_field(ui, &k, &mut other));
    // Press in the other field: the knob commits what was typed in that frame.
    let field_at = egui::pos2(360.0, 50.0);
    let mut typed = None;
    for events in [Rig::press(field_at, Modifiers::NONE), Rig::release(field_at, Modifiers::NONE)] {
        let out = rig.frame(events, |ui| with_field(ui, &k, &mut other)).0;
        typed = typed.or(out.typed);
    }
    assert_eq!(typed.as_deref(), Some("440 Hz"), "a click elsewhere commits the typing");
    let (out, field) = rig.idle(|ui| with_field(ui, &k, &mut other));
    assert!(!out.layout.editing, "the value field closed");
    assert!(field.has_focus(), "the clicked field has the keyboard");
    rig.frame(vec![Event::Text("kick".into())], |ui| with_field(ui, &k, &mut other));
    assert_eq!(other, "kick", "typing goes to the clicked field");
    // A click on empty space releases the keyboard altogether.
    rig.click(egui::pos2(600.0, 400.0), |ui| with_field(ui, &k, &mut other));
    assert!(!rig.ctx.egui_wants_keyboard_input(), "nothing holds the keyboard");
}

#[test]
fn escape_gives_the_typing_up_and_nothing_is_returned() {
    let mut rig = Rig::new();
    let k = Knob::new("Cutoff", "1.20 kHz", 0.5);
    let value = rig.idle(|ui| draw(ui, &k)).layout.value;
    rig.click(mid(value), |ui| draw(ui, &k));
    rig.frame(vec![Event::Text("9".into())], |ui| draw(ui, &k));
    let out = rig.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], |ui| {
        draw(ui, &k)
    });
    assert_eq!(out.typed, None, "Escape returns no text");
    assert!(!out.layout.editing, "Escape closes the field");
    assert!(!rig.ctx.egui_wants_keyboard_input());
}

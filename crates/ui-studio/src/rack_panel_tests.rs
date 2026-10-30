//! Racks in the UI (ADR 0034, #197): Group into Rack from the header menu, the "Rack" badge in the
//! header and the mixer, the rack card in the device chain (macros, member zones, Play), and the
//! gestures on it: a zone drag and a macro drag are one undo step each. Driven like a user, through
//! the control channel's input queue, a frame at a time. No player, no plug-ins.

use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_model::{TrackId, Zone};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    a: TrackId,
    b: TrackId,
}

impl T {
    /// Two instrument tracks "A" and "B", the mixer and the devices shown.
    fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let mut ids = Vec::new();
        for name in ["A", "B"] {
            let r = app.run("track.new", json!({"kind": "instrument", "name": name})).unwrap();
            ids.push(TrackId(r["tracks"][0].as_u64().unwrap()));
        }
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, a: ids[0], b: ids[1] };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn input(&mut self, method: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(30);
    }

    fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.input("ui.drag", json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1, "steps": 12}));
    }

    fn rack(&mut self) -> TrackId {
        let r = self.app.group_into_rack(&[self.a, self.b]).unwrap();
        self.frames(5);
        r
    }

    fn zone(&self, rack: TrackId, m: TrackId) -> Zone {
        self.app.engine.session().track(rack).unwrap().rack.as_ref().unwrap().member(m).unwrap().key_zone
    }
}

#[test]
fn group_into_rack_shows_the_badge_and_the_rack_card() {
    let mut t = T::new();
    let rack = t.rack();
    assert!(t.app.engine.session().track(rack).unwrap().is_rack());
    let row = t.app.arrange.layout.tracks.iter().find(|r| r.id == rack.0).cloned().unwrap();
    assert_eq!(row.subtitle, "Rack · 2 instruments");
    assert!(row.rack_badge[2] > row.rack_badge[0], "the header wears the badge");
    let card = t.app.mixer.devices.rack.clone().expect("the focused rack's card");
    assert_eq!(card.macros.len(), 8);
    assert_eq!(card.members.iter().map(|m| m.0).collect::<Vec<_>>(), [t.a.0, t.b.0]);
    // The header menu offers it, and Ungroup dissolves a rack like any group.
    t.app.ungroup(&[rack]).unwrap();
    assert!(t.app.engine.session().tracks.iter().all(|x| x.rack.is_none()));
}

#[test]
fn dragging_a_zone_edge_is_one_undo_step() {
    let mut t = T::new();
    let rack = t.rack();
    t.app.engine.mark_clean();
    let steps = t.app.engine.undo_history().len();
    let card = t.app.mixer.devices.rack.clone().unwrap();
    let (_, key, _) = card.members.iter().find(|m| m.0 == t.a.0).copied().unwrap();
    // The lower half near the right edge: the high edge, dragged to the middle (about 64).
    let y = key[1] + (key[3] - key[1]) * 0.8;
    let mid = (key[0] + key[2]) / 2.0;
    t.drag((key[2] - 1.0, y), (mid, y));
    let z = t.zone(rack, t.a);
    assert!((60..=68).contains(&z.high), "the high edge moved to about the middle: {z:?}");
    assert_eq!(z.low, 0);
    assert_eq!(t.app.engine.undo_history().len(), steps + 1, "one drag, one step");
    // The upper half near the left: the fade-in handle.
    let y = key[1] + (key[3] - key[1]) * 0.2;
    t.drag((key[0] + 1.0, y), (key[0] + (key[2] - key[0]) * 0.25, y));
    let z = t.zone(rack, t.a);
    assert!(z.fade_low > 0 && z.fade_low <= z.high, "a fade-in: {z:?}");
    assert_eq!(t.app.engine.undo_history().len(), steps + 2);
    assert!(t.app.engine.undo());
    assert_eq!(t.zone(rack, t.a).fade_low, 0);
}

/// Review finding 6: a double-click reset is its own undo step, closed at once, so a later drag
/// of the same knob does not merge into it.
#[test]
fn a_double_click_reset_is_its_own_step() {
    let mut t = T::new();
    let rack = t.rack();
    t.app.run("rack.macro", json!({"track": rack.0, "macro": 1, "value": 0.5})).unwrap();
    let steps = t.app.engine.undo_history().len();
    let k = t.app.mixer.devices.rack.clone().unwrap().macros[0];
    let c = ((k[0] + k[2]) / 2.0, k[1] + 20.0);
    t.input("ui.click", json!({"x": c.0, "y": c.1, "count": 2}));
    let value = |t: &T| t.app.engine.session().track(rack).unwrap().rack.as_ref().unwrap().macros[0].value;
    assert_eq!(value(&t), 0.0, "reset");
    assert_eq!(t.app.engine.undo_history().len(), steps + 1);
    // The knob's next move, under the knob's own gesture key, is a step of its own.
    let key = format!("rack_macro:{}:0", rack.0);
    t.app.engine.execute_merged("rack.macro", &json!({"track": rack.0, "macro": 1, "value": 0.7}), &key).unwrap();
    assert_eq!(t.app.engine.undo_history().len(), steps + 2, "the next move is a step of its own");
    assert!(t.app.engine.undo());
    assert_eq!(value(&t), 0.0, "undo takes back the next move only");
}

#[test]
fn turning_a_macro_writes_it_in_one_undo_step() {
    let mut t = T::new();
    let rack = t.rack();
    let steps = t.app.engine.undo_history().len();
    let card = t.app.mixer.devices.rack.clone().unwrap();
    let k = card.macros[2];
    let c = ((k[0] + k[2]) / 2.0, k[1] + 20.0);
    t.drag(c, (c.0, c.1 - 90.0));
    let v = t.app.engine.session().track(rack).unwrap().rack.as_ref().unwrap().macros[2].value;
    assert!(v > 0.3, "dragged up: {v}");
    assert_eq!(t.app.engine.undo_history().len(), steps + 1, "one drag, one step");
}

/// Review finding 8: the rack card's Play and the rack header's I both turn the rack's input
/// monitor on and off (playback then routes live MIDI to the rack).
#[test]
fn play_and_the_header_i_button_monitor_the_rack() {
    let mut t = T::new();
    let rack = t.rack();
    let monitoring = |t: &T| t.app.engine.session().track(rack).unwrap().mixer.input_monitor;
    assert!(!monitoring(&t));
    let play = t.app.mixer.devices.rack.clone().unwrap().play;
    t.input("ui.click", json!({"x": (play[0] + play[2]) / 2.0, "y": (play[1] + play[3]) / 2.0}));
    assert!(monitoring(&t), "Play turns it on");
    let row = t.app.arrange.layout.tracks.iter().find(|r| r.id == rack.0).cloned().unwrap();
    let i = row.monitor;
    t.input("ui.click", json!({"x": (i[0] + i[2]) / 2.0, "y": (i[1] + i[3]) / 2.0}));
    assert!(!monitoring(&t), "the header's I turns it off");
    t.input("ui.click", json!({"x": (i[0] + i[2]) / 2.0, "y": (i[1] + i[3]) / 2.0}));
    assert!(monitoring(&t), "and on again");
}

/// ADR 0036, racks: a rack's own MIDI effects come first in the chain, then the rack card, and the
/// pane is as tall as the taller of the two.
#[test]
fn a_rack_with_midi_effects_shows_both_cards_effects_first() {
    let mut t = T::new();
    let rack = t.rack();
    t.app.add_midi_effect(rack, "midi_transpose").unwrap();
    t.app.add_midi_effect(rack, "midi_velocity").unwrap();
    t.frames(5);
    let (fx, card) = (t.app.mixer.devices.midi_fx.clone().expect("the MIDI effects card"), t.app.mixer.devices.rack.clone().expect("the rack card"));
    assert_eq!(fx.rows.len(), 2);
    assert!(fx.rect[2] <= card.rect[0], "the MIDI effects card is ahead of the rack card: {:?} then {:?}", fx.rect, card.rect);
    assert!(t.app.mixer.devices.cards.is_empty(), "a rack has no instrument card");
    let track = t.app.engine.session().track(rack).unwrap();
    let need = crate::midi_fx_panel::card_height(track).max(crate::rack_panel::card_height(t.app.engine.session(), track)).max(116.0) + 16.0;
    assert!((crate::devices::pane_height(&t.app) - need).abs() < 0.5, "{} covers both cards ({need})", crate::devices::pane_height(&t.app));
    assert!(fx.rect[3] - fx.rect[1] <= crate::devices::pane_height(&t.app) && card.rect[3] - card.rect[1] <= crate::devices::pane_height(&t.app));
}

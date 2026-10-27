use super::*;
use crate::{Services, StudioApp};

struct Harness {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    clip: ClipId,
}
impl Harness {
    fn new() -> Self {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let track = app.run("track.new", json!({"kind":"instrument"})).unwrap()["tracks"][0].clone();
        let clip = ClipId(app.run("midi.new_clip", json!({"tracks":[track],"start":0,"end":96000})).unwrap()["clip"].as_u64().unwrap());
        app.open_keys(clip);
        let mut h = Self { app, ctx: egui::Context::default(), time: 0.0, clip };
        h.frames();
        h
    }
    fn frames(&mut self) {
        for _ in 0..30 {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let mut output = self.ctx.run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            });
            output.textures_delta.clear();
        }
    }
    fn input(&mut self, method: &str, params: Value) {
        let (req, _) = crate::ControlRequest::new(method, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
        self.frames();
    }
    fn pos(&self, tick: i64, v: i32) -> Pos2 {
        let l = &self.app.keys.layout;
        pos2(
            l.velocity[0] + ((tick as f64 - l.scroll_ticks) * l.px_per_tick) as f32,
            l.velocity[3] - 4.0 - (l.velocity[3] - l.velocity[1] - 8.0) * v as f32 / 127.0,
        )
    }
    fn click(&mut self, p: Pos2) {
        self.input("ui.click", json!({"x":p.x,"y":p.y}));
    }
    fn points(&self) -> Vec<CtrlEvent> {
        clip_view(self.app.engine.session(), self.clip).unwrap().ctrls
    }
}
#[test]
fn cc_lane_pointer_draw_move_delete_and_undo_with_sustain_regions() {
    let mut h = Harness::new();
    // Open the real selector, then choose Sustain through egui's menu by its text geometry.
    let r = h.app.keys.layout.cc.selector;
    h.click(pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0));
    let r = h.app.keys.layout.cc.choices.iter().find(|(n, _)| *n == 64).unwrap().1;
    h.click(pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0));
    assert_eq!(h.app.keys.cc.controller, Some(64));
    let before = h.app.engine.undo_pushes();
    h.click(h.pos(480, 100));
    assert_eq!(h.points().len(), 1);
    assert_eq!(h.points()[0].tick, 480);
    assert!((h.points()[0].value - 100).abs() <= 1);
    assert_eq!(h.app.engine.undo_pushes(), before + 1);
    assert!(!h.app.keys.layout.cc.sustain.is_empty());
    let from = h.pos(480, h.points()[0].value);
    let to = h.pos(960, 75);
    h.input("ui.drag", json!({"x":from.x,"y":from.y,"to_x":to.x,"to_y":to.y}));
    assert_eq!(h.points()[0].tick, 960);
    assert!((h.points()[0].value - 75).abs() <= 1);
    assert_eq!(h.app.engine.undo_pushes(), before + 2);
    assert!(h.app.engine.undo());
    h.frames();
    assert_eq!(h.points()[0].tick, 480);
    assert!(h.app.engine.redo());
    h.frames();
    let pos = h.pos(960, h.points()[0].value);
    h.input("ui.click", json!({"x":pos.x,"y":pos.y,"button":"secondary"}));
    assert!(h.points().is_empty());
    assert!(h.app.engine.undo());
    h.frames();
    assert_eq!(h.points().len(), 1);
}

#[test]
fn cc_lane_separates_channels_and_controllers_and_bounds_pedal_regions() {
    let mut h = Harness::new();
    for (tick, channel, controller, value) in
        [(0, 0, 64, 63), (480, 0, 64, 64), (960, 0, 64, 0), (1440, 0, 64, 127), (0, 1, 64, 127), (240, 0, 1, 42)]
    {
        h.app.run("midi.cc_add", json!({"clip":h.clip.0,"tick":tick,"channel":channel,"controller":controller,"value":value})).unwrap();
    }
    h.app.keys.cc.controller = Some(64);
    h.frames();
    let l = &h.app.keys.layout;
    assert_eq!(l.cc.points.len(), 4);
    assert_eq!(l.cc.sustain.len(), 2);
    let x = |tick: i64| l.grid[0] + ((tick as f64 - l.scroll_ticks) * l.px_per_tick) as f32;
    assert!((l.cc.sustain[0][0] - x(480)).abs() < 0.1);
    assert!((l.cc.sustain[0][2] - x(960)).abs() < 0.1);
    assert!((l.cc.sustain[1][2] - x(l.clip_ticks)).abs() < 0.1);
    h.app.keys.cc.channel = 1;
    h.frames();
    assert_eq!(h.app.keys.layout.cc.points.len(), 1);
    assert_eq!(h.app.keys.layout.cc.sustain.len(), 1);
    h.app.keys.cc.channel = 0;
    h.app.keys.cc.controller = Some(1);
    h.frames();
    assert_eq!(h.app.keys.layout.cc.points.len(), 1);
    assert!(h.app.keys.layout.cc.sustain.is_empty());
    h.click(h.pos(960, 90));
    assert_eq!(h.points().iter().filter(|c| c.kind == CtrlKind::Cc(1)).count(), 2);
    assert_eq!(h.points().iter().filter(|c| c.kind == CtrlKind::Cc(64)).count(), 5);
    let before = h.app.engine.undo_pushes();
    let from = h.pos(2400, 40);
    let to = h.pos(2880, 80);
    h.input("ui.drag", json!({"x":from.x,"y":from.y,"to_x":to.x,"to_y":to.y}));
    assert!(h.points().iter().any(|c| c.kind == CtrlKind::Cc(1) && c.tick == 2880 && (c.value - 80).abs() <= 1));
    assert_eq!(h.app.engine.undo_pushes(), before + 1);
}

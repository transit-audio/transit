//! The loop's own hue (#326, the LoopColor and Arrange artboards): the brace, the ruler tint, the
//! edges, the dimmed outside and the Cycle button, with Cycle on and off. Deterministic: the
//! arrangement's layout and the tokens it painted with.

use crate::loopbar::{brace, range_text};
use crate::theme::{DARK, TOKENS};
use crate::{Services, StudioApp};
use serde_json::json;
use transit_time::{BarBeat, Range};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl T {
    fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.run("track.new", json!({"kind": "audio", "name": "A"})).unwrap();
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))),
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

    fn bar(&self, bar: i64) -> i64 {
        let s = self.app.engine.session();
        s.tempo.samples_at_bar_beat(BarBeat { bar, beat: 1, tick: 0 }, s.sample_rate)
    }

    fn set_loop(&mut self, from: i64, to: i64, on: bool) {
        let (a, b) = (self.bar(from), self.bar(to));
        self.app.run("transport.loop", json!({"start": a, "end": b})).unwrap();
        self.app.run("options.loop_playback", json!({"value": on})).unwrap();
        self.frames(3);
    }
}

#[test]
fn the_loop_hue_is_magenta_and_keeps_the_artboard_contrast() {
    use crate::theme::contrast;
    let d = &DARK;
    assert_eq!(d.loop_region, crate::tokens::hex(0xD05DAD));
    assert_eq!(crate::theme::LIGHT.loop_region, crate::tokens::hex(0xB73590));
    assert!((contrast(d.loop_region, d.bg_window) - 5.1).abs() < 0.05, "5.1 on the window");
    assert!((contrast(d.loop_region, d.lane) - 4.87).abs() < 0.05, "4.9 on a lane");
    assert!((contrast(d.loop_region, d.surface_inset) - 5.35).abs() < 0.05, "5.35 on the key rows");
    assert!(contrast(d.ink_dark, d.loop_region) >= 4.5, "the brace's range and the Cycle glyph read on the hue");
    // Negative control: the old amber is not this hue, and the hue is not mod.
    assert_ne!(d.loop_region, crate::tokens::hex(0xE6AB27));
    assert_ne!(d.loop_region, d.modulation);
}

#[test]
fn cycle_on_paints_the_brace_solid_tints_the_ruler_marks_the_edges_and_dims_the_outside() {
    let mut t = T::new();
    t.set_loop(5, 9, true);
    let l = &t.app.arrange.layout;
    let v = &l.loop_view;
    assert_eq!(v.brace_look, brace(true));
    assert_eq!((v.brace_look.fill, v.brace_look.ink, v.brace_look.edge), (DARK.loop_region, DARK.ink_dark, None));
    assert_eq!(v.range_text, "5.1 – 9.1 · 4 bars");
    let tint = v.ruler_tint.expect("the ruler is tinted under the loop");
    assert!((tint[0] - v.brace[0]).abs() < 1.5 && (tint[2] - v.brace[2]).abs() < 1.5, "under the brace: {tint:?} vs {:?}", v.brace);
    assert_eq!(v.edges.len(), 2, "both edges");
    assert_eq!(v.outside.len(), 2, "left and right of the loop");
    let tl = l.timeline;
    assert!((v.outside[0][2] - v.brace[0]).abs() < 1.5 && (v.outside[1][0] - v.brace[2]).abs() < 1.5, "the outside stops at the loop");
    assert!(v.outside.iter().all(|r| r[1] >= tl[1] - 0.5 && r[3] <= tl[3] + 0.5), "over the lanes, not the ruler");
    let c = l.cycle_look;
    assert_eq!((c.fill, c.glyph), (DARK.loop_region, DARK.ink_dark), "Cycle on is a role toggle in the hue");
}

#[test]
fn cycle_off_parks_the_brace_at_30_percent_and_leaves_the_ruler_and_lanes_alone() {
    let mut t = T::new();
    t.set_loop(5, 9, false);
    let l = &t.app.arrange.layout;
    let v = &l.loop_view;
    let look = brace(false);
    assert_eq!(v.brace_look, look);
    assert_eq!(look.fill, DARK.loop_region.gamma_multiply(0.3));
    assert_eq!(look.edge, Some(DARK.loop_region.gamma_multiply(0.55)));
    assert_eq!(look.ink, DARK.text);
    // Negative control: none of Cycle on's marks.
    assert_eq!(v.ruler_tint, None);
    assert!(v.edges.is_empty() && v.outside.is_empty());
    let c = l.cycle_look;
    assert_eq!((c.fill, c.border, c.glyph), (DARK.surface_raised, DARK.line, DARK.text_muted), "Cycle off: a toggle's off look");
    assert_eq!(TOKENS.loop_region, DARK.loop_region);
}

#[test]
fn the_brace_range_reads_bar_beat_and_bars_on_bar_lines_only() {
    let t = T::new();
    let s = t.app.engine.session();
    assert_eq!(range_text(s, Range::new(t.bar(5), t.bar(9))), "5.1 – 9.1 · 4 bars");
    assert_eq!(range_text(s, Range::new(t.bar(1), t.bar(2))), "1.1 – 2.1 · 1 bar");
    let beat2 = s.tempo.samples_at_bar_beat(BarBeat { bar: 2, beat: 2, tick: 0 }, s.sample_rate);
    // Negative control: off the bar lines there is no length.
    assert_eq!(range_text(s, Range::new(t.bar(1), beat2)), "1.1 – 2.2");
}

/// The brace's range keeps at least this much strip above and below its text rect (#326).
const LABEL_MARGIN: f32 = 2.0;

/// The margins (above, below) of the printed range inside the strip, from the last frame.
fn label_margins(t: &T) -> (f32, f32) {
    let l = &t.app.arrange.layout;
    let (strip, text) = (l.loop_strip, l.loop_view.text_rect);
    (text[1] - strip[1], strip[3] - text[3])
}

#[test]
fn the_loop_strip_is_14_points_and_the_label_has_margins_above_and_below() {
    let mut t = T::new();
    t.set_loop(1, 9, true);
    let l = &t.app.arrange.layout;
    assert_eq!(l.loop_strip[3] - l.loop_strip[1], crate::tokens::arrangement::LOOP_STRIP_HEIGHT);
    assert_eq!(crate::loopbar::STRIP_H, 14.0);
    assert_eq!(l.loop_view.range_text, "1.1 – 9.1 · 8 bars");
    let (above, below) = label_margins(&t);
    assert!(above >= LABEL_MARGIN && below >= LABEL_MARGIN, "label margins {above} / {below}");
    assert!((above - below).abs() < 0.5, "centered: {above} / {below}");
    // The bar numbers and ticks still fit under the strip, inside the ruler.
    assert!(l.loop_strip[3] + 20.0 <= l.timeline[1], "the ruler keeps room under the strip");
}

#[test]
fn an_8_point_strip_fails_the_margin_rule() {
    // Negative control: the old strip height leaves the same text rect under 2 points of margin.
    let mut t = T::new();
    t.set_loop(1, 9, true);
    let text = t.app.arrange.layout.loop_view.text_rect;
    let h = text[3] - text[1];
    assert!((8.0 - h) / 2.0 < LABEL_MARGIN, "a {h} point label in 8 points has {} per side", (8.0 - h) / 2.0);
    assert!((14.0 - h) / 2.0 >= LABEL_MARGIN, "and in 14 points {}", (14.0 - h) / 2.0);
}

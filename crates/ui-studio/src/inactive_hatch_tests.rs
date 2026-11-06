//! #326: an inactive track's header and lane are hatched, anchored to the window so the stripes
//! run on across the seam; its color bar goes to `ink.ghost` and its clips to ghost outlines.

use crate::theme::DARK;
use crate::{Services, StudioApp};
use serde_json::json;
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl T {
    fn new() -> T {
        let app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        T { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() }
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
            self.shapes = out.shapes;
        }
    }

    fn track(&mut self, kind: &str, name: &str) -> TrackId {
        TrackId(self.app.run("track.new", json!({"kind": kind, "name": name})).unwrap()["tracks"][0].as_u64().unwrap())
    }

    fn row(&self, t: TrackId) -> crate::arrange::TrackRow {
        self.app.arrange.layout.tracks.iter().find(|r| r.id == t.0).cloned().expect("a row")
    }
}

#[test]
fn an_inactive_track_is_hatched_across_header_and_lane_with_a_ghost_color_bar() {
    let mut t = T::new();
    let off = t.track("instrument", "Gtr DI");
    let on = t.track("instrument", "Lead");
    t.app.run("track.make_inactive", json!({"tracks": [off.0], "inactive": true})).unwrap();
    t.app.run("track.height", json!({"tracks": [off.0, on.0], "height": "medium"})).unwrap();
    t.frames(3);
    let r = t.row(off);
    let (header, lane) =
        (r.hatch_header.clone().unwrap_or_else(|| panic!("the header is hatched: {r:?}")), r.hatch_lane.clone().expect("the lane is hatched"));
    assert_eq!(header.color, DARK.text.gamma_multiply(0.07));
    assert!(header.lines.iter().any(|c| lane.lines.contains(c)), "the stripes run on across the seam");
    assert_eq!(r.color_bar, DARK.ink_ghost, "the color bar goes to ink.ghost");
    // Negative control: the active track is neither hatched nor ghosted.
    let a = t.row(on);
    assert!(a.hatch_header.is_none() && a.hatch_lane.is_none());
    assert_ne!(a.color_bar, DARK.ink_ghost);
    // Every height hatches (#326, the floor is 16): the Small default (46) and Mini (24, the
    // compact header) as well; the old 48 px floor would have refused both.
    for height in ["small", "mini"] {
        t.app.run("track.height", json!({"tracks": [off.0], "height": height})).unwrap();
        t.frames(3);
        let r = t.row(off);
        assert!(r.height < 48.0, "{height}: {}", r.height);
        assert!(r.hatch_header.is_some() && r.hatch_lane.is_some(), "{height} ({} px) is hatched", r.height);
        assert_eq!(r.color_bar, DARK.ink_ghost);
    }
}

#[test]
fn an_inactive_tracks_clips_drop_to_ghost_outlines() {
    let mut t = T::new();
    let off = t.track("instrument", "Gtr DI");
    t.app.run("track.height", json!({"tracks": [off.0], "height": "medium"})).unwrap();
    let clip = t.app.run("midi.new_clip", json!({"track": off.0, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    t.frames(3);
    let ghost = DARK.ink_ghost.gamma_multiply(crate::tokens::hatch::GHOST_FILL);
    let rect = t.app.arrange.layout.clips.iter().find(|c| c.id == clip).map(|c| c.rect).expect("the clip is drawn");
    let center = egui::pos2((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let fills = |t: &T| -> Vec<egui::Color32> {
        t.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.rect.contains(center) && r.rect.width() > 20.0 && r.rect.height() > 20.0 => Some(r.fill),
                _ => None,
            })
            .collect()
    };
    // Negative control: active, the clip is its solid body.
    assert!(!fills(&t).contains(&ghost));
    t.app.run("track.make_inactive", json!({"tracks": [off.0], "inactive": true})).unwrap();
    t.frames(3);
    assert!(fills(&t).contains(&ghost), "a ghost fill of ink.ghost: {:?}", fills(&t));
}

//! ADR 0051: the Middle C preference (decision 2), and the rig the display-site flip tests use.

use crate::StudioApp;
use serde_json::{Value, json};
use transit_midi::MiddleC::{self, C3, C4};

/// A studio with real frames, its control channel and the texts each frame paints.
pub(crate) struct Rig {
    pub app: StudioApp,
    pub ctx: egui::Context,
    pub t: f64,
}

impl Rig {
    pub fn new() -> Self {
        Self::with(crate::tests::app())
    }

    pub fn with(app: StudioApp) -> Self {
        let mut r = Rig { app, ctx: egui::Context::default(), t: 0.0 };
        r.frames(3);
        r
    }

    pub fn frames(&mut self, n: usize) {
        crate::tests::frames(&mut self.app, &self.ctx, n, &mut self.t);
    }

    /// Every text one frame paints, sorted.
    pub fn texts(&mut self) -> Vec<String> {
        shape_texts(&crate::tests::frame_shapes(&mut self.app, &self.ctx, &mut self.t))
    }

    /// A control-channel request, as an agent sends it.
    pub fn control(&mut self, method: &str, p: Value) -> Value {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        crate::control::handle_for_test(&mut self.app, &self.ctx, &req).unwrap_or(Value::Null)
    }

    pub fn click(&mut self, r: [f32; 4]) {
        let (x, y) = crate::tests::mid(r);
        self.control("ui.click", json!({"x": x, "y": y}));
        self.frames(4);
    }

    /// Settles the frame under the current setting (two equal frames), runs `options.middle_c` to
    /// `to` and draws exactly one more frame. Returns the texts that frame lost and gained.
    pub fn flip(&mut self, to: MiddleC) -> (Vec<String>, Vec<String>) {
        let (before, after) = self.flip_frames(to);
        (minus(&before, &after), minus(&after, &before))
    }

    /// [`Rig::flip`]'s two whole frames: the settled one before and the next one after.
    pub fn flip_frames(&mut self, to: MiddleC) -> (Vec<String>, Vec<String>) {
        self.frames(2);
        let before = self.texts();
        assert_eq!(before, self.texts(), "the frame is settled before the flip");
        self.control("options.middle_c", json!({"value": to.label()}));
        (before, self.texts())
    }
}

/// Every text in `shapes` (nested ones included), sorted.
pub(crate) fn shape_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|s| walk(&s.shape, &mut out));
    out.sort();
    out
}

/// The items of `a` not matched one for one in `b`.
pub(crate) fn minus(a: &[String], b: &[String]) -> Vec<String> {
    let mut rest = b.to_vec();
    let mut out = Vec::new();
    for x in a {
        match rest.iter().position(|y| y == x) {
            Some(i) => {
                rest.remove(i);
            }
            None => out.push(x.clone()),
        }
    }
    out
}

#[test]
fn the_command_sets_answers_and_refuses_and_the_engine_follows_in_the_same_step() {
    let mut app = crate::tests::app();
    assert_eq!(app.ui.middle_c, C3, "the default");
    assert_eq!(app.run("options.middle_c", json!({})).unwrap(), json!({"value": "C3"}), "without value it answers");
    let undo = crate::tests::undo_len(&mut app);
    let revision = app.engine.revision;
    assert_eq!(app.run("options.middle_c", json!({"value": "C4"})).unwrap(), json!({"value": "C4"}));
    assert_eq!(app.ui.middle_c, C4);
    assert_eq!(app.engine.message_middle_c(), C4, "set in the same step, before any frame");
    assert_eq!(crate::tests::undo_len(&mut app), undo, "not an undo step");
    assert_eq!(app.engine.revision, revision, "not a document change");
    assert!(!app.engine.is_dirty());
    for bad in [json!({"value": "C5"}), json!({"value": "c4"}), json!({"value": 60}), json!({"value": null})] {
        assert!(app.run("options.middle_c", bad.clone()).is_err(), "{bad} is refused");
    }
    assert_eq!((app.ui.middle_c, app.engine.message_middle_c()), (C4, C4), "a refusal changes nothing");
    app.run("options.middle_c", json!({"value": "C3"})).unwrap();
    assert_eq!((app.ui.middle_c, app.engine.message_middle_c()), (C3, C3));
    // Negative control: the engine alone has no adapter and refuses it.
    assert!(app.engine.execute("options.middle_c", &json!({"value": "C4"})).is_err());
    assert_eq!(app.engine.message_middle_c(), C3);
}

#[test]
fn the_control_channel_and_ui_inspect_report_it() {
    let mut r = Rig::new();
    let v = r.control("options.middle_c", json!({"value": "C4"}));
    assert_eq!(v["result"]["value"], "C4", "{v}");
    let inspect = r.app.inspect(&r.ctx);
    assert_eq!(inspect["middle_c"], "C4");
    assert_eq!(inspect["ui"]["middle_c"], "C4");
    // `ui.set` reaches the engine's message convention too.
    r.control("ui.set", json!({"middle_c": "C3"}));
    assert_eq!((r.app.ui.middle_c, r.app.engine.message_middle_c()), (C3, C3));
}

#[test]
fn the_options_menu_radio_pair_runs_the_command() {
    let mut r = Rig::new();
    let item = |r: &Rig, label: &str| r.app.arrange.bar_menu.iter().find(|m| m.label == label).map(|m| m.rect);
    let options = item(&r, "Options").unwrap();
    r.click(options);
    let c4 = item(&r, "Middle C as C4").expect("the C4 row");
    assert!(item(&r, "Middle C as C3").is_some(), "the C3 row");
    assert_eq!(r.app.arrange.bar_menu.iter().filter(|m| m.value == "options.middle_c").count(), 2, "a pair");
    r.click(c4);
    assert_eq!((r.app.ui.middle_c, r.app.engine.message_middle_c()), (C4, C4));
    r.click(options);
    let c3 = item(&r, "Middle C as C3").unwrap();
    r.click(c3);
    assert_eq!(r.app.ui.middle_c, C3);
}

#[test]
fn the_command_saves_middle_c_into_ui_json_and_keeps_the_other_keys() {
    let dir = std::env::temp_dir().join(format!("transit-middle-c-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui.json");
    std::fs::write(&path, r#"{"unrelated":42,"middle_c":"C3"}"#).unwrap();
    let mut app = crate::tests::app();
    app.autosave_dir = Some(dir.join("Autosave"));
    app.run("options.middle_c", json!({"value": "C4"})).unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v, json!({"unrelated": 42, "middle_c": "C4"}), "only the one key is written");
    // Asking writes nothing.
    std::fs::write(&path, r#"{"middle_c":"C3"}"#).unwrap();
    app.run("options.middle_c", json!({})).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"middle_c":"C3"}"#);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn a_flip_on_a_screen_without_note_names_changes_no_text() {
    // The control for every display-site flip test: the empty arrangement names no note, so a
    // flip leaves its frame exactly as it was, and flipping to the setting in force does too.
    let mut r = Rig::new();
    assert_eq!(r.flip(C4), (vec![], vec![]));
    assert_eq!(r.flip(C4), (vec![], vec![]));
    assert_eq!(minus(&["a".into(), "a".into(), "b".into()], &["a".into()]), vec!["a".to_string(), "b".to_string()]);
}

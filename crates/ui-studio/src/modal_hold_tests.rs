//! A modal holds the whole app (#770 round 2): one Escape answers the top dialog only; nothing
//! behind a dialog acts on the input of the frame it opens in (a key, text into a field that had
//! the keyboard, a click); a file dropped behind a dialog changes nothing; the export dialogs'
//! scrim is `overlay`; Recover's Escape is Later. The real app, no player, no device.

use crate::{Dialog, Services, StudioApp};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

const SCREEN: egui::Vec2 = vec2(1400.0, 800.0);

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    dropped: Vec<std::path::PathBuf>,
    /// The modifiers the next frames hold.
    mods: Modifiers,
}

impl T {
    fn new() -> T {
        let mut t = T {
            app: StudioApp::new(transit_engine::Engine::default(), None, Services::default()),
            ctx: egui::Context::default(),
            time: 0.0,
            dropped: Vec::new(),
            mods: Modifiers::NONE,
        };
        t.frames(3);
        t
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            events: std::iter::once(Event::ModifiersChanged(self.mods)).chain(events).collect(),
            dropped_files: std::mem::take(&mut self.dropped).into_iter().map(|p| std::sync::Arc::new(Dropped(p)) as egui::DroppedFileHandle).collect(),
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

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }
    }

    fn press(&mut self, key: Key) {
        self.frame(vec![T::key(key, true)]);
        self.frame(vec![T::key(key, false)]);
    }

    fn button(p: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn click(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p)]);
        self.frame(vec![T::button(p, true)]);
        self.frame(vec![T::button(p, false)]);
        self.frames(2);
    }
}

/// A file egui reports as dropped.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn mid(r: [f32; 4]) -> Pos2 {
    pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// About is up; the save question opens over it; one Escape cancels the question only.
#[test]
fn one_escape_cancels_the_save_question_and_leaves_about_open_under_it() {
    let mut t = T::new();
    t.app.about.open = true;
    t.frames(3);
    t.app.dialog = Some(Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(3);
    t.press(Key::Escape);
    t.frames(2);
    assert!(t.app.dialog.is_none(), "the question was cancelled");
    assert!(t.app.about.open, "About, under the question, stays open");
    t.press(Key::Escape);
    t.frames(2);
    assert!(!t.app.about.open, "the next Escape closes About");
}

/// The frame a dialog opens in: Space does not start the transport.
#[test]
fn a_shortcut_in_the_frame_a_dialog_opens_does_nothing_behind_it() {
    let mut t = T::new();
    t.app.about.open = true;
    t.frame(vec![T::key(Key::Space, true)]);
    t.frame(vec![T::key(Key::Space, false)]);
    t.frames(3);
    assert!(!t.app.is_playing(), "Space in About's opening frame does not start the transport");
    assert!(t.app.about.open);
}

/// The frame a dialog opens in: a click on the transport's Play button does nothing.
#[test]
fn a_click_in_the_frame_a_dialog_opens_does_nothing_behind_it() {
    let mut t = T::new();
    let play = mid(*t.app.arrange.layout.transport.get("play").expect("the Play button is laid out"));
    t.frame(vec![Event::PointerMoved(play)]);
    t.app.ui.show_shortcuts = true;
    t.frame(vec![T::button(play, true), T::button(play, false)]);
    t.frames(3);
    assert!(!t.app.is_playing(), "a click in the dialog's opening frame does not reach Play");
    assert!(t.app.shortcuts_window.open);
    // Negative control: with the dialog closed the same click plays.
    t.app.ui.show_shortcuts = false;
    t.frames(3);
    t.click(play);
    assert!(t.app.is_playing(), "the click plays with no dialog up");
}

/// The frame a dialog opens in: text does not reach the field that had the keyboard.
#[test]
fn text_in_the_frame_a_dialog_opens_does_not_reach_the_field_behind_it() {
    let mut t = T::new();
    t.app.ui.browser.show = true;
    t.frames(3);
    let filter = mid(t.app.browser.layout.filter);
    t.click(filter);
    t.frame(vec![Event::Text("a".into())]);
    t.frames(1);
    assert_eq!(t.app.browser.filter, "a", "the browser's filter has the keyboard");
    t.app.about.open = true;
    t.frame(vec![Event::Text("b".into())]);
    t.frames(3);
    assert_eq!(t.app.browser.filter, "a", "text in About's opening frame does not reach the filter");
    t.frame(vec![Event::Text("c".into())]);
    t.frames(1);
    assert_eq!(t.app.browser.filter, "a", "nor any frame after it");
}

/// A file dropped behind a dialog opens nothing and asks nothing.
#[test]
fn a_file_dropped_behind_a_dialog_changes_nothing() {
    let mut t = T::new();
    t.app.run("track.new", json!({})).unwrap();
    t.frames(2);
    t.app.about.open = true;
    t.frames(3);
    let at = pos2(700.0, 400.0);
    t.dropped = vec![std::path::PathBuf::from("dropped behind a dialog.transit")];
    t.frame(vec![Event::PointerMoved(at)]);
    t.frames(3);
    assert!(t.app.dialog.is_none(), "no save question: the dropped session is not opened");
    assert_eq!(t.app.engine.session().tracks.len(), 1, "the session is the same");
    // Negative control: with About closed the same drop asks to save first.
    t.app.about.open = false;
    t.frames(3);
    t.dropped = vec![std::path::PathBuf::from("dropped behind a dialog.transit")];
    t.frame(vec![Event::PointerMoved(at)]);
    t.frames(2);
    assert!(matches!(t.app.dialog, Some(Dialog::SaveChanges { .. })), "the drop opens a session when no dialog is up");
}

/// Recover unsaved work: Escape is Later.
#[test]
fn escape_is_later_in_recover_unsaved_work() {
    let mut t = T::new();
    t.app.leftovers = vec![transit_engine::autosave::Leftover {
        dir: std::path::PathBuf::from("no such autosave folder"),
        name: "Left behind".into(),
        origin: None,
        saved: std::time::SystemTime::UNIX_EPOCH,
        recording: false,
        midi_journal: None,
        midi_placed: false,
    }];
    t.frames(3);
    assert!(!t.app.leftovers.is_empty());
    t.press(Key::Escape);
    t.frames(2);
    assert!(t.app.leftovers.is_empty(), "Escape is Later: the offer goes for this run");
}

/// Show Hidden Tracks (a plain egui modal) dims the app with `overlay`, as every dialog does.
#[test]
fn the_raw_modals_dim_with_the_overlay_token() {
    let mut t = T::new();
    let tr = t.app.run("track.new", json!({})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.run("track.hide", json!({"tracks": [tr], "hidden": true})).unwrap();
    let fill = std::sync::Arc::new(std::sync::Mutex::new(None));
    t.ctx.add_plugin(BackdropFill(fill.clone(), crate::modal::layer_of(egui::Id::new("hidden_tracks"))));
    t.app.dialog = Some(Dialog::HiddenTracks);
    // Past egui's fade-in, which scales the backdrop's alpha.
    t.frames(30);
    assert_eq!(*fill.lock().unwrap(), Some(crate::theme::TOKENS.overlay));
}

/// The fill of the screen-sized rect a layer painted, read at the end of each pass.
struct BackdropFill(std::sync::Arc<std::sync::Mutex<Option<egui::Color32>>>, egui::LayerId);

impl egui::Plugin for BackdropFill {
    fn debug_name(&self) -> &'static str {
        "backdrop_fill"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        let fill = ui.ctx().graphics(|g| {
            g.get(self.1).and_then(|l| {
                l.all_entries().find_map(|c| match &c.shape {
                    egui::Shape::Rect(s) if s.rect.contains_rect(screen) => Some(s.fill),
                    _ => None,
                })
            })
        });
        *self.0.lock().unwrap() = fill;
    }
}

/// Every `egui::Modal` goes through `chrome::raw_modal` (its scrim is `overlay`, its frame is
/// marked modal): no source file of the crate calls the plain constructor but chrome.rs.
#[test]
fn no_dialog_builds_an_egui_modal_by_hand() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name.contains("tests") || name == "chrome.rs" {
                continue;
            }
            let code = std::fs::read_to_string(&path).unwrap();
            if code.contains(concat!("egui::Modal", "::new(")) {
                found.push(path.display().to_string());
            }
        }
    }
    assert!(found.is_empty(), "build modals with chrome::raw_modal: {found:?}");
}

/// The keyboard handlers behind a dialog wait as a group (#770 round 2): a paste (egui's Paste
/// event, read directly, not through the shortcut table) does nothing behind About.
#[test]
fn a_paste_behind_a_dialog_does_nothing() {
    let mut t = T::new();
    let tr = t.app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap();
    let c = t.app.run("midi.new_clip", json!({"tracks": [tr], "start": 0, "end": 3840})).unwrap()["clip"].as_u64().unwrap();
    t.app.run("edit.select", json!({"clips": [c]})).unwrap();
    t.frames(2);
    t.app.copy_selected(false).unwrap();
    // Paste lands at the cursor: put it well past the clip, so a paste adds a clip of its own.
    t.app.set_cursor(480_000, None);
    let clips = |t: &T| t.app.engine.session().tracks.iter().map(|tr| tr.clips().len()).sum::<usize>();
    let before = clips(&t);
    let paste = |t: &mut T| {
        t.mods = Modifiers::COMMAND;
        t.frame(vec![Event::Paste(String::new())]);
        t.mods = Modifiers::NONE;
        t.frames(2);
    };
    t.app.about.open = true;
    t.frames(3);
    paste(&mut t);
    assert_eq!(clips(&t), before, "a paste behind About adds no clip");
    // Negative control: the same paste with About closed adds one.
    t.app.about.open = false;
    t.frames(3);
    paste(&mut t);
    assert_eq!(clips(&t), before + 1, "the paste works with no dialog up");
}

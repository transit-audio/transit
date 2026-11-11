//! The standard dialogs in the application chrome (#326): the save-changes question and the delete
//! confirmations are the chrome's modal (380 wide, the question as the title, the default
//! rightmost and named for what it does). Layout only: their keys and outcomes are the dialogs'
//! own tests (groups, takes, menus).

use crate::{Dialog, Services, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    tracks: Vec<TrackId>,
}

impl T {
    fn new() -> T {
        T::with(Services::default())
    }

    fn with(services: Services) -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, services);
        let tracks = ["A", "B", "C"]
            .iter()
            .map(|name| TrackId(app.run("track.new", json!({"kind": "audio", "name": name})).unwrap()["tracks"][0].as_u64().unwrap()))
            .collect();
        app.engine.mark_clean();
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, tracks };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
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

    fn modal(&self, id: &str) -> crate::chrome::ModalLayout {
        crate::chrome::modal_layout(&self.ctx, egui::Id::new(id)).unwrap_or_else(|| panic!("{id} was drawn"))
    }
}

fn texts(l: &crate::chrome::ModalLayout) -> Vec<&str> {
    l.buttons.iter().map(|b| b.text.as_str()).collect()
}

#[test]
fn the_delete_question_names_the_loss_on_its_default_button_rightmost() {
    let mut t = T::new();
    t.app.dialog = Some(Dialog::DeleteTracks { tracks: t.tracks.clone(), clips: true });
    t.frames(3);
    let l = t.modal("delete_tracks");
    assert_eq!(texts(&l), ["Cancel", "Delete 3 Tracks"]);
    assert_eq!(l.detail_text.as_deref(), Some("Undo brings them back."), "the artboard's detail line");
    assert!(l.buttons[1].default && !l.buttons[0].default, "the destructive action takes the default style: Enter performs it");
    assert!(l.buttons[1].rect[0] >= l.buttons[0].rect[2], "rightmost");
    assert_eq!(l.rect[2] - l.rect[0], crate::tokens::chrome::MODAL_WIDTH);
    assert_eq!(l.fill, crate::theme::DARK.surface_panel);
    // Negative control: no button says OK or a bare Delete.
    assert!(l.buttons.iter().all(|b| b.text != "OK" && b.text != "Delete"));
}

#[test]
fn every_delete_label_says_what_goes() {
    let mut t = T::new();
    let s = t.app.engine.session();
    let (_, one) = crate::menus::delete_tracks_text(s, &t.tracks[..1], true);
    let (_, empty) = crate::menus::delete_tracks_text(s, &t.tracks[..1], false);
    let (_, two) = crate::menus::delete_tracks_text(s, &t.tracks[..2], false);
    assert_eq!((one.as_str(), empty.as_str(), two.as_str()), ("Delete Track", "Delete Track", "Delete 2 Tracks"));
    let g = t.app.new_group_track().expect("a group");
    let (q, label) = crate::menus::delete_tracks_text(t.app.engine.session(), &[g], false);
    assert_eq!(label, "Delete Group", "{q}");
    t.app.dialog = Some(Dialog::DeleteTake { track: t.tracks[0], take: 2 });
    t.frames(3);
    let l = t.modal("delete_take");
    assert_eq!(texts(&l), ["Cancel", crate::menus::DELETE_TAKE_LABEL]);
    assert_eq!(l.detail_text.as_deref(), Some("Undo brings it back."));
    // Negative control: the save question's detail is its own.
    t.app.dialog = Some(Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(3);
    assert_eq!(t.modal("save_changes").detail_text.as_deref(), Some("Your changes are lost if you don't save them."));
    assert_eq!(crate::menus::DELETE_TAKE_LABEL, "Delete Take");
}

#[test]
fn the_save_question_puts_dont_save_alone_left_and_save_rightmost_and_cancel_still_cancels() {
    let mut t = T::new();
    t.app.dialog = Some(Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(3);
    let l = t.modal("save_changes");
    assert_eq!(texts(&l), ["Don't Save", "Cancel", "Save"]);
    assert!(l.buttons[2].default, "Save is the default");
    assert!(l.buttons[0].rect[2] + 20.0 < l.buttons[1].rect[0], "Don't Save stands apart at the far left");
    assert!(l.detail.is_some(), "the detail line under the question");
    // Cancel closes the question and leaves the session (the behavior is the dialog's own).
    let c = &l.buttons[1].rect;
    t.input("ui.click", json!({"x": (c[0] + c[2]) / 2.0, "y": (c[1] + c[3]) / 2.0}));
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().tracks.len(), 3, "no new session");
}

/// #326, Enter and Escape in the chrome's modal (Live and Logic agree): Enter is the default
/// button, Escape is Cancel, in every dialog that uses it.
#[test]
fn enter_deletes_the_tracks_and_escape_keeps_them() {
    let mut t = T::new();
    t.app.dialog = Some(Dialog::DeleteTracks { tracks: t.tracks[..2].to_vec(), clips: false });
    t.frames(2);
    t.input("ui.key", json!({"key": "Escape"}));
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().tracks.len(), 3, "Escape deletes nothing");
    t.app.dialog = Some(Dialog::DeleteTracks { tracks: t.tracks[..2].to_vec(), clips: false });
    t.frames(2);
    t.input("ui.key", json!({"key": "Enter"}));
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().tracks.len(), 1, "Enter performs Delete 2 Tracks");
}

#[test]
fn enter_runs_delete_take_and_escape_does_not() {
    let mut t = T::new();
    let status = t.app.status.clone();
    t.app.dialog = Some(Dialog::DeleteTake { track: t.tracks[0], take: 2 });
    t.frames(2);
    t.input("ui.key", json!({"key": "Escape"}));
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.status, status, "Escape runs nothing");
    t.app.dialog = Some(Dialog::DeleteTake { track: t.tracks[0], take: 2 });
    t.frames(2);
    t.input("ui.key", json!({"key": "Enter"}));
    assert!(t.app.dialog.is_none());
    // The track has no take 2: the command ran and said so, which is what Enter is for here.
    assert!(!t.app.status.is_empty() && t.app.status != status, "Enter ran takes.delete: {}", t.app.status);
}

#[test]
fn enter_saves_then_continues_and_escape_cancels() {
    let path = std::env::temp_dir().join(format!("transit-326c-enter-{}.transit", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let target = path.to_string_lossy().to_string();
    let services = Services { pick_save: Some(Box::new(move |_| Some(target.clone()))), ..Services::default() };
    let mut t = T::with(services);
    t.app.dialog = Some(Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(2);
    t.input("ui.key", json!({"key": "Escape"}));
    assert!(t.app.dialog.is_none());
    assert!(!path.exists(), "Escape saves nothing");
    assert_eq!(t.app.engine.session().tracks.len(), 3, "and starts no new session");
    t.app.dialog = Some(Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(2);
    t.input("ui.key", json!({"key": "Enter"}));
    assert!(t.app.dialog.is_none());
    assert!(path.exists(), "Enter is Save");
    assert!(t.app.engine.session().tracks.is_empty(), "then the new session it was asked for");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dawproject_export_discloses_the_fallback_then_enter_chooses_a_location() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("transit-dawproject-dialog-{}-{stamp}.dawproject", std::process::id()));
    let answer = path.to_string_lossy().into_owned();
    let services = Services { pick_save: Some(Box::new(move |_| Some(answer.clone()))), ..Services::default() };
    let mut t = T::with(services);
    t.app.dialog = Some(Dialog::DawprojectExport);
    t.frames(3);
    let layout = t.modal("dawproject_export");
    let detail = layout.detail_text.as_deref().unwrap_or_default();
    assert!(detail.contains("full-length WAV stems") && detail.contains("MIDI clips remain editable"), "{detail}");
    assert!(detail.contains("External Instrument audio is left out"), "{detail}");
    assert_eq!(texts(&layout), ["Cancel", "Choose Location…"]);
    t.input("ui.key", json!({"key": "Escape"}));
    assert!(t.app.dialog.is_none() && !path.exists(), "Escape exports nothing");
    t.app.dialog = Some(Dialog::DawprojectExport);
    t.frames(2);
    t.input("ui.key", json!({"key": "Enter"}));
    assert!(t.app.dialog.is_none() && path.is_file(), "Enter chooses the location and exports");
    assert!(t.app.status.starts_with("Exported DAWproject to "), "{}", t.app.status);
    let _ = std::fs::remove_file(&path);
}

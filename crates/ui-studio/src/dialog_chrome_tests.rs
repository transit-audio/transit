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
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
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

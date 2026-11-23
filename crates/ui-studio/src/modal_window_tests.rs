//! A `theme::modal_window` dialog holds the app (#770; the design language's modal rule): it is
//! centered and fixed, its `overlay` scrim dims every panel and window behind it, nothing behind
//! it takes the pointer or the keyboard, Tab stays inside it, the app's shortcuts and the
//! transport wait, and Escape closes it.

use crate::shortcuts::{self, Action};
use crate::{Services, StudioApp, theme};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

const SCREEN: egui::Vec2 = vec2(1200.0, 800.0);
const DIALOG: &str = "Test dialog";
const BEHIND: &str = "Behind";

/// What the frames saw behind and in the dialog.
struct Seen {
    panel_clicks: usize,
    behind_clicks: usize,
    dialog_clicks: usize,
    shortcut: bool,
    panel_button: Rect,
    behind_button: Rect,
    dialog_buttons: Vec<egui::Id>,
    panel_ids: Vec<egui::Id>,
    dialog_rect: Option<Rect>,
}

/// A bare egui rig: a panel with a field and a button, a tool window with a button, and the dialog
/// while `open` (closable through the flag, as Quantize and Hardware Insert are).
struct Rig {
    ctx: egui::Context,
    time: f64,
    open: bool,
    text: String,
    seen: Seen,
}

impl Rig {
    fn new(open: bool) -> Rig {
        let ctx = egui::Context::default();
        theme::apply_pinned(&ctx, egui::Theme::Dark);
        let seen = Seen {
            panel_clicks: 0,
            behind_clicks: 0,
            dialog_clicks: 0,
            shortcut: false,
            panel_button: Rect::NOTHING,
            behind_button: Rect::NOTHING,
            dialog_buttons: Vec::new(),
            panel_ids: Vec::new(),
            dialog_rect: None,
        };
        let mut r = Rig { ctx, time: 0.0, open, text: String::new(), seen };
        r.frames(3);
        r
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(self.time), events, ..Default::default() };
        let (open, text, seen) = (&mut self.open, &mut self.text, &mut self.seen);
        seen.panel_ids.clear();
        seen.dialog_buttons.clear();
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| {
                let f = ui.text_edit_singleline(text);
                let b = ui.button("Panel button");
                seen.panel_ids.extend([f.id, b.id]);
                seen.panel_button = b.rect;
                seen.panel_clicks += usize::from(b.clicked());
            });
            theme::window(&ctx, BEHIND).default_pos(pos2(40.0, 600.0)).show(&ctx, |ui| {
                let b = ui.button("Behind button");
                seen.panel_ids.push(b.id);
                seen.behind_button = b.rect;
                seen.behind_clicks += usize::from(b.clicked());
            });
            seen.dialog_rect = None;
            if *open {
                let r = theme::modal_window(&ctx, DIALOG).open(open).collapsible(false).resizable(false).show(&ctx, |ui| {
                    ui.label("A question");
                    for name in ["First", "Second"] {
                        let b = ui.button(name);
                        seen.dialog_buttons.push(b.id);
                        seen.dialog_clicks += usize::from(b.clicked());
                    }
                });
                seen.dialog_rect = r.map(|r| r.response.rect);
            }
            seen.shortcut = shortcuts::pressed(&ctx, Action::PlayStop);
        });
        out.textures_delta.clear();
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(Vec::new());
        }
    }

    fn click(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p)]);
        let button = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame(vec![button(true)]);
        self.frame(vec![button(false)]);
        self.frames(2);
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let ev = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frame(vec![ev(true)]);
        self.frame(vec![ev(false)]);
    }

    fn focused(&self) -> Option<egui::Id> {
        self.ctx.memory(|m| m.focused())
    }

    fn layer_index(&self, layer: egui::LayerId) -> Option<usize> {
        self.ctx.memory(|m| m.layer_ids().position(|l| l == layer))
    }
}

type Fill = std::sync::Arc<std::sync::Mutex<Option<egui::Color32>>>;

/// The fill of a rect on the scrim's layer that covers the whole screen, read at the end of each
/// pass (the paint lists are gone once the frame's output is taken).
struct ScrimFill(Fill);

impl egui::Plugin for ScrimFill {
    fn debug_name(&self) -> &'static str {
        "scrim_fill"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        let fill = ui.ctx().graphics(|g| {
            g.get(theme::modal_scrim_layer(DIALOG)).and_then(|l| {
                l.all_entries().find_map(|c| match &c.shape {
                    egui::Shape::Rect(s) if s.rect.contains_rect(screen) => Some(s.fill),
                    _ => None,
                })
            })
        });
        *self.0.lock().unwrap() = fill;
    }
}

fn dialog_layer() -> egui::LayerId {
    crate::layout_gate::modal_window(DIALOG)
}

#[test]
fn a_window_behind_a_modal_takes_no_click_and_is_dimmed() {
    let mut r = Rig::new(false);
    let scrim_fill = Fill::default();
    r.ctx.add_plugin(ScrimFill(scrim_fill.clone()));
    let behind = r.seen.behind_button.center();
    r.click(behind);
    assert_eq!(r.seen.behind_clicks, 1, "the rig's tool window takes a click with no dialog up");
    r.open = true;
    r.frames(3);
    let dialog = r.seen.dialog_rect.expect("the dialog is drawn");
    assert!(!dialog.contains(behind), "the tool window's button is outside the dialog");
    r.click(behind);
    assert_eq!(r.seen.behind_clicks, 1, "a window behind the dialog takes no click");
    r.click(r.seen.panel_button.center());
    assert_eq!(r.seen.panel_clicks, 0, "the panel behind takes no click");
    // The scrim lies over the tool window and under the dialog, and paints `overlay` over the
    // whole content.
    let (scrim, tool) = (r.layer_index(theme::modal_scrim_layer(DIALOG)), r.layer_index(crate::layout_gate::window(BEHIND)));
    let order = r.ctx.memory(|m| m.layer_ids().collect::<Vec<_>>());
    let (scrim, tool, dialog_at) =
        (scrim.expect("the scrim is drawn"), tool.expect("the tool window is drawn"), r.layer_index(dialog_layer()).unwrap());
    assert!(tool < scrim && scrim < dialog_at, "tool window < scrim < dialog: {order:?}");
    let fill = *scrim_fill.lock().unwrap();
    assert_eq!(fill, Some(theme::TOKENS.overlay), "the scrim paints overlay over the whole content");
}

#[test]
fn the_dialog_still_takes_its_own_clicks() {
    let mut r = Rig::new(true);
    let dialog = r.seen.dialog_rect.expect("drawn");
    let id = r.seen.dialog_buttons[0];
    let rect = r.ctx.read_response(id).expect("the First button").rect;
    assert!(dialog.contains_rect(rect));
    r.click(rect.center());
    assert_eq!(r.seen.dialog_clicks, 1, "the dialog's own button works");
}

#[test]
fn a_modal_holds_the_shortcuts_and_lets_dialog_keys_through() {
    let mut r = Rig::new(false);
    r.frame(vec![Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(r.seen.shortcut, "Space is Play/Stop with no dialog up");
    r.frame(Vec::new());
    r.open = true;
    r.frames(3);
    r.frame(vec![Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(!r.seen.shortcut, "Space does not reach the transport behind a dialog");
}

#[test]
fn tab_stays_inside_the_modal() {
    let mut r = Rig::new(true);
    let mut visited = Vec::new();
    for _ in 0..6 {
        r.key(Key::Tab, Modifiers::NONE);
        visited.push(r.focused());
    }
    for f in &visited {
        let f = f.expect("Tab focuses something");
        assert!(!r.seen.panel_ids.contains(&f), "Tab reached a widget behind the dialog: {visited:?}");
    }
    assert!(visited.iter().any(|f| f.is_some_and(|f| r.seen.dialog_buttons.contains(&f))), "Tab cycles the dialog's buttons: {visited:?}");
}

#[test]
fn a_field_behind_gives_the_keyboard_up_when_a_modal_opens() {
    let mut r = Rig::new(false);
    let field = r.seen.panel_ids[0];
    r.ctx.memory_mut(|m| m.request_focus(field));
    r.frames(2);
    assert_eq!(r.focused(), Some(field), "the field has the keyboard");
    r.open = true;
    r.frames(3);
    r.frame(vec![Event::Text("x".into())]);
    r.frames(1);
    assert_eq!(r.text, "", "typing does not reach a field behind the dialog");
    assert_ne!(r.focused(), Some(field), "the field gave its focus up");
}

#[test]
fn escape_closes_a_closable_modal() {
    let mut r = Rig::new(true);
    r.key(Key::Escape, Modifiers::NONE);
    assert!(!r.open, "Escape is Cancel");
}

#[test]
fn a_modal_opens_centered_and_a_title_drag_does_not_move_it() {
    let mut r = Rig::new(true);
    r.frames(2);
    let dialog = r.seen.dialog_rect.expect("drawn");
    let center = Rect::from_min_size(Pos2::ZERO, SCREEN).center();
    assert!((dialog.center() - center).length() <= 1.0, "centered: {dialog:?} around {center:?}");
    let grab = pos2(dialog.center().x, dialog.top() + 8.0);
    let to = grab + vec2(-200.0, 150.0);
    let press = |p, pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    r.frame(vec![Event::PointerMoved(grab)]);
    r.frame(vec![press(grab, true)]);
    for i in 1..=10 {
        r.frame(vec![Event::PointerMoved(grab + (to - grab) * (i as f32 / 10.0))]);
    }
    r.frame(vec![press(to, false)]);
    r.frames(2);
    let after = r.seen.dialog_rect.expect("drawn");
    assert!((after.center() - center).length() <= 1.0, "fixed: a title-bar drag left it at {after:?}");
}

/// The real app: Help > Keyboard Shortcuts is a modal window; Space behind it does not start the
/// transport.
#[test]
fn the_transport_waits_behind_the_keyboard_shortcuts_window() {
    let ctx = egui::Context::default();
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    let mut time = 0.0;
    let mut frame = |app: &mut StudioApp, events: Vec<Event>| {
        time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    };
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    app.ui.show_shortcuts = true;
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    assert!(app.shortcuts_window.open, "the window is up");
    let space = |pressed: bool| Event::Key { key: Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    frame(&mut app, vec![space(true)]);
    frame(&mut app, vec![space(false)]);
    for _ in 0..3 {
        frame(&mut app, Vec::new());
    }
    assert!(!app.is_playing(), "Space behind the dialog does not start the transport");
    assert!(app.shortcuts_window.open, "Space does not close the window either");
}

/// Escape closes the dialog when the keyboard is on one of its buttons (only a text field or an
/// open list holds Escape back).
#[test]
fn escape_closes_the_modal_with_a_button_focused() {
    let mut r = Rig::new(true);
    let button = r.seen.dialog_buttons[0];
    r.ctx.memory_mut(|m| m.request_focus(button));
    r.frames(2);
    assert_eq!(r.focused(), Some(button), "a dialog button has the keyboard");
    r.key(Key::Escape, Modifiers::NONE);
    assert!(!r.open, "Escape closes the dialog with a button focused");
}

/// The piano roll's edge readers (`edges`, `step_control_edges`) give nothing while a modal is up.
#[test]
fn the_edge_readers_give_nothing_under_a_modal() {
    let r = Rig::new(true);
    let space = |pressed: bool| Event::Key { key: Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let enter = |pressed: bool| Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let n = |pressed: bool| Event::Key { key: Key::N, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let mut seen = (Vec::new(), Vec::new(), Vec::new());
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        time: Some(r.time + 1.0 / 60.0),
        events: vec![n(true), enter(true), space(true)],
        ..Default::default()
    };
    let _ = r.ctx.run_ui(raw, |ui| {
        let ctx = ui.ctx().clone();
        seen.0 = shortcuts::edges(&ctx, Action::NotesFocus);
        seen.1 = shortcuts::step_control_edges(&ctx, Action::StepTieRest, false);
        seen.2 = shortcuts::step_control_edges(&ctx, Action::StepTieRest, true);
        let mut open = true;
        let _ = theme::modal_window(&ctx, DIALOG).open(&mut open).show(&ctx, |ui| ui.label("A question"));
    });
    assert_eq!(seen, (Vec::new(), Vec::new(), Vec::new()), "N, Enter and Space give no edge under a modal");
}

const UPPER: &str = "Upper dialog";

/// Two dialogs at once.
struct Two {
    ctx: egui::Context,
    time: f64,
    lower_open: bool,
    upper_open: bool,
    upper_on: bool,
    lower_clicks: usize,
    lower_button: Rect,
}

impl Two {
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(self.time), events, ..Default::default() };
        let (lower_open, upper_open, upper_on) = (&mut self.lower_open, &mut self.upper_open, self.upper_on);
        let (clicks, button) = (&mut self.lower_clicks, &mut self.lower_button);
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            if *lower_open {
                let _ = theme::modal_window(&ctx, DIALOG).open(lower_open).show(&ctx, |ui| {
                    ui.add_space(120.0);
                    let b = ui.button("Lower button, wide enough to reach past the upper dialog");
                    *button = b.rect;
                    *clicks += usize::from(b.clicked());
                });
            }
            if upper_on && *upper_open {
                let _ = theme::modal_window(&ctx, UPPER).open(upper_open).show(&ctx, |ui| ui.label("Small"));
            }
        });
        out.textures_delta.clear();
    }
}

/// The upper dialog, opened later, covers the lower one with its own scrim; the lower one takes no
/// click and no Escape while the upper one is up.
#[test]
fn a_dialog_over_another_covers_it_and_owns_the_keys() {
    let ctx = egui::Context::default();
    theme::apply_pinned(&ctx, egui::Theme::Dark);
    let mut t = Two { ctx, time: 0.0, lower_open: true, upper_open: true, upper_on: false, lower_clicks: 0, lower_button: Rect::NOTHING };
    for _ in 0..3 {
        t.frame(Vec::new());
    }
    t.upper_on = true;
    for _ in 0..3 {
        t.frame(Vec::new());
    }
    let at = |l: egui::LayerId| t.ctx.memory(|m| m.layer_ids().position(|x| x == l)).expect("drawn");
    let (lower, scrim, upper) = (at(theme::modal_window_layer(DIALOG)), at(theme::modal_scrim_layer(UPPER)), at(theme::modal_window_layer(UPPER)));
    assert!(lower < scrim && scrim < upper, "lower dialog < upper scrim < upper dialog");
    let press = |p, pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let p = t.lower_button.left_center() + vec2(4.0, 0.0);
    t.frame(vec![Event::PointerMoved(p)]);
    t.frame(vec![press(p, true)]);
    t.frame(vec![press(p, false)]);
    assert_eq!(t.lower_clicks, 0, "the covered dialog takes no click");
    let esc = |pressed| Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    t.frame(vec![esc(true)]);
    t.frame(vec![esc(false)]);
    assert!(!t.upper_open && t.lower_open, "one Escape closes the upper dialog only");
}

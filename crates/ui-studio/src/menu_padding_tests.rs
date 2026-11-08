//! Menu rows as the chrome's Menus artboard draws them (#386): inside the popup's 1 px edge the
//! 4 px padding, then the row's 4 px inset, the 16 px check gutter on every row and a 4 px gap
//! before the text (28 pt in all); the shortcut ends 4 + 8 = 12 pt inside the edge.
use crate::shortcuts::{Action as A, label};
use crate::tokens::chrome as c;
use crate::{Services, StudioApp};
use egui::{Rect, Response, Ui};
use serde_json::{Value, json};
use transit_model::TrackId;

/// Text start inside the edge's inner side: padding, row inset, gutter, gap.
pub(crate) const TEXT_IN: f32 = c::POPUP_PADDING + c::ROW_PADDING[0] + c::CHECK_GUTTER + c::ROW_GAP;
/// Shortcut end inside the edge's inner side: padding and the row's right inset.
pub(crate) const SHORTCUT_IN: f32 = c::POPUP_PADDING + c::ROW_PADDING[1];
/// The popup's edge (`window_stroke`, 1 px `line.strong`).
pub(crate) const EDGE: f32 = 1.0;

pub(crate) type Shapes = Vec<egui::epaint::ClippedShape>;

/// Text shapes with exactly `text` in a frame's output.
pub(crate) fn texts(shapes: &Shapes, text: &str) -> Vec<Rect> {
    shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(s) if s.galley.text() == text => Some(s.galley.rect.translate(s.pos.to_vec2())),
            _ => None,
        })
        .collect()
}

/// The popup area holding `p` (its frame's outer rect, edge included).
pub(crate) fn popup_at(ctx: &egui::Context, p: egui::Pos2) -> Rect {
    let layer = ctx.layer_id_at(p).expect("a layer under the text");
    assert_eq!(layer.order, egui::Order::Foreground, "the text is in a popup");
    ctx.memory(|m| m.area_rect(layer.id)).expect("the popup's area")
}

/// (text start, shortcut end) inside the edge's inner side.
pub(crate) fn insets(frame: Rect, text: Rect, shortcut: Rect) -> (f32, f32) {
    (text.min.x - (frame.min.x + EDGE), (frame.max.x - EDGE) - shortcut.max.x)
}

pub(crate) fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

// --- The kit: rows in a menu-styled popup, with controls ---------------------------------------

struct Kit {
    frame: Rect,
    rows: Vec<Rect>,
    shapes: Shapes,
}

/// Opens a popup in the menu bar's row style (`chrome::menu_style`) holding `rows`; `margin`
/// overrides the theme's popup padding.
fn kit(margin: Option<i8>, rows: &dyn Fn(&mut Ui) -> Vec<Response>) -> Kit {
    let ctx = egui::Context::default();
    crate::theme::apply_pinned(&ctx, egui::Theme::Dark);
    if let Some(m) = margin {
        ctx.style_mut_of(egui::Theme::Dark, |st| st.spacing.menu_margin = egui::Margin::same(m));
    }
    let (mut rects, mut shapes) = (Vec::new(), Vec::new());
    for i in 0..3 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            time: Some(f64::from(i) / 60.0),
            ..Default::default()
        };
        let mut o = ctx.run_ui(raw, |ui| {
            let anchor = ui.button("Edit");
            egui::Popup::menu(&anchor).open(true).style(crate::chrome::menu_style).show(|ui| {
                rects = rows(ui).iter().map(|r| r.rect).collect();
            });
        });
        o.textures_delta.clear();
        shapes = o.shapes;
    }
    Kit { frame: popup_at(&ctx, rects[0].center()), rows: rects, shapes }
}

fn chrome_rows(ui: &mut Ui) -> Vec<Response> {
    vec![
        ui.add(crate::chrome::MenuRow::new("Undo").shortcut("Ctrl+Z")),
        ui.add(crate::chrome::MenuRow::new("Browser").checked(true).shortcut("Ctrl+Alt+B")),
        ui.add(crate::chrome::MenuRow::new("Count-in").checked(false)),
    ]
}

/// The rows before #386: egui buttons with a shortcut and no gutter.
fn plain_rows(ui: &mut Ui) -> Vec<Response> {
    vec![
        ui.add(egui::Button::new("Undo").shortcut_text(crate::chrome::shortcut(ui.ctx(), "Ctrl+Z"))),
        ui.add(egui::Button::new("Browser").shortcut_text(crate::chrome::shortcut(ui.ctx(), "Ctrl+Alt+B"))),
        ui.add(egui::Button::new("Count-in")),
    ]
}

#[test]
fn a_menu_row_has_the_gutter_and_insets_and_check_and_plain_rows_share_the_text_x() {
    let k = kit(None, &chrome_rows);
    let (undo, z) = (texts(&k.shapes, "Undo")[0], texts(&k.shapes, "Ctrl+Z")[0]);
    let (text_in, shortcut_in) = insets(k.frame, undo, z);
    assert!(near(text_in, TEXT_IN) && near(shortcut_in, SHORTCUT_IN), "text {text_in}, shortcut {shortcut_in}: want 28 and 12");
    for t in ["Browser", "Count-in"] {
        assert!(near(texts(&k.shapes, t)[0].min.x, undo.min.x), "{t} starts where Undo does");
    }
    assert!(near(texts(&k.shapes, "Ctrl+Alt+B")[0].max.x, z.max.x), "the shortcuts end together");
    for r in &k.rows {
        assert_eq!(r.height(), c::ROW_HEIGHT, "24 high");
        assert!(near(r.min.x - k.frame.min.x, EDGE + c::POPUP_PADDING), "the row sits 4 inside the edge: {r:?} in {:?}", k.frame);
        assert!(near(k.frame.max.x - r.max.x, EDGE + c::POPUP_PADDING), "on both sides");
    }
    // Negative control: the rows before #386 (no gutter, a 4 px right inset) read 8 and 8.
    let p = kit(None, &plain_rows);
    let (text_in, shortcut_in) = insets(p.frame, texts(&p.shapes, "Undo")[0], texts(&p.shapes, "Ctrl+Z")[0]);
    assert!(near(text_in, 8.0) && near(shortcut_in, 8.0), "control measured {text_in}, {shortcut_in}");
    assert!(!near(text_in, TEXT_IN) && !near(shortcut_in, SHORTCUT_IN));
    // Negative control: a popup with no padding puts the rows on the edge.
    let flush = kit(Some(0), &chrome_rows);
    assert!(!near(flush.rows[0].min.x - flush.frame.min.x, EDGE + c::POPUP_PADDING), "margin 0 fails the row check");
    let (text_in, _) = insets(flush.frame, texts(&flush.shapes, "Undo")[0], texts(&flush.shapes, "Ctrl+Z")[0]);
    assert!(!near(text_in, TEXT_IN));
}

#[test]
fn a_checked_row_paints_the_check_centered_in_its_gutter() {
    let k = kit(None, &chrome_rows);
    let marks: Vec<Rect> = k
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(m) => Some(m.calc_bounds()),
            _ => None,
        })
        .filter(|b| b.width() > 0.0 && b.height() <= c::CHECK_GLYPH + 0.5)
        .collect();
    let in_row = |r: Rect| marks.iter().filter(|b| r.contains(b.center())).count();
    let browser = k.rows[1];
    let gutter_x = browser.min.x + c::ROW_PADDING[0] + c::CHECK_GUTTER / 2.0;
    assert!(
        marks.iter().any(|b| browser.contains(b.center()) && near(b.center().x, gutter_x) && near(b.center().y, browser.center().y)),
        "a check centered in the gutter at x {gutter_x}: {marks:?}"
    );
    assert_eq!(in_row(k.rows[0]), 0, "a plain row's gutter stays empty");
    assert_eq!(in_row(k.rows[2]), 0, "an unchecked row's gutter stays empty");
}

// --- The app: the menu bar's menus and a track header's context menu ---------------------------

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    shapes: Shapes,
}

impl T {
    fn new() -> Self {
        let app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let mut t = Self { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() };
        t.frames();
        t
    }
    fn frames(&mut self) {
        for _ in 0..5 {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let mut output = self.ctx.run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }
    }
    fn input(&mut self, method: &str, params: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
        for _ in 0..6 {
            self.frames();
        }
    }
    fn click(&mut self, x: f32, y: f32, button: &str) {
        self.input("ui.click", json!({"x": x, "y": y, "button": button}));
    }
    fn menu(&mut self, name: &str) {
        let r = self.app.arrange.bar_menu.iter().find(|m| m.label == name).unwrap().rect;
        self.click((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, "primary");
    }
    fn text(&self, text: &str) -> Rect {
        *texts(&self.shapes, text).first().unwrap_or_else(|| panic!("no text {text:?} drawn"))
    }
}

#[test]
fn the_edit_menu_text_starts_28_and_its_shortcut_ends_12_inside_the_popup_edge() {
    let mut t = T::new();
    t.menu("Edit");
    let (undo, z) = (t.text("Undo"), t.text(&label(A::Undo)));
    let frame = popup_at(&t.ctx, undo.center());
    let (text_in, shortcut_in) = insets(frame, undo, z);
    assert!(near(text_in, TEXT_IN), "Undo starts {text_in} inside the edge, want 28");
    assert!(near(shortcut_in, SHORTCUT_IN), "{} ends {shortcut_in} inside the edge, want 12", label(A::Undo));
    // Every row, from menus.rs and the helpers it calls, shares the text x.
    let tool = crate::edit_tools::Tool::ALL[0].label();
    for row in ["Redo", "Cut", "Delete", "Bounce in Place", tool, "Undo History"] {
        assert!(near(t.text(row).min.x, undo.min.x), "{row} starts where Undo does");
    }
    for name in ["Bounce in Place", "Undo History"] {
        let r = t.app.arrange.bar_menu.iter().find(|m| m.label == name).unwrap().rect;
        assert_eq!(r[3] - r[1], c::ROW_HEIGHT, "{name} is 24 high");
        assert!(near(r[0] - frame.min.x, EDGE + c::POPUP_PADDING), "{name} sits 4 inside the edge");
    }
}

#[test]
fn the_view_menu_check_rows_and_submenus_share_the_text_x_28_inside_the_edge() {
    let mut t = T::new();
    t.menu("View");
    let browser = t.text("Grid Over Clips");
    let frame = popup_at(&t.ctx, browser.center());
    // "Mixer" and "Devices" are the lower pane's tabs too: take the text inside the menu.
    let inside = |row: &str| texts(&t.shapes, row).into_iter().find(|r| frame.contains(r.center())).unwrap_or_else(|| panic!("no {row:?} row"));
    for row in ["Browser", "Mixer", "Devices", "Track Height", "Appearance"] {
        assert!(near(inside(row).min.x, browser.min.x), "{row} starts where Grid Over Clips does");
    }
    assert!(near(browser.min.x - (frame.min.x + EDGE), TEXT_IN));
}

#[test]
fn a_track_header_context_menu_keeps_the_popup_padding_and_the_chrome_rows() {
    let mut t = T::new();
    let id = TrackId(t.app.run("track.new", json!({"kind": "audio", "name": "A"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.frames();
    let h = t.app.arrange.layout.tracks.iter().find(|r| r.id == id.0).unwrap().header;
    t.click(h[0] + 60.0, (h[1] + h[3]) / 2.0, "secondary");
    let freeze = t.app.arrange.menu.iter().find(|m| m.label == "Freeze").expect("the header's menu is open").rect;
    let frame = popup_at(&t.ctx, egui::pos2((freeze[0] + freeze[2]) / 2.0, (freeze[1] + freeze[3]) / 2.0));
    assert!(near(freeze[0] - frame.min.x, EDGE + c::POPUP_PADDING), "the row sits 4 inside the edge on the left");
    assert!(near(frame.max.x - freeze[2], EDGE + c::POPUP_PADDING), "and on the right");
    // The helpers shared with the Track menu draw the same chrome rows here (#409), so the group
    // and activity rows start where Freeze does, 28 inside the edge.
    let freeze_x = t.text("Freeze").min.x;
    assert!(near(freeze_x - (frame.min.x + EDGE), TEXT_IN), "Freeze starts {} inside the edge", freeze_x - frame.min.x - EDGE);
    assert!(near(t.text("New Group Track").min.x, freeze_x));
    assert!(near(t.text("Make Inactive").min.x, freeze_x));
}

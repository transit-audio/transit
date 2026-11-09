//! Context menus and popups on the chrome's rows (#409): `chrome::context_menu` and
//! `chrome::popup_menu` open the popup in the row style and every row is a `chrome::MenuRow`, so
//! a context menu's text starts 28 pt and its shortcut ends 12 pt inside the popup's edge, its
//! rows are 24 high and 4 inside the edge, and a row with a state shows the check centered in its
//! gutter. One test per surface family, each also clicking a row to show the action is the same.
use crate::menu_padding_tests::{EDGE, SHORTCUT_IN, Shapes, TEXT_IN, TEXT_IN_PLAIN, insets, near, popup_at, texts};
use crate::mixer::MenuItem;
use crate::shortcuts::{Action as A, label};
use crate::tokens::chrome as c;
use crate::{Services, StudioApp};
use egui::{Pos2, Rect};
use serde_json::{Value, json};
use transit_model::TrackId;

pub(crate) struct T {
    pub(crate) app: StudioApp,
    pub(crate) ctx: egui::Context,
    pub(crate) time: f64,
    pub(crate) shapes: Shapes,
}

impl T {
    pub(crate) fn new() -> Self {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = false;
        app.ui.show_devices = false;
        let mut t = Self { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() };
        t.frames(3);
        t
    }
    pub(crate) fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }
    }
    pub(crate) fn input(&mut self, method: &str, params: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
        self.frames(6);
    }
    pub(crate) fn click(&mut self, r: [f32; 4], button: &str) {
        self.input("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0, "button": button}));
    }
    pub(crate) fn track(&mut self, kind: &str, name: &str) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": kind, "name": name})).unwrap();
        self.frames(2);
        TrackId(r["tracks"][0].as_u64().unwrap())
    }
    pub(crate) fn header(&self, id: TrackId) -> [f32; 4] {
        self.app.arrange.layout.tracks.iter().find(|r| r.id == id.0).unwrap().header
    }
}

pub(crate) fn find(menu: &[MenuItem], label: &str) -> MenuItem {
    menu.iter()
        .find(|m| m.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("no row {label:?}: {:?}", menu.iter().map(|m| &m.label).collect::<Vec<_>>()))
}

pub(crate) fn rect(r: [f32; 4]) -> Rect {
    Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
}

/// What one open popup measures: its frame, and the text `row` drawn inside it.
pub(crate) struct Popup<'a> {
    pub(crate) frame: Rect,
    shapes: &'a Shapes,
}

impl<'a> Popup<'a> {
    /// The popup that holds the row `item`.
    pub(crate) fn of(t: &'a T, item: &MenuItem) -> Self {
        Popup { frame: popup_at(&t.ctx, rect(item.rect).center()), shapes: &t.shapes }
    }
    /// The text `s` as drawn inside this popup.
    pub(crate) fn text(&self, s: &str) -> Rect {
        texts(self.shapes, s).into_iter().find(|r| self.frame.contains(r.center())).unwrap_or_else(|| panic!("no text {s:?} in the popup"))
    }
    /// Where `s` starts, inside the edge's inner side.
    pub(crate) fn text_in(&self, s: &str) -> f32 {
        self.text(s).min.x - (self.frame.min.x + EDGE)
    }
    /// The row's checks: 24 high, 4 inside the edge on both sides.
    pub(crate) fn row(&self, item: &MenuItem) {
        let r = rect(item.rect);
        assert_eq!(r.height(), c::ROW_HEIGHT, "{} is 24 high", item.label);
        assert!(near(r.min.x - self.frame.min.x, EDGE + c::POPUP_PADDING), "{} sits 4 inside the edge: {r:?} in {:?}", item.label, self.frame);
        assert!(near(self.frame.max.x - r.max.x, EDGE + c::POPUP_PADDING), "{} on the right too", item.label);
    }
    /// The first row's text 28 inside the edge and, given, its shortcut or trailing text 12.
    pub(crate) fn insets(&self, text: &str, trailing: Option<&str>) {
        self.insets_at(text, trailing, TEXT_IN);
    }
    /// [`Self::insets`] for a menu with no checkable row: the text 12 inside the edge.
    pub(crate) fn insets_plain(&self, text: &str, trailing: Option<&str>) {
        self.insets_at(text, trailing, TEXT_IN_PLAIN);
    }
    fn insets_at(&self, text: &str, trailing: Option<&str>, want: f32) {
        let at = self.text_in(text);
        assert!(near(at, want), "{text:?} starts {at} inside the edge, want {want}");
        if let Some(s) = trailing {
            let (_, end) = insets(self.frame, self.text(text), self.text(s));
            assert!(near(end, SHORTCUT_IN), "{s:?} ends {end} inside the edge, want {SHORTCUT_IN}");
        }
    }
    /// The check marks (12 px meshes) centered in `item`'s gutter.
    pub(crate) fn checks_in(&self, item: &MenuItem) -> usize {
        let r = rect(item.rect);
        let gutter_x = r.min.x + c::ROW_PADDING[0] + c::CHECK_GUTTER / 2.0;
        self.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(m) => Some(m.calc_bounds()),
                _ => None,
            })
            .filter(|b| b.width() > 0.0 && b.height() <= c::CHECK_GLYPH + 0.5 && r.contains(b.center()))
            .filter(|b| near(b.center().x, gutter_x) && near(b.center().y, r.center().y))
            .count()
    }
}

/// Negative control: the row before #409 (egui's plain button, the old `gutter(false)` path) in
/// the same chrome popup starts its text 8 inside the edge, so the inset check rejects it.
#[test]
fn a_plain_row_in_the_chrome_context_popup_fails_the_inset_check() {
    let ctx = egui::Context::default();
    crate::theme::apply_pinned(&ctx, egui::Theme::Dark);
    let (mut rows, mut shapes) = (Vec::new(), Vec::new());
    for i in 0..3 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            time: Some(f64::from(i) / 60.0),
            ..Default::default()
        };
        let mut o = ctx.run_ui(raw, |ui| {
            let anchor = ui.button("Track");
            crate::chrome::popup_menu(&anchor, true).open(true).show(|ui| {
                rows = vec![ui.add(egui::Button::new("Freeze")).rect, ui.add(crate::chrome::MenuRow::new("Flatten")).rect];
            });
        });
        o.textures_delta.clear();
        shapes = o.shapes;
    }
    let frame = popup_at(&ctx, rows[0].center());
    let plain = texts(&shapes, "Freeze")[0].min.x - (frame.min.x + EDGE);
    let row = texts(&shapes, "Flatten")[0].min.x - (frame.min.x + EDGE);
    assert!(near(plain, 8.0) && !near(plain, TEXT_IN), "the plain row measured {plain}");
    assert!(near(row, TEXT_IN), "the chrome row in the same popup measured {row}");
}

#[test]
fn a_track_header_context_menu_has_the_chrome_rows_and_its_rows_still_act() {
    let mut t = T::new();
    let a = t.track("audio", "A");
    let h = t.header(a);
    t.click([h[0] + 50.0, h[1], h[0] + 70.0, h[3]], "secondary");
    let menu = t.app.arrange.menu.clone();
    let freeze = find(&menu, "Freeze");
    let p = Popup::of(&t, &freeze);
    // The track menu has no checkable row (#418): no gutter, the text 12 inside the edge.
    p.insets_plain("Freeze", None);
    p.insets_plain("Group Tracks", Some(&label(A::Group)));
    for row in ["Flatten", "New Group Track", "Color", "Make Inactive", "Delete Track"] {
        assert!(near(p.text_in(row), TEXT_IN_PLAIN), "{row} starts where Freeze does");
    }
    for row in ["Freeze", "Flatten", "New Group Track", "Group Tracks", "Make Inactive", "Delete Track"] {
        p.row(&find(&menu, row));
    }
    // The same actions: Make Inactive runs track.make_inactive, New Group Track adds the group.
    t.click(find(&menu, "Make Inactive").rect, "primary");
    assert!(t.app.engine.session().track(a).unwrap().inactive, "{}", t.app.status);
    let n = t.app.engine.session().tracks.len();
    t.click([h[0] + 50.0, h[1], h[0] + 70.0, h[3]], "secondary");
    let new = find(&t.app.arrange.menu, "New Group Track");
    t.click(new.rect, "primary");
    assert_eq!(t.app.engine.session().tracks.len(), n + 1, "{}", t.app.status);
    // The Instrument submenu keeps the chrome rows and has checks of its own (its MenuConfig carries the style).
    let keys = t.track("instrument", "Keys");
    let h = t.header(keys);
    t.click([h[0] + 50.0, h[1], h[0] + 70.0, h[3]], "secondary");
    let sub = find(&t.app.arrange.menu, "Instrument");
    // "Instrument" is also the header's subtitle under the popup: take the text in the row.
    let frame = Popup::of(&t, &sub).frame;
    let text = texts(&t.shapes, "Instrument").into_iter().find(|r| rect(sub.rect).contains(r.center())).expect("the submenu button's text");
    assert!(near(text.min.x - (frame.min.x + EDGE), TEXT_IN_PLAIN), "the Instrument button starts 12 in (its parent has no checks)");
    t.input("ui.move", json!({"x": (sub.rect[0] + sub.rect[2]) / 2.0, "y": (sub.rect[1] + sub.rect[3]) / 2.0}));
    t.frames(10);
    let menu = t.app.arrange.menu.clone();
    let sampler = menu.iter().find(|m| m.value == "sampler").cloned().expect("the submenu lists the instruments");
    let current = menu.iter().find(|m| m.value == "macro_oscillator").cloned().expect("the current instrument");
    let p = Popup::of(&t, &sampler);
    assert!(!p.frame.contains(rect(sub.rect).center()), "the rows are in the submenu's own popup");
    p.insets(&sampler.label, None);
    assert!(near(p.text_in("Built-in"), TEXT_IN), "the submenu's heading starts 28 in");
    p.row(&sampler);
    p.row(&current);
    assert_eq!(p.checks_in(&current), 1, "the current instrument is checked in its gutter");
    t.click(sampler.rect, "primary");
    assert_eq!(t.app.engine.session().track(keys).unwrap().instrument.as_ref().unwrap().plugin, "sampler", "{}", t.app.status);
}

#[test]
fn a_clip_context_menu_has_the_chrome_rows_and_bounce_in_place_still_opens_its_form() {
    let mut t = T::new();
    let keys = t.track("instrument", "Keys");
    t.app.run("midi.new_clip", json!({"track": keys.0, "start": 10_000, "length": 40_000})).unwrap();
    let clip = t.app.engine.session().track(keys).unwrap().clips()[0].id;
    t.app.engine.session_mut().edit.selected_clips = vec![clip];
    t.frames(3);
    let at = t.app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    t.click(at, "secondary");
    let menu = t.app.arrange.menu.clone();
    let bounce = find(&menu, "Bounce in Place");
    let p = Popup::of(&t, &bounce);
    let tool = crate::edit_tools::Tool::ALL.into_iter().find(|x| x.action().is_some()).unwrap();
    // No fade was clicked, so the clip menu has no checkable row (#418): the text starts 12 in.
    p.insets_plain(tool.label(), Some(&label(tool.action().unwrap())));
    p.insets_plain("Rename", Some(&label(A::Rename)));
    for row in ["Add Mono Audio Track", "Paste at Cursor", "Split at Cursor", "Bounce in Place", "Color"] {
        assert!(near(p.text_in(row), TEXT_IN_PLAIN), "{row} starts 12 inside the edge");
    }
    p.row(&bounce);
    t.click(bounce.rect, "primary");
    assert!(matches!(t.app.dialog, Some(crate::Dialog::BounceInPlace)), "{}", t.app.status);
    assert_eq!(t.app.bounce_place.clips, vec![clip]);
}

#[test]
fn a_mixer_strip_routing_list_has_the_chrome_rows_with_the_route_checked_and_picks_the_same_route() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    t.app.add_fx_channel().unwrap();
    t.app.ui.show_mixer = true;
    t.frames(3);
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == gtr.0).cloned().unwrap();
    t.click(strip.output, "primary");
    let menu = t.app.mixer.layout.menu.clone();
    let fx = find(&menu, "FX 1");
    let current = find(&menu, &strip.output_label);
    let p = Popup::of(&t, &fx);
    p.insets("FX 1", None);
    assert!(near(p.text_in(&strip.output_label), TEXT_IN), "the current route starts where FX 1 does");
    p.row(&fx);
    p.row(&current);
    assert_eq!(p.checks_in(&current), 1, "the current route's check sits centered in its gutter");
    assert_eq!(p.checks_in(&fx), 0, "another route's gutter stays empty");
    t.click(fx.rect, "primary");
    let s = t.app.mixer.layout.strips.iter().find(|s| s.track == gtr.0).cloned().unwrap();
    assert_eq!(s.output_label, "FX 1", "{}", t.app.status);
}

#[test]
fn a_device_card_instrument_menu_has_the_chrome_rows_with_the_instrument_checked_and_picks_the_same() {
    let mut t = T::new();
    let keys = t.track("instrument", "Keys");
    t.app.ui.show_devices = true;
    t.app.focus(keys);
    t.frames(3);
    let card = t.app.mixer.devices.cards.first().cloned().unwrap();
    t.click(card.pick_button, "primary");
    let menu = t.app.mixer.devices.menu.clone();
    let current = menu.iter().find(|m| m.value == "macro_oscillator").cloned().expect("the instrument is listed");
    let sampler = menu.iter().find(|m| m.value == "sampler").cloned().unwrap();
    let hardware = find(&menu, crate::external_instrument::MENU_LABEL);
    let p = Popup::of(&t, &hardware);
    p.insets(crate::external_instrument::MENU_LABEL, None);
    for row in [current.label.as_str(), sampler.label.as_str(), "Built-in"] {
        assert!(near(p.text_in(row), TEXT_IN), "{row} starts 28 inside the edge");
    }
    p.row(&hardware);
    p.row(&current);
    assert_eq!(p.checks_in(&current), 1, "the current instrument's check sits centered in its gutter");
    assert_eq!(p.checks_in(&sampler), 0);
    t.click(sampler.rect, "primary");
    assert_eq!(t.app.engine.session().track(keys).unwrap().instrument.as_ref().unwrap().plugin, "sampler", "{}", t.app.status);
}

/// A scratch folder holding `Kicks/`, removed on drop.
struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_browser_location_menu_has_the_chrome_rows_and_add_to_locations_still_adds() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = Dir(std::env::temp_dir().join(format!("transit-ctxmenu-{}-{nanos}", std::process::id())));
    std::fs::create_dir_all(dir.0.join("Kicks")).unwrap();
    std::fs::write(dir.0.join("Kicks").join("notes.txt"), b"x").unwrap();
    let root = dir.0.to_string_lossy().into_owned();
    let kicks = dir.0.join("Kicks").to_string_lossy().into_owned();
    let mut t = T::new();
    t.app.ui.browser.show = true;
    assert!(t.app.add_location(&root));
    t.frames(3);
    let row = |t: &T, path: &str, root: bool| t.app.browser.layout.rows.iter().find(|r| r.path == path && r.root == root).and_then(|r| r.rect);
    t.click(row(&t, &root, true).expect("the location row"), "primary");
    let start = std::time::Instant::now();
    while row(&t, &kicks, false).is_none() && start.elapsed().as_secs() < 5 {
        t.frames(2);
    }
    t.click(row(&t, &kicks, false).expect("the Kicks row"), "secondary");
    let add = find(&t.app.browser.menu, "Add to Locations");
    let p = Popup::of(&t, &add);
    p.insets_plain("Add to Locations", None);
    p.row(&add);
    t.click(add.rect, "primary");
    assert_eq!(t.app.ui.browser.locations, vec![root, kicks], "{}", t.app.status);
}

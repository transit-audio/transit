//! In-app help (#178): the Info panel and the catalog behind it.
//!
//! Convention: Ableton Live's Info View and Logic Pro's Quick Help. Both are a small panel in a
//! corner of the main window that explains the control under the mouse and is switched on and off
//! with a key (Shift+? in both, which is the toggle here); the controls themselves carry no extra
//! decoration. The panel names the control, says in a few plain sentences what it does or what a
//! figure means, gives its key combination if it has one, and offers "More", which opens the
//! matching docs page in the browser.
//!
//! One source: every entry lives in `docs/help/*.toml` (compiled in by [`catalog`]) and is keyed by
//! an [`Id`]. A widget registers the id of the control it draws with [`hover`]; the panel shows the
//! entry of the last control the pointer was over, and keeps it when the pointer moves to the panel
//! to press More. Circled question marks ([`header_button`]) exist only where there is nothing to
//! hover, the headers of the diagnostics windows, and show the same entry. `cargo xtask help`
//! checks that every id in `ids.rs` has an entry, that every entry has an id, that every id is used
//! by a widget, and that the generated sections of `docs/troubleshooting.md` match the catalog.

mod catalog;
mod ids;
pub mod map;

pub use catalog::{DOCS_URL, Entry, entry};
pub use ids::Id;

#[cfg(test)]
pub use catalog::{area_of, keys};

use crate::StudioApp;
use crate::arrange::{R4, r4};
use crate::theme::{RADIUS, SPACE, STATUS_H, TOKENS};
use egui::{Align2, Sense, Ui, vec2};

/// Width of the panel in points.
const PANEL_W: f32 = 300.0;

/// Where [`hover`] leaves the control the pointer is over, for the panel to pick up at the end of
/// the frame.
fn slot() -> egui::Id {
    egui::Id::new("transit_help_hovered")
}

/// Register `id` as the help for `response`'s control: when the pointer is over it, the Info panel
/// shows its entry. Cheap enough to call for every frame; a no-op unless the pointer is there.
pub fn hover(ui: &Ui, response: &egui::Response, id: Id) {
    let hot = response.hovered() || response.contains_pointer();
    let visible = ui.is_rect_visible(response.rect);
    ui.ctx().data_mut(|d| {
        if hot {
            d.insert_temp(slot(), Some(id));
        }
        // With the panel on, every visible target is recorded for `ui.inspect` and the tests.
        if visible && d.get_temp::<bool>(on_slot()).unwrap_or(false) {
            d.get_temp_mut_or_default::<Vec<(Id, R4)>>(targets_slot()).push((id, r4(response.rect)));
        }
    });
}

/// Whether the panel was on at the end of the last frame (so [`hover`] records targets).
fn on_slot() -> egui::Id {
    egui::Id::new("transit_help_on")
}

fn targets_slot() -> egui::Id {
    egui::Id::new("transit_help_targets")
}

/// What the Info panel is showing, and the toggle state.
#[derive(Default)]
pub struct HelpState {
    /// The panel is on (the shortcut or Help > Info Panel; not saved with the session).
    pub open: bool,
    /// The entry in the panel: the last one hovered, or the one a question mark asked for.
    pub shown: Option<Id>,
    /// The page More opened last (tests read it; the browser got the same URL).
    pub opened_url: Option<String>,
    pub layout: HelpLayout,
}

/// Where the panel and the question marks were last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HelpLayout {
    pub open: bool,
    /// The key of the entry in the panel, for example `telemetry.load_now`.
    pub id: Option<String>,
    pub name: String,
    pub text: String,
    pub key: Option<String>,
    pub rect: R4,
    pub more: R4,
    /// Where each help target was drawn last frame, by entry key (only while the panel is on).
    pub targets: std::collections::BTreeMap<String, R4>,
    /// The circled question marks drawn this frame, by the key of the entry they open.
    pub question_marks: std::collections::BTreeMap<String, R4>,
}

impl HelpState {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Show `id` in the panel and switch the panel on (a question mark was pressed).
    pub fn ask(&mut self, id: Id) {
        self.open = true;
        self.shown = Some(id);
    }
}

/// A circled question mark for a window header that has no hover target. Pressing it opens the
/// Info panel on `id`'s entry; hovering it shows the same entry when the panel is already on.
pub fn header_button(app: &mut StudioApp, ui: &mut Ui, id: Id) {
    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let hot = resp.hovered();
    let p = ui.painter();
    let ink = if hot { TOKENS.accent } else { TOKENS.text_dim };
    p.circle_stroke(r.center(), 8.0, egui::Stroke::new(1.0, ink));
    p.text(r.center(), Align2::CENTER_CENTER, "?", egui::FontId::proportional(11.5), ink);
    app.help.layout.question_marks.insert(id.key().to_owned(), r4(r));
    hover(ui, &resp, id);
    if resp.clicked() {
        app.help.ask(id);
    }
}

/// Draw one status line of the Audio Devices card with a hover on each figure in it: the pieces
/// come from [`map::status_segments`] and join back to the line unchanged.
pub fn status_line(ui: &mut Ui, line: &str, mono: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for seg in map::status_segments(line) {
            let text = egui::RichText::new(&seg.text).color(TOKENS.text_dim).size(11.5);
            let resp = ui.label(if mono { text.monospace() } else { text });
            if let Some(id) = seg.id {
                hover(ui, &resp, id);
            }
        }
    });
}

/// The Info panel, drawn last in the frame (bottom-left, above the status bar). Picks up what
/// [`hover`] saw this frame; with the panel off it only clears that.
pub fn panel(app: &mut StudioApp, ctx: &egui::Context) {
    let hovered = ctx.data_mut(|d| d.remove_temp::<Option<Id>>(slot())).flatten();
    let targets = ctx.data_mut(|d| {
        d.insert_temp(on_slot(), app.help.open);
        d.remove_temp::<Vec<(Id, R4)>>(targets_slot())
    });
    app.help.layout.targets = targets.unwrap_or_default().into_iter().map(|(id, r)| (id.key().to_owned(), r)).collect();
    app.help.layout.open = app.help.open;
    app.help.layout.rect = [0.0; 4];
    app.help.layout.more = [0.0; 4];
    if !app.help.open {
        return;
    }
    if let Some(id) = hovered {
        app.help.shown = Some(id);
    }
    let shown = app.help.shown.and_then(|id| entry(id).map(|e| (id, e)));
    let toggle = crate::shortcuts::label(crate::shortcuts::Action::ToggleInfo);
    let mut open_url = None;
    let at = ctx.content_rect();
    let r = egui::Area::new(egui::Id::new("help_panel"))
        .order(egui::Order::Foreground)
        .anchor(Align2::LEFT_BOTTOM, vec2(SPACE, -(STATUS_H + SPACE)))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(TOKENS.panel_raised)
                .stroke(egui::Stroke::new(1.0, TOKENS.line))
                .corner_radius(RADIUS)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(PANEL_W.min(at.width() - 4.0 * SPACE).max(120.0));
                    match shown {
                        Some((_, e)) => {
                            ui.label(egui::RichText::new(&e.name).strong().color(TOKENS.text));
                            ui.label(egui::RichText::new(&e.text).color(TOKENS.text_dim).size(12.0));
                            if let Some(key) = &e.key {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Key").color(TOKENS.text_dim).size(11.0));
                                    ui.label(egui::RichText::new(key).font(crate::theme::mono(ui.ctx(), 12.0)).color(TOKENS.accent));
                                });
                            }
                            if let Some(url) = e.url() {
                                let b = ui.small_button("More");
                                app.help.layout.more = r4(b.rect);
                                if b.on_hover_text("Open the matching docs page in your browser").clicked() {
                                    open_url = Some(url);
                                }
                            }
                        }
                        None => {
                            ui.label(egui::RichText::new("Info").strong().color(TOKENS.text));
                            ui.label(
                                egui::RichText::new(format!("Point at a control or a figure to see what it does. {toggle} hides this panel."))
                                    .color(TOKENS.text_dim)
                                    .size(12.0),
                            );
                        }
                    }
                });
        });
    app.help.layout.rect = r4(r.response.rect);
    app.help.layout.id = shown.map(|(id, _)| id.key().to_owned());
    app.help.layout.name = shown.map(|(_, e)| e.name.clone()).unwrap_or_default();
    app.help.layout.text = shown.map(|(_, e)| e.text.clone()).unwrap_or_default();
    app.help.layout.key = shown.and_then(|(_, e)| e.key.clone());
    if let Some(url) = open_url {
        ctx.open_url(egui::OpenUrl::new_tab(&url));
        app.help.opened_url = Some(url);
    }
}

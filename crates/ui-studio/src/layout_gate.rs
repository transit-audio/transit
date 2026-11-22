//! The dialog layout gate (#735): after a test has drawn a dialog or window with its longest
//! realistic content, no button's rectangle may intersect another widget's rectangle or any text
//! the dialog paints, and every button must lie inside the window. #732 (the status card growing
//! to seven lines over the button row of Audio Devices) passed every test because nothing asserted
//! that controls do not overlap.
//!
//! Adding the check to a dialog's test is two lines: `watch` once when the test builds its rig and
//! `assert_clean` after the frames that draw the dialog, at each of [`SCALES`] (the tests set
//! `ui.ui_zoom` and the screen, as the existing scale tests do):
//!
//! ```ignore
//! let layer = layout_gate::window("Audio Devices");
//! layout_gate::watch(&t.ctx, layer);
//! // ... frames with the longest content ...
//! layout_gate::assert_clean(&t.ctx, layer);
//! ```
//!
//! What is read. The widget rectangles egui recorded in its last pass (`prev_pass.widgets`), in the
//! dialog's own layer, and the ink of the text the layer painted, one rectangle per row of a label
//! that wraps (a plugin reads the layer's paint list at the end of each pass; a label's widget rectangle is a layout reservation
//! that can be far wider than its text, the painted text is what a reader sees).
//!
//! * A *button* is a widget that senses clicks and not drags ([`is_button`]).
//! * "Another widget" is another button or a painted piece of text. Other interactive widgets are
//!   not compared: this app's labels are selectable, so a label's widget rectangle is as wide as
//!   its layout row and would flag every sidebar entry, and scroll bars float over the edge of
//!   the content by design. A slider or text field is judged by the text it paints.
//! * Rectangles are the visible part (clipped by the parent's clip rect), so a button scrolled out
//!   of a scroll area is not a violation and a half-visible one is judged by what shows.
//! * Edge contact is not an overlap: rectangles are shrunk by [`TOLERANCE`] points first.
//! * Nesting (within [`NESTING`] points) is not an overlap: something wholly inside the button is its content (icon, label),
//!   something wholly holding it is its background (title bar, scroll viewport, card). The limit of
//!   that rule: a single line of text that is wider and taller than a small button and sits exactly
//!   over it is not caught; a partial cover, as in #732, is.

use egui::{Context, Id, LayerId, Order, Rect, Shape};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The interface scales every dialog is checked at: 1x and Windows at 150 percent.
pub(crate) const SCALES: [f32; 2] = [1.0, 1.5];

/// Points two rectangles may share before they count as intersecting (float spacing noise).
const TOLERANCE: f32 = 0.5;

/// Points by which a label's glyphs may stick out of its button's rectangle and still count as the
/// button's own content (a glyph overhangs the advance width by a fraction of a point).
const NESTING: f32 = 2.0;

/// The layer of a `theme::window` / `theme::modal_window` window with this title.
pub(crate) fn window(title: &str) -> LayerId {
    // egui derives the id from `Atoms::text()`, which is an `Option`: the `Some` is part of the hash.
    LayerId::new(Order::Middle, Id::new(Some(title)))
}

/// The layer of an `egui::Modal` created with this id.
pub(crate) fn modal(id: &str) -> LayerId {
    modal_layer(Id::new(id))
}

/// The layer of an `egui::Modal` created with this id (for ids that are not a plain string).
pub(crate) fn modal_layer(id: Id) -> LayerId {
    LayerId::new(Order::Foreground, id)
}

/// What one layer painted in its last pass: the text, and every piece of shape of any kind.
#[derive(Default, Clone)]
struct Painted {
    texts: Vec<Rect>,
    shapes: Vec<Rect>,
}

type Texts = Arc<Mutex<HashMap<LayerId, Painted>>>;

/// Reads the painted text of the watched layers at the end of every pass.
struct Capture {
    texts: Texts,
    layers: Vec<LayerId>,
}

/// The ink of a text shape, one rectangle per row of its galley. A label that wraps (in a wrapped
/// row of buttons it can end one row and continue on the next) is one shape whose bounding box
/// spans both rows and everything between them; the glyphs only cover each row's own ink.
fn text_rects(t: &egui::epaint::TextShape, clip: Rect, out: &mut Vec<Rect>) {
    if t.angle != 0.0 {
        let r = t.visual_bounding_rect().intersect(clip);
        if r.is_positive() {
            out.push(r);
        }
        return;
    }
    for row in &t.galley.rows {
        let r = row.visuals.mesh_bounds.translate(row.pos.to_vec2() + t.pos.to_vec2()).intersect(clip);
        if r.is_positive() {
            out.push(r);
        }
    }
}

fn collect(shape: &Shape, clip: Rect, out: &mut Painted) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| collect(s, clip, out)),
        Shape::Noop => {}
        other => {
            let r = other.visual_bounding_rect().intersect(clip);
            if r.is_positive() {
                out.shapes.push(r);
                if let Shape::Text(t) = other {
                    text_rects(t, clip, &mut out.texts);
                }
            }
        }
    }
}

impl egui::Plugin for Capture {
    fn debug_name(&self) -> &'static str {
        "layout_gate"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        let mut found: HashMap<LayerId, Painted> = HashMap::new();
        ui.ctx().graphics(|g| {
            for layer in &self.layers {
                let mut painted = Painted::default();
                if let Some(list) = g.get(*layer) {
                    for entry in list.all_entries() {
                        collect(&entry.shape, entry.clip_rect, &mut painted);
                    }
                }
                found.insert(*layer, painted);
            }
        });
        *self.texts.lock().unwrap() = found;
    }
}

/// Start reading the text `layer` paints. Call once per context, before the frames that draw it
/// (a second call with another layer adds it).
pub(crate) fn watch(ctx: &Context, layer: LayerId) {
    let texts_id = Id::new("layout_gate_texts");
    let layers_id = Id::new("layout_gate_layers");
    let texts: Texts = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(texts_id, Texts::default).clone());
    let mut layers: Vec<LayerId> = ctx.data_mut(|d| d.get_temp(layers_id).unwrap_or_default());
    if !layers.contains(&layer) {
        layers.push(layer);
    }
    ctx.data_mut(|d| d.insert_temp(layers_id, layers.clone()));
    let again = layers.clone();
    if ctx.with_plugin::<Capture, _>(|c| c.layers = again).is_none() {
        ctx.add_plugin(Capture { texts, layers });
    }
}

/// A button is a widget that senses clicks and not drags: egui's buttons, checkboxes and selectable
/// rows. Sliders, text fields, scroll areas and the window's drag areas also sense drags.
fn is_button(w: &egui::WidgetRect) -> bool {
    w.sense.senses_click() && !w.sense.senses_drag()
}

fn shrunk(r: Rect) -> Rect {
    r.shrink(TOLERANCE)
}

/// What the gate measured on one dialog: the counts say the check looked at something.
pub(crate) struct Report {
    pub(crate) buttons: usize,
    pub(crate) texts: usize,
    pub(crate) findings: Vec<String>,
}

/// Measure the dialog on `layer` from the context's last pass. Panics when the layer holds no
/// window area, no button, or was never `watch`ed, so a wrong title or id can never pass quietly.
pub(crate) fn measure(ctx: &Context, layer: LayerId) -> Report {
    let window = ctx.memory(|m| m.area_rect(layer.id)).unwrap_or_else(|| panic!("no window or dialog is drawn on {layer:?}"));
    let painted: Painted = ctx
        .with_plugin::<Capture, _>(|c| c.texts.lock().unwrap().get(&layer).cloned())
        .flatten()
        .unwrap_or_else(|| panic!("layout_gate::watch(ctx, {layer:?}) was not called before the frames"));
    let rects: Vec<egui::WidgetRect> = ctx.viewport(|v| v.prev_pass.widgets.get_layer(layer).copied().collect());
    let all: Vec<&egui::WidgetRect> = rects.iter().filter(|w| w.interact_rect.is_positive()).collect();
    // A `Ui` is a widget too (its rectangle is the room its content took) and egui's parent ids
    // do not name those rectangles, so a container is found by shape: a non-button that wholly
    // holds another widget. Its content is what a button can collide with, not the container.
    // A button the pass measured but did not paint (a sizing probe, a hidden row) is not on screen:
    // a real button paints its frame or its label inside its own rectangle.
    let painted_inside = |w: &egui::WidgetRect| painted.shapes.iter().any(|s| w.interact_rect.expand(1.0).contains_rect(*s));
    let buttons: Vec<&egui::WidgetRect> = all.iter().copied().filter(|w| is_button(w) && painted_inside(w)).collect();
    let texts = painted.texts;
    assert!(!buttons.is_empty(), "the dialog on {layer:?} drew no button: the gate would pass on nothing");
    let apart = |r: Rect, o: Rect| {
        // Nested is not an overlap: the other is the button's content, or the button sits in it.
        o.expand(NESTING).contains_rect(r) || r.expand(NESTING).contains_rect(o) || !shrunk(r).intersects(shrunk(o))
    };
    let mut findings = Vec::new();
    for (i, b) in buttons.iter().enumerate() {
        let r = b.interact_rect;
        if !window.expand(TOLERANCE).contains_rect(r) {
            findings.push(format!("button {r:?} lies outside the window {window:?}"));
        }
        for o in buttons.iter().skip(i + 1) {
            if !apart(r, o.interact_rect) {
                findings.push(format!("button {r:?} intersects button {:?}", o.interact_rect));
            }
        }
        for t in &texts {
            if !apart(r, *t) {
                findings.push(format!("button {r:?} intersects painted text {t:?}"));
            }
        }
    }
    Report { buttons: buttons.len(), texts: texts.len(), findings }
}

/// The one-line check: panics, listing each violation, when a button of the dialog on `layer`
/// intersects another button or painted text, or lies outside the window.
#[track_caller]
pub(crate) fn assert_clean(ctx: &Context, layer: LayerId) {
    let report = measure(ctx, layer);
    assert!(
        report.findings.is_empty(),
        "{} finding(s) among {} buttons and {} texts at {} ppp:\n{}",
        report.findings.len(),
        report.buttons,
        report.texts,
        ctx.pixels_per_point(),
        report.findings.join("\n")
    );
}

/// Like [`assert_clean`], but returns the findings instead of panicking (a first run lists them).
pub(crate) fn findings(ctx: &Context, layer: LayerId) -> Vec<String> {
    measure(ctx, layer).findings
}

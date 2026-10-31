//! The Macro Oscillator's modulation matrix on its panel (#243): a compact grid below the shaper,
//! the sixteen rows in two columns of eight, each row a source, a via, a destination and a curve
//! dropdown and an amount you drag (-100% to +100%). Every cell is a parameter of its own
//! (`mod<n>_<field>`), so it takes MIDI Learn, automation and undo like the panel's knobs; values go
//! through `StudioApp::set_builtin_param`.

use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::panel::{choice, finish};
use crate::theme::TOKENS;
use egui::{RichText, Ui};
use transit_dsp::PluginInfo;
use transit_dsp::plugins::macro_osc::matrix::ROWS;
use transit_dsp::plugins::macro_osc::row_param;
use transit_model::{Insert, TrackId};

/// The row fields in the order the grid shows them, with each column's heading and width: the
/// offsets are the parameter table's (source 0, via 1, destination 2, amount 3, curve 4).
const COLUMNS: [(usize, &str, f32); 5] = [(0, "Source", 88.0), (1, "Via", 70.0), (2, "Destination", 98.0), (4, "Curve", 72.0), (3, "Amount", 44.0)];
/// Rows per column of the grid.
const PER_COLUMN: usize = 8;
const NUMBER_W: f32 = 16.0;
/// A grid line's height and the gap between lines: compact, so the expanded window stays within
/// 800 pt.
const LINE_H: f32 = 18.0;
const LINE_GAP: f32 = 2.0;

/// Every row parameter the grid shows, in row order.
pub fn params() -> impl Iterator<Item = &'static str> {
    (0..ROWS).flat_map(|r| COLUMNS.iter().filter_map(move |(f, ..)| row_param(r, *f)))
}

/// Draws the grid.
pub fn show(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, lay: &mut EditorLayout) {
    ui.vertical(|ui| grid(app, ui, key, ins, info, lay));
}

fn grid(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, lay: &mut EditorLayout) {
    let heading =
        ui.label(RichText::new("Each row adds amount × curve(source) × via to its destination, per voice").size(10.5).color(TOKENS.text_dim));
    crate::help::hover(ui, &heading, crate::help::Id::MacroOscMatrix);
    ui.horizontal_top(|ui| {
        for first in (0..ROWS).step_by(PER_COLUMN) {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = LINE_GAP;
                ui.spacing_mut().button_padding.y = 1.0;
                ui.spacing_mut().interact_size.y = LINE_H;
                ui.horizontal(|ui| {
                    ui.add_sized([NUMBER_W, 14.0], egui::Label::new(""));
                    for (_, title, w) in COLUMNS {
                        ui.add_sized([w, 14.0], egui::Label::new(RichText::new(title).size(10.0).color(TOKENS.text_dim)));
                    }
                });
                for r in first..(first + PER_COLUMN).min(ROWS) {
                    ui.horizontal(|ui| {
                        ui.add_sized([NUMBER_W, LINE_H], egui::Label::new(RichText::new(format!("{}", r + 1)).size(10.5).color(TOKENS.text_dim)));
                        for (field, _, w) in COLUMNS {
                            cell(app, ui, key, ins, info, r, field, w, lay);
                        }
                    });
                }
            });
            if first + PER_COLUMN < ROWS {
                ui.add_space(10.0);
            }
        }
    });
}

/// One cell: a dropdown for a choice, a drag value for the amount.
#[allow(clippy::too_many_arguments)]
fn cell(
    app: &mut StudioApp,
    ui: &mut Ui,
    key: (TrackId, usize),
    ins: &Insert,
    info: &'static PluginInfo,
    row: usize,
    field: usize,
    width: f32,
    lay: &mut EditorLayout,
) {
    let Some(id) = row_param(row, field) else { return };
    let Some(p) = info.param(id) else { return };
    let v = p.clamp(app.followed_param(key, id).or_else(|| ins.params.get(id).copied()).unwrap_or(p.default));
    let drawn = if p.choices.is_empty() {
        let mut x = v;
        let r = ui.add_sized([width, LINE_H], egui::DragValue::new(&mut x).range(p.min..=p.max).speed(0.5).fixed_decimals(0).suffix("%"));
        let reset = crate::builtin_editor::double_clicked(ui, &r);
        let new = if reset { Some(p.default) } else { (x != v).then_some(x) };
        (r, new, p.format(new.unwrap_or(v)))
    } else {
        choice(ui, ins, p, "", v, width - 8.0)
    };
    finish(app, ui, key, info, id, drawn, v, lay);
}

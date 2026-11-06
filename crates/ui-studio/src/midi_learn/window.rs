//! Mapping table presentation.

use super::*;

fn kind_color(over: bool) -> Color32 {
    if over { TOKENS.text_dim } else { TOKENS.text }
}

/// Natural widths of the table's fixed columns, then the least widths of its stretch columns.
const COL_CH: f32 = 36.0;
const COL_RANGE: f32 = 124.0;
const COL_THRU: f32 = 90.0;
const COL_SCOPE: f32 = 176.0;
const COL_LEARN: f32 = 90.0;
const COL_DELETE: f32 = 64.0;
const MIN_PORT: f32 = 70.0;
const MIN_SOURCE: f32 = 90.0;
const MIN_TARGET: f32 = 130.0;
/// The widest the content-sized columns grow to fit a long name.
const MAX_PORT: f32 = 220.0;
const MAX_SOURCE: f32 = 170.0;
const MAX_TARGET: f32 = 360.0;
const COLUMN_GAP: f32 = 8.0;
const COLUMNS: f32 = 9.0;
const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 20.0;
/// The Takeover row: a combo box and its line of text.
const TAKEOVER_H: f32 = 28.0;
/// What the window adds to its size limits (measured): the title bar and frame heights; the width limits are the outer width.
const CHROME: egui::Vec2 = egui::vec2(0.0, 36.0);
/// Room the table keeps beside the window's content edge.
const SIDE_ROOM: f32 = 24.0;
/// The vertical scrollbar's room at the table's right edge.
const SCROLLBAR_W: f32 = 14.0;

/// The window's size limits (outer rectangle, in points) on a screen whose content area is
/// `content`: the table's natural width up to 1.5 times it, and from the Takeover row, the header
/// and three rows up to the content height under the transport bar.
pub fn window_bounds(content: egui::Vec2, cols: StretchWidths) -> (egui::Vec2, egui::Vec2) {
    let natural = natural_width(cols);
    let min = egui::vec2(natural.min(content.x), TAKEOVER_H + HEADER_H + 3.0 * ROW_H + CHROME.y);
    let max = egui::vec2((natural * 1.5).min(content.x).max(min.x), (content.y - crate::theme::TRANSPORT_H).max(min.y));
    (min, max)
}

/// The table at its natural width: every column at its width, the gaps, the scrollbar and the
/// window's margins.
fn natural_width(cols: StretchWidths) -> f32 {
    cols.port
        + COL_CH
        + cols.source
        + cols.target
        + COL_RANGE
        + COL_THRU
        + COL_SCOPE
        + COL_LEARN
        + COL_DELETE
        + COLUMN_GAP * COLUMNS
        + SCROLLBAR_W
        + SIDE_ROOM
}

/// The stored size clamped to `bounds`; the default (natural width, ten rows) when it is unusable.
fn window_size(stored: [f32; 2], bounds: (egui::Vec2, egui::Vec2)) -> egui::Vec2 {
    let (min, max) = bounds;
    let size = if stored.iter().all(|v| v.is_finite() && *v > 0.0) { egui::vec2(stored[0], stored[1]) } else { min };
    size.clamp(min, max)
}

/// The widths of the stretch columns at which the longest port, source and target among the rows
/// show whole (within the caps; the floors for a short or empty list).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StretchWidths {
    pub port: f32,
    pub source: f32,
    pub target: f32,
}

/// The stretch columns' natural widths for `rows`, measured in the table's font.
pub fn stretch_widths(ctx: &egui::Context, rows: &[Row]) -> StretchWidths {
    let font = crate::theme::mono(ctx, 12.0);
    let longest = |f: &dyn Fn(&Row) -> &str, lo: f32, hi: f32| {
        let w = rows
            .iter()
            .map(|r| ctx.fonts_mut(|fonts| fonts.layout_no_wrap(f(r).to_string(), font.clone(), Color32::WHITE).size().x))
            .fold(0.0, f32::max);
        (w + 4.0).clamp(lo, hi)
    };
    StretchWidths {
        port: longest(&|r| r.port.as_str(), MIN_PORT, MAX_PORT),
        source: longest(&|r| r.source.as_str(), MIN_SOURCE, MAX_SOURCE),
        target: longest(&|r| r.target.as_str(), MIN_TARGET, MAX_TARGET),
    }
}

/// `text` with its middle replaced by an ellipsis until it fits `width`.
fn middle_fit(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let wide = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x;
    if wide(text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (2..chars.len()).rev() {
        let head: String = chars[..keep.div_ceil(2)].iter().collect();
        let tail: String = chars[chars.len() - keep / 2..].iter().collect();
        let t = format!("{head}\u{2026}{tail}");
        if wide(&t) <= width {
            return t;
        }
    }
    "\u{2026}".to_string()
}

/// Window > MIDI Mappings.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    app.midi_learn.window = [0.0; 4];
    if !app.ui.midi_mappings.open {
        return;
    }
    let mut open = true;
    let rows = rows(app);
    let cols = stretch_widths(ctx, &rows);
    let bounds = window_bounds(ctx.content_rect().size(), cols);
    let size = window_size(app.ui.midi_mappings.size, bounds);
    let dropped = app.midi_learn.dropped;
    let mut takeover = app.ui.midi_takeover;
    let mut actions: Vec<(usize, Action)> = Vec::new();
    let r = crate::theme::window(ctx, "MIDI Mappings")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .min_size(bounds.0 - CHROME)
        .max_size(bounds.1 - CHROME)
        .default_size(size - CHROME)
        .default_pos(egui::pos2(ctx.content_rect().center().x - size.x / 2.0, crate::theme::TRANSPORT_H + 80.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Hover a control for what it does.").color(TOKENS.text_dim).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| crate::help::header_button(app, ui, crate::help::Id::MidiMappingsWindow));
            });
            if dropped > 0 {
                ui.label(egui::RichText::new(format!("{dropped} controller button presses or learn messages were dropped (the UI was not draining fast enough).")).color(TOKENS.record));
            }
            ui.horizontal(|ui| {
                ui.label("Takeover");
                crate::chrome::field(ui, |ui| egui::ComboBox::from_id_salt("mm_takeover").selected_text(takeover.label()).width(90.0).icon(crate::chrome::chevron).show_ui(ui, |ui| {
                    for m in [MapMode::Scaled, MapMode::Jump, MapMode::Pickup] {
                        ui.selectable_value(&mut takeover, m, m.label());
                    }
                }));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(match takeover {
                            MapMode::Scaled => "A knob moves a control toward it and they meet at the end of the knob's travel.",
                            MapMode::Jump => "A control takes the knob's value at once.",
                            MapMode::Pickup => "A control waits until the knob passes through its value.",
                        })
                        .color(TOKENS.text_dim),
                    )
                    .truncate(),
                );
            });
            if rows.is_empty() {
                ui.label(
                    egui::RichText::new("No mappings yet. Right-click a fader, pan, mute, solo, record arm or send level and choose MIDI Learn, then move a knob or press a key on your controller.")
                        .color(TOKENS.text_dim),
                );
                return;
            }
            let mono = crate::theme::mono(ui.ctx(), 12.0);
            let cell = |ui: &mut egui::Ui, text: &str, ink: Color32| {
                let shown = middle_fit(ui, text, &mono, ui.available_width());
                let l = ui.add(egui::Label::new(egui::RichText::new(&shown).font(mono.clone()).color(ink)).truncate());
                if shown != text {
                    l.on_hover_text(text);
                }
            };
            let scroll_h = (ui.available_height() - HEADER_H - 4.0).max(3.0 * ROW_H);
            egui_extras::TableBuilder::new(ui)
                .id_salt("midi_mappings")
                .striped(true)
                .resizable(false)
                .vscroll(true)
                .auto_shrink([false, false])
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .min_scrolled_height(3.0 * ROW_H)
                .max_scroll_height(scroll_h)
                .column(egui_extras::Column::remainder().at_least(cols.port).clip(true))
                .column(egui_extras::Column::exact(COL_CH))
                .column(egui_extras::Column::remainder().at_least(cols.source).clip(true))
                .column(egui_extras::Column::remainder().at_least(cols.target).clip(true))
                .column(egui_extras::Column::exact(COL_RANGE))
                .column(egui_extras::Column::exact(COL_THRU))
                .column(egui_extras::Column::exact(COL_SCOPE))
                .column(egui_extras::Column::exact(COL_LEARN))
                .column(egui_extras::Column::exact(COL_DELETE))
                .header(HEADER_H, |mut h| {
                    for name in ["Port", "Ch", "Source", "Target", "Range", "Pass through", "Scope", "", ""] {
                        h.col(|ui| {
                            ui.label(egui::RichText::new(name).font(mono.clone()).strong().color(TOKENS.text_dim));
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, rows.len(), |mut tr| {
                        let i = tr.index();
                        let row = &rows[i];
                        let ink = kind_color(row.overridden || !row.live);
                        tr.col(|ui| cell(ui, &row.port, ink));
                        tr.col(|ui| cell(ui, &row.channel, ink));
                        tr.col(|ui| cell(ui, &row.source, ink));
                        tr.col(|ui| cell(ui, &row.target, ink));
                        tr.col(|ui| {
                            if row.target_id.starts_with("transport:") {
                                cell(ui, "Momentary", ink);
                                return;
                            }
                            // An edit lives in the row's edit state until it commits: a drag adds to the
                            // value it held a frame ago, so the stored range cannot be put back each frame.
                            let edit_id = egui::Id::new(("mm_range_edit", &row.target_id, matches!(row.scope, Scope::Session)));
                            let held: Option<[f32; 2]> = ui.data(|d| d.get_temp(edit_id));
                            let [mut lo, mut hi] = held.unwrap_or(row.range).map(|v| v * 100.0);
                            // A table cell lays out top down: the pair needs a row of its own, as
                            // tall as the table's row so the fields center in it.
                            ui.spacing_mut().interact_size.y = ROW_H;
                            let (a, b) = ui
                                .horizontal(|ui| {
                                    let a = crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut lo).range(0.0..=100.0).suffix("%").speed(0.5)));
                                    let b = crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut hi).range(0.0..=100.0).suffix("%").speed(0.5)));
                                    (a, b)
                                })
                                .inner;
                            let now = [lo / 100.0, hi / 100.0];
                            let moved = (now[0] - row.range[0]).abs() > 1e-4 || (now[1] - row.range[1]).abs() > 1e-4;
                            if a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus() {
                                ui.data_mut(|d| d.remove_temp::<[f32; 2]>(edit_id));
                                if moved {
                                    actions.push((i, Action::Range(now[0], now[1])));
                                }
                            } else if moved {
                                ui.data_mut(|d| d.insert_temp(edit_id, now));
                            } else {
                                ui.data_mut(|d| d.remove_temp::<[f32; 2]>(edit_id));
                            }
                        });
                        tr.col(|ui| {
                            if row.target_id.starts_with("transport:") {
                                cell(ui, "Consumed", ink);
                                return;
                            }
                            let mut thru = row.pass_through;
                            let check = crate::chrome::checkbox(
                                ui, &mut thru, "")
                                .on_hover_text("Also send the mapped message to the track (its instrument, a recording). Off: only the control hears it.");
                            crate::help::hover(ui, &check, crate::help::Id::MidiMappingsPassThrough);
                            if check.changed() {
                                actions.push((i, Action::PassThrough(thru)));
                            }
                        });
                        tr.col(|ui| {
                            let scope = match (row.scope, row.overridden) {
                                (Scope::Session, true) => "Session (overridden)",
                                (Scope::Session, false) => "Session",
                                (Scope::Profile, true) => "Controller (overridden)",
                                (Scope::Profile, false) => "Controller",
                            };
                            cell(ui, scope, ink);
                        });
                        tr.col(|ui| {
                            let label = if row.learning { "Listening..." } else { "Learn" };
                            let learn = ui.button(label).on_hover_text("Press a key or move a knob: it replaces this row's source.");
                            crate::help::hover(ui, &learn, crate::help::Id::MidiMappingsLearn);
                            if learn.clicked() {
                                actions.push((i, if row.learning { Action::Cancel } else { Action::Learn }));
                            }
                        });
                        tr.col(|ui| {
                            if ui.button("Delete").clicked() {
                                actions.push((i, Action::Delete));
                            }
                        });
                    });
                });
        });
    if let Some(r) = r {
        let rect = r.response.rect;
        app.midi_learn.window = mixer::r4(rect);
        app.ui.midi_mappings.size = window_size([rect.width(), rect.height()], bounds).into();
    }
    app.ui.midi_mappings.open = open;
    app.ui.midi_takeover = takeover;
    for (i, action) in actions {
        if let Some(row) = rows.get(i) {
            let _ = app.mapping_row_action(row, action);
        }
    }
}

/// What a row of the window can do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Learn,
    Cancel,
    Delete,
    Range(f32, f32),
    PassThrough(bool),
}

impl StudioApp {
    /// Does a window row's action through the engine commands.
    pub(crate) fn mapping_row_action(&mut self, row: &Row, action: Action) -> Result<Value, String> {
        let scope = row.scope;
        match action {
            Action::Learn => self.run("midi.learn", json!({"target": row.target_id, "scope": scope})),
            Action::Cancel => self.run("midi.learn_cancel", json!({})),
            Action::Delete => self.run("midi.unmap", json!({"target": row.target_id, "scope": scope})),
            Action::Range(..) | Action::PassThrough(_) => {
                let target: ControlTarget = row.target_id.parse()?;
                let set: &MappingSet = match scope {
                    Scope::Session => &self.engine.session().midi_mappings,
                    Scope::Profile => self.engine.midi_profile(),
                };
                let old = set.for_target(&target).cloned().ok_or("that mapping is gone")?;
                let (mode, lo, hi, pass_through) = match action {
                    Action::Range(lo, hi) => (old.mode, lo, hi, old.pass_through),
                    Action::PassThrough(p) => (old.mode, old.min, old.max, p),
                    _ => (old.mode, old.min, old.max, old.pass_through),
                };
                let channel = old.source.channel.map_or(json!("any"), |c| json!(c + 1));
                self.run(
                    "midi.map",
                    json!({
                        "source": {"port": old.source.port, "channel": channel, "kind": old.source.kind, "number": old.source.number},
                        "target": row.target_id,
                        "scope": scope,
                        "mode": mode,
                        "range": [lo, hi],
                        "toggle": old.toggle,
                        "pass_through": pass_through,
                    }),
                )
            }
        }
    }
}

//! Track printing and hidden-track recovery. Choices live only for the current session.

use crate::arrange::{R4, r4};
use crate::help::{Id, hover};
use crate::{Dialog, StudioApp};
use serde_json::{Value, json};
use transit_model::{ClipId, TrackId, TrackKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Source {
    Leave,
    #[default]
    Mute,
    MuteDeactivate,
    Hide,
}

impl Source {
    fn param(self) -> &'static str {
        match self {
            Self::Leave => "leave",
            Self::Mute => "mute",
            Self::MuteDeactivate => "mute_deactivate",
            Self::Hide => "hide",
        }
    }

    /// The engine's value for this choice.
    fn engine(self) -> transit_engine::io::BounceSource {
        use transit_engine::io::BounceSource as B;
        match self {
            Self::Leave => B::Leave,
            Self::Mute => B::Mute,
            Self::MuteDeactivate => B::MuteDeactivate,
            Self::Hide => B::Hide,
        }
    }

    const ALL: [Self; 4] = [Self::Leave, Self::Mute, Self::MuteDeactivate, Self::Hide];

    fn label(self) -> &'static str {
        match self {
            Self::Leave => "Leave",
            Self::Mute => "Mute",
            Self::MuteDeactivate => "Mute and deactivate",
            Self::Hide => "Hide",
        }
    }

    /// What the choice does to the source track, under the control.
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Leave => "The source track stays as it is.",
            Self::Mute => "Mutes the source track. Its plug-ins keep running.",
            Self::MuteDeactivate => "Mutes the source track and stops its plug-ins and instrument.",
            Self::Hide => "Deactivates the source track and hides it. Track > Show Hidden Tracks brings it back.",
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Leave => "left unchanged",
            Self::Mute => "muted",
            Self::MuteDeactivate => "muted and deactivated",
            Self::Hide => "hidden",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub source_track: Option<TrackId>,
    /// The selected clips of a clip bounce; empty for the track form.
    pub clips: Vec<ClipId>,
    pub existing: bool,
    pub destination: Option<TrackId>,
    pub source: Source,
    pub layout: Layout,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Layout {
    pub window: R4,
    pub span: R4,
    pub new_track: R4,
    pub existing_track: R4,
    pub dropdown: R4,
    pub destinations: Vec<(TrackId, R4)>,
    pub sources: Vec<R4>,
    pub bounce: R4,
    pub cancel: R4,
    /// The footer's two buttons as drawn: Bounce (the default style) and Cancel (standard).
    pub bounce_button: crate::chrome::ButtonLayout,
    pub cancel_button: crate::chrome::ButtonLayout,
    /// The section labels, top to bottom.
    pub sections: Vec<String>,
    /// The line under the Source control saying what the choice does, and its text.
    pub source_hint: R4,
    pub source_hint_text: String,
    /// The before-bar-1 warning card on the Source row (zero when it does not apply) and its text.
    pub zone_warning: R4,
    pub zone_warning_text: String,
    /// Whether the track picker takes input (only with Existing track).
    pub dropdown_enabled: bool,
    /// The range line's text (a clip bounce).
    pub span_text: String,
    pub hidden: Vec<(TrackId, String, R4)>,
    pub show_all: R4,
}

impl StudioApp {
    /// Open the form without touching the document or undo history.
    pub fn open_bounce_in_place(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(track) = self.focus_track() else {
            self.status = "Select a track to bounce in place.".into();
            return;
        };
        self.bounce_place.source_track = Some(track);
        self.bounce_place.clips.clear();
        self.finish_open();
    }

    /// Open the form for the selected clips (Edit and clip context menus).
    pub fn open_bounce_clips(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let picked = self.selected_clips();
        let Some(first) = picked.first() else {
            self.status = "Select a clip to bounce in place.".into();
            return;
        };
        self.bounce_place.source_track = Some(first.0);
        self.bounce_place.clips = picked.iter().map(|c| c.1).collect();
        self.finish_open();
    }

    fn finish_open(&mut self) {
        let eligible = self.bounce_destinations();
        if !eligible.iter().any(|(id, _)| Some(*id) == self.bounce_place.destination) {
            self.bounce_place.destination = eligible.first().map(|(id, _)| *id);
        }
        self.dialog = Some(Dialog::BounceInPlace);
    }

    /// The tracks the form prints: those of the selected clips, or the focused track.
    pub fn bounce_sources(&self) -> Vec<TrackId> {
        let s = self.engine.session();
        if self.bounce_place.clips.is_empty() {
            return self.bounce_place.source_track.into_iter().collect();
        }
        let mut out = Vec::new();
        for c in &self.bounce_place.clips {
            if let Some((t, _)) = s.find_clip(*c)
                && !out.contains(&t)
            {
                out.push(t);
            }
        }
        out
    }

    /// The line under the title of a clip bounce: how many clips on how many tracks, and the span
    /// in bars and beats as the loop brace in the ruler writes a range (`loopbar::range_text`; the
    /// pre-count zone counts down 0, -1, ...).
    pub fn bounce_span_line(&self) -> String {
        let s = self.engine.session();
        let clips: Vec<_> = self.bounce_place.clips.iter().filter_map(|c| s.find_clip(*c).map(|x| x.1)).collect();
        let (Some(start), Some(end)) = (clips.iter().map(|c| c.start).min(), clips.iter().map(|c| c.end()).max()) else { return String::new() };
        let tracks = self.bounce_sources().len();
        let (n, m) = (clips.len(), tracks);
        format!(
            "{n} clip{} on {m} track{}, {}",
            if n == 1 { "" } else { "s" },
            if m == 1 { "" } else { "s" },
            crate::loopbar::range_text(s, transit_time::Range::new(start, end))
        )
    }

    pub fn bounce_destinations(&self) -> Vec<(TrackId, String)> {
        let sources = self.bounce_sources();
        self.engine
            .session()
            .tracks
            .iter()
            .filter(|t| !sources.contains(&t.id) && t.kind == TrackKind::Audio && !t.hidden && t.freeze.is_none() && t.playlist().is_some())
            .map(|t| (t.id, t.name.clone()))
            .collect()
    }

    /// Apply the form as one engine command, then focus the printed track.
    pub fn bounce_in_place(&mut self) -> Result<Value, String> {
        let f = self.bounce_place.clone();
        let t = f.source_track.ok_or_else(|| "Select a track to bounce in place.".to_string())?;
        let sources = self.bounce_sources();
        let existing = f.existing && sources.len() == 1;
        let destination =
            if existing { json!(f.destination.ok_or_else(|| "Choose an eligible audio destination.".to_string())?.0) } else { json!("new") };
        let name = self.track_name(t);
        for id in &sources {
            if self.engine.session().track(*id).is_some_and(|tr| tr.inactive) {
                let why = transit_engine::io::inactive_source_refusal(&self.track_name(*id));
                self.status = why.clone();
                return Err(why);
            }
        }
        let result = if f.clips.is_empty() {
            self.run("track.bounce", json!({"tracks": [t.0], "destination": destination, "source": f.source.param()}))?
        } else {
            let clips: Vec<u64> = f.clips.iter().map(|c| c.0).collect();
            self.run("clip.bounce", json!({"clips": clips, "destination": destination, "source": f.source.param()}))?
        };
        if let Some(id) = result["tracks"][0].as_u64() {
            let dest = TrackId(id);
            self.focus(dest);
            let what = if f.clips.is_empty() || sources.len() == 1 { name } else { format!("{} tracks", sources.len()) };
            self.status = format!("Bounced {what} to {} (source {})", self.track_name(dest), f.source.status());
            // The engine's notice when the part before bar 1 stops playing (ADR 0052 decision 10).
            if let Some(notice) = result["notice"].as_str() {
                self.status = format!("{}. {notice}", self.status);
            }
        } else {
            self.status = format!("Nothing to bounce on {name}: it has no clips");
        }
        self.dialog = None;
        Ok(result)
    }

    pub fn open_hidden_tracks(&mut self) {
        if self.dialog.is_none() && self.engine.session().tracks.iter().any(|t| t.hidden) {
            self.dialog = Some(Dialog::HiddenTracks);
        }
    }
}

/// "Bounce in Place" for the selected clips, in the Edit menu and the clip context menu. Returns
/// the item for the menu registry; disabled with no clip selected.
pub fn clip_menu_item(app: &mut StudioApp, ui: &mut egui::Ui) -> crate::mixer::MenuItem {
    let has_clip = !app.engine.session().edit.selected_clips.is_empty();
    let b = ui.add_enabled(has_clip, crate::chrome::MenuRow::new("Bounce in Place")).on_disabled_hover_text("Select a clip first");
    hover(ui, &b, Id::BouncePlaceClipMenu);
    let item = crate::mixer::MenuItem::at(ui, "Bounce in Place".into(), String::new(), b.rect);
    if b.clicked() {
        app.open_bounce_clips();
        ui.close();
    }
    item
}

pub fn dialog(app: &mut StudioApp, ctx: &egui::Context) {
    app.bounce_place.layout = Layout::default();
    match app.dialog {
        Some(Dialog::BounceInPlace) => bounce_form(app, ctx),
        Some(Dialog::HiddenTracks) => hidden_form(app, ctx),
        _ => {}
    }
}

/// The width of the form's content, points. The window adds the tool window's 16 px body padding
/// a side and its 1 px edge: 422.
const BODY_W: f32 = 388.0;

/// A section label (`label.section`: Sans 600 11, uppercase, tracked, `text.muted`).
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(crate::tokens::LABEL_SECTION.job(ui.ctx(), text, crate::theme::TOKENS.text_muted));
}

/// The 1 px `line.inset` that splits a tool window's groups.
fn group_divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, crate::theme::TOKENS.line_inset);
}

/// A line of explanation under a control: Sans 11 in `text.faint`, wrapped to the body.
fn hint_line(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).size(11.0).color(crate::theme::TOKENS.text_faint))
}

fn rect_of(r: R4) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
}

/// A warning (README, Warnings and errors): the message in `text` after a triangle-alert, no
/// color, on a `surface.raised` card with a `line.strong` edge, in the Notice's radius and padding;
/// the text wraps to the card.
fn warning_card(ui: &mut egui::Ui, text: &str) -> egui::Rect {
    use crate::tokens::chrome as c;
    let t = &crate::theme::TOKENS;
    let [top, right, bottom, left] = c::NOTICE_PADDING;
    let edge = c::FIELD_EDGE_WIDTH;
    let frame = egui::Frame::new()
        .fill(t.surface_raised)
        .stroke(egui::Stroke::new(edge, t.line_strong))
        .corner_radius(c::NOTICE_RADIUS)
        .inner_margin(egui::Margin { left: left as i8, right: right as i8, top: top as i8, bottom: bottom as i8 });
    let width = ui.available_width() - left - right - 2.0 * edge;
    frame
        .show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = crate::theme::SPACE;
                let (glyph, _) = ui.allocate_exact_size(egui::vec2(c::NOTICE_ICON_SIZE, c::NOTICE_ICON_SIZE), egui::Sense::hover());
                crate::icons::paint(ui.painter(), glyph.center(), crate::chrome::Severity::Warning.icon(), c::NOTICE_ICON_SIZE, t.text);
                let font = crate::tokens::LABEL_CONTROL.font_id(ui.ctx());
                ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(t.text)).wrap());
            });
        })
        .response
        .rect
}

fn bounce_form(app: &mut StudioApp, ctx: &egui::Context) {
    use crate::widgets::segmented;
    let eligible = app.bounce_destinations();
    let span = app.bounce_span_line();
    let sources = app.bounce_sources();
    let several = sources.len() > 1;
    let session = app.engine.session_arc();
    let mut f = app.bounce_place.clone();
    let mut layout = Layout::default();
    // Escape closes through the window's close flag (the shared modal rule: the top dialog only,
    // and not while the destination list is open); Enter is read before the body, as Escape is.
    let (mut go, mut cancel) = (false, false);
    let enter = crate::modal::enter(ctx, crate::theme::modal_window_layer("Bounce in Place"));
    let mut open = true;
    let id = egui::Id::new("bounce_in_place");
    let response = crate::theme::modal_window(ctx, "Bounce in Place")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(BODY_W);
            ui.spacing_mut().item_spacing = egui::vec2(crate::theme::SPACE, crate::theme::SPACE);
            if !f.clips.is_empty() {
                let font = crate::tokens::LABEL_CONTROL.font_id(ui.ctx());
                let line = ui.label(egui::RichText::new(&span).font(font).color(crate::theme::TOKENS.text_muted));
                hover(ui, &line, Id::BouncePlaceSpan);
                layout.span = r4(line.rect);
                layout.span_text = span.clone();
            }
            section_label(ui, "Destination");
            layout.sections.push("Destination".into());
            let can_choose = !several;
            ui.horizontal(|ui| {
                let chosen = usize::from(f.existing && can_choose);
                let seg = ui
                    .add_enabled_ui(can_choose, |ui| segmented::show(ui, id.with("destination"), &["New track", "Existing track"], chosen, None))
                    .inner;
                if let Some(i) = seg.picked {
                    f.existing = i == 1;
                }
                layout.new_track = seg.layout.segments[0];
                layout.existing_track = seg.layout.segments[1];
                let track = ui.interact(rect_of(seg.layout.track), id.with("destination_hover"), egui::Sense::hover());
                hover(ui, &track, Id::BouncePlaceDestination);
                if several {
                    track.on_hover_text("One new track per source track");
                }
                let picker = f.existing && can_choose;
                layout.dropdown_enabled = picker;
                let width = ui.available_width().max(80.0);
                ui.add_enabled_ui(picker, |ui| {
                    let label = eligible.iter().find(|(id, _)| Some(*id) == f.destination).map_or("Choose an audio track", |(_, name)| name.as_str());
                    let combo = crate::chrome::field(ui, |ui| {
                        crate::chrome_pick::Combo::new("bounce_destination", label).width(width).show(ui, |ui| {
                            for (id, name) in &eligible {
                                let b = crate::chrome_pick::choice_value(ui, &mut f.destination, Some(*id), name);
                                layout.destinations.push((*id, r4(b.rect)));
                            }
                        })
                    });
                    layout.dropdown = r4(combo.response.rect);
                    hover(ui, &combo.response, Id::BouncePlaceDestination);
                });
            });
            if f.existing && can_choose && eligible.is_empty() {
                hint_line(ui, "No eligible audio tracks");
            }
            group_divider(ui);
            section_label(ui, "Source");
            layout.sections.push("Source".into());
            let labels: Vec<&str> = Source::ALL.iter().map(|s| s.label()).collect();
            let chosen = Source::ALL.iter().position(|s| *s == f.source).unwrap_or(0);
            let seg = segmented::show(ui, id.with("source"), &labels, chosen, None);
            if let Some(i) = seg.picked {
                f.source = Source::ALL[i];
            }
            layout.sources = seg.layout.segments.clone();
            let track = ui.interact(rect_of(seg.layout.track), id.with("source_hover"), egui::Sense::hover());
            hover(ui, &track, Id::BouncePlaceSource);
            let hint = hint_line(ui, f.source.hint());
            layout.source_hint = r4(hint.rect);
            layout.source_hint_text = f.source.hint().into();
            // The notice the bounce will put in the status line, said before the user confirms. It goes
            // when audio clips can start before bar 1 (#442), with the engine's notice.
            if let Some(notice) = transit_engine::io::zone_silenced_notice(&session, &sources, f.source.engine(), f.clips.is_empty()) {
                layout.zone_warning = r4(warning_card(ui, notice));
                layout.zone_warning_text = notice.into();
            }
            let ready = !f.existing || several || eligible.iter().any(|(id, _)| Some(*id) == f.destination);
            crate::chrome::tool_footer(ui, |ui| {
                ui.spacing_mut().item_spacing.x = crate::theme::SPACE;
                let (b, bl) = ui.add_enabled_ui(ready, |ui| crate::chrome::default_button(ui, "Bounce")).inner;
                layout.bounce = bl.rect;
                layout.bounce_button = bl;
                hover(ui, &b, Id::BouncePlaceApply);
                // An open dropdown keeps Enter to select an item first (`modal::held`).
                go = b.clicked() || (ready && enter);
                let (b, cl) = crate::chrome::button(ui, "Cancel");
                layout.cancel = cl.rect;
                layout.cancel_button = cl;
                hover(ui, &b, Id::BouncePlaceCancel);
                cancel |= b.clicked();
            });
        });
    if let Some(r) = response {
        layout.window = r4(r.response.rect);
    }
    f.layout = layout;
    app.bounce_place = f;
    if cancel || !open {
        app.dialog = None;
    } else if go && let Err(why) = app.bounce_in_place() {
        app.status = why;
    }
}

fn hidden_form(app: &mut StudioApp, ctx: &egui::Context) {
    let hidden: Vec<_> = app.engine.session().tracks.iter().filter(|t| t.hidden).map(|t| (t.id, t.name.clone())).collect();
    let mut show = Vec::new();
    let mut close = crate::modal::escape(ctx, crate::modal::layer_of(egui::Id::new("hidden_tracks")));
    let mut layout = Layout::default();
    let response = crate::chrome::raw_modal(ctx, egui::Id::new("hidden_tracks")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading("Show Hidden Tracks");
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for (id, name) in &hidden {
                ui.push_id(id.0, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        let b = ui.button("Show");
                        hover(ui, &b, Id::HiddenTracksShow);
                        layout.hidden.push((*id, name.clone(), r4(b.rect)));
                        if b.clicked() {
                            show.push(*id);
                        }
                    })
                });
            }
        });
        ui.horizontal(|ui| {
            let b = ui.add_enabled(!hidden.is_empty(), egui::Button::new("Show All"));
            hover(ui, &b, Id::HiddenTracksShowAll);
            layout.show_all = r4(b.rect);
            if b.clicked() {
                show = hidden.iter().map(|(id, _)| *id).collect();
            }
            let b = ui.button("Close");
            hover(ui, &b, Id::HiddenTracksClose);
            close |= b.clicked();
        });
    });
    layout.window = r4(response.response.rect);
    app.bounce_place.layout = layout;
    if !show.is_empty() {
        let _ = app.run("track.hide", json!({"tracks": show, "hidden": false}));
    }
    if close || !app.engine.session().tracks.iter().any(|t| t.hidden) {
        app.dialog = None;
    }
}

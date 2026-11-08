//! Track printing and hidden-track recovery. Choices live only for the current session.

use crate::arrange::{R4, r4};
use crate::help::{Id, hover};
use crate::shortcuts::{Action, hit};
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

    /// The line under the title of a clip bounce: how many clips on how many tracks, and the span.
    pub fn bounce_span_line(&self) -> String {
        let s = self.engine.session();
        let clips: Vec<_> = self.bounce_place.clips.iter().filter_map(|c| s.find_clip(*c).map(|x| x.1)).collect();
        let (Some(start), Some(end)) = (clips.iter().map(|c| c.start).min(), clips.iter().map(|c| c.end()).max()) else { return String::new() };
        let fmt = s.edit.main_counter;
        let tracks = self.bounce_sources().len();
        let (n, m) = (clips.len(), tracks);
        format!(
            "{n} clip{} on {m} track{}, {} to {}",
            if n == 1 { "" } else { "s" },
            if m == 1 { "" } else { "s" },
            s.format_position(start, fmt),
            s.format_position(end, fmt)
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

fn bounce_form(app: &mut StudioApp, ctx: &egui::Context) {
    let eligible = app.bounce_destinations();
    let span = app.bounce_span_line();
    let several = app.bounce_sources().len() > 1;
    let mut f = app.bounce_place.clone();
    let mut layout = Layout::default();
    let (mut go, mut cancel) = (false, ctx.input(|i| hit(i, Action::DialogCancel)));
    let response = egui::Modal::new(egui::Id::new("bounce_in_place")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.spacing_mut().item_spacing = egui::vec2(crate::theme::SPACE, crate::theme::SPACE);
        ui.heading("Bounce in Place");
        if !f.clips.is_empty() {
            let line = ui.label(&span);
            hover(ui, &line, Id::BouncePlaceSpan);
            layout.span = r4(line.rect);
        }
        ui.label("Destination");
        let new = crate::chrome::radio_value(ui, &mut f.existing, false, "New track");
        hover(ui, &new, Id::BouncePlaceDestination);
        layout.new_track = r4(new.rect);
        let existing = crate::chrome::radio_enabled(ui, !several, f.existing && !several, "Existing track");
        if existing.clicked() {
            f.existing = true;
        }
        let existing = existing.on_disabled_hover_text("One new track per source track");
        hover(ui, &existing, Id::BouncePlaceDestination);
        layout.existing_track = r4(existing.rect);
        ui.add_enabled_ui(f.existing && !several, |ui| {
            let label = eligible.iter().find(|(id, _)| Some(*id) == f.destination).map_or("Choose an audio track", |(_, name)| name.as_str());
            let combo = crate::chrome::field(ui, |ui| {
                crate::chrome_pick::Combo::new("bounce_destination", label).show(ui, |ui| {
                    for (id, name) in &eligible {
                        let b = crate::chrome_pick::choice_value(ui, &mut f.destination, Some(*id), name);
                        layout.destinations.push((*id, r4(b.rect)));
                    }
                })
            });
            layout.dropdown = r4(combo.response.rect);
            hover(ui, &combo.response, Id::BouncePlaceDestination);
            if eligible.is_empty() {
                ui.label("No eligible audio tracks");
            }
        });
        ui.label("Source");
        ui.horizontal_wrapped(|ui| {
            let choices = [(Source::Leave, "Leave"), (Source::Mute, "Mute"), (Source::MuteDeactivate, "Mute and deactivate"), (Source::Hide, "Hide")];
            for (value, label) in choices {
                let b = crate::chrome::radio_value(ui, &mut f.source, value, label);
                layout.sources.push(r4(b.rect));
                hover(ui, &b, Id::BouncePlaceSource);
            }
        });
        let ready = !f.existing || several || eligible.iter().any(|(id, _)| Some(*id) == f.destination);
        ui.horizontal(|ui| {
            let b = ui.add_enabled(ready, egui::Button::new("Bounce"));
            layout.bounce = r4(b.rect);
            hover(ui, &b, Id::BouncePlaceApply);
            // Let an open dropdown consume Enter to select an item first.
            go = b.clicked() || (ready && !egui::Popup::is_any_open(ctx) && ui.input(|i| hit(i, Action::DialogConfirm)));
            let b = ui.button("Cancel");
            layout.cancel = r4(b.rect);
            hover(ui, &b, Id::BouncePlaceCancel);
            cancel |= b.clicked();
        });
    });
    layout.window = r4(response.response.rect);
    f.layout = layout;
    app.bounce_place = f;
    if cancel {
        app.dialog = None;
    } else if go && let Err(why) = app.bounce_in_place() {
        app.status = why;
    }
}

fn hidden_form(app: &mut StudioApp, ctx: &egui::Context) {
    let hidden: Vec<_> = app.engine.session().tracks.iter().filter(|t| t.hidden).map(|t| (t.id, t.name.clone())).collect();
    let mut show = Vec::new();
    let mut close = ctx.input(|i| hit(i, Action::DialogCancel));
    let mut layout = Layout::default();
    let response = egui::Modal::new(egui::Id::new("hidden_tracks")).show(ctx, |ui| {
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

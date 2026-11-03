//! Track printing and hidden-track recovery. Choices live only for the current session.

use crate::arrange::{R4, r4};
use crate::help::{Id, hover};
use crate::shortcuts::{Action, hit};
use crate::{Dialog, StudioApp};
use serde_json::{Value, json};
use transit_model::{TrackId, TrackKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Source {
    Leave,
    #[default]
    Mute,
    Hide,
}

impl Source {
    fn param(self) -> &'static str {
        match self {
            Self::Leave => "leave",
            Self::Mute => "mute",
            Self::Hide => "hide",
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Leave => "left unchanged",
            Self::Mute => "muted",
            Self::Hide => "hidden",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub source_track: Option<TrackId>,
    pub existing: bool,
    pub destination: Option<TrackId>,
    pub source: Source,
    pub layout: Layout,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Layout {
    pub window: R4,
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
        let eligible = self.bounce_destinations();
        if !eligible.iter().any(|(id, _)| Some(*id) == self.bounce_place.destination) {
            self.bounce_place.destination = eligible.first().map(|(id, _)| *id);
        }
        self.dialog = Some(Dialog::BounceInPlace);
    }

    pub fn bounce_destinations(&self) -> Vec<(TrackId, String)> {
        self.engine
            .session()
            .tracks
            .iter()
            .filter(|t| {
                Some(t.id) != self.bounce_place.source_track
                    && t.kind == TrackKind::Audio
                    && !t.hidden
                    && t.freeze.is_none()
                    && t.playlist().is_some()
            })
            .map(|t| (t.id, t.name.clone()))
            .collect()
    }

    /// Apply the form as one engine command, then focus the printed track.
    pub fn bounce_in_place(&mut self) -> Result<Value, String> {
        let f = self.bounce_place.clone();
        let t = f.source_track.ok_or_else(|| "Select a track to bounce in place.".to_string())?;
        let destination =
            if f.existing { json!(f.destination.ok_or_else(|| "Choose an eligible audio destination.".to_string())?.0) } else { json!("new") };
        let name = self.track_name(t);
        let result = self.run("track.bounce", json!({"tracks": [t.0], "destination": destination, "source": f.source.param()}))?;
        if let Some(id) = result["tracks"][0].as_u64() {
            let dest = TrackId(id);
            self.focus(dest);
            self.status = format!("Bounced {name} to {} (source {})", self.track_name(dest), f.source.status());
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
    let mut f = app.bounce_place.clone();
    let mut layout = Layout::default();
    let (mut go, mut cancel) = (false, ctx.input(|i| hit(i, Action::DialogCancel)));
    let response = egui::Modal::new(egui::Id::new("bounce_in_place")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.spacing_mut().item_spacing = egui::vec2(crate::theme::SPACE, crate::theme::SPACE);
        ui.heading("Bounce in Place");
        ui.label("Destination");
        let new = ui.radio_value(&mut f.existing, false, "New track");
        hover(ui, &new, Id::BouncePlaceDestination);
        layout.new_track = r4(new.rect);
        let existing = ui.radio_value(&mut f.existing, true, "Existing track");
        hover(ui, &existing, Id::BouncePlaceDestination);
        layout.existing_track = r4(existing.rect);
        ui.add_enabled_ui(f.existing, |ui| {
            let label = eligible.iter().find(|(id, _)| Some(*id) == f.destination).map_or("Choose an audio track", |(_, name)| name.as_str());
            let combo = egui::ComboBox::from_id_salt("bounce_destination").selected_text(label).show_ui(ui, |ui| {
                for (id, name) in &eligible {
                    let b = ui.selectable_value(&mut f.destination, Some(*id), name);
                    layout.destinations.push((*id, r4(b.rect)));
                }
            });
            layout.dropdown = r4(combo.response.rect);
            hover(ui, &combo.response, Id::BouncePlaceDestination);
            if eligible.is_empty() {
                ui.label("No eligible audio tracks");
            }
        });
        ui.label("Source");
        ui.horizontal(|ui| {
            for (value, label) in [(Source::Leave, "Leave"), (Source::Mute, "Mute"), (Source::Hide, "Hide")] {
                let b = ui.radio_value(&mut f.source, value, label);
                layout.sources.push(r4(b.rect));
                hover(ui, &b, Id::BouncePlaceSource);
            }
        });
        let ready = !f.existing || eligible.iter().any(|(id, _)| Some(*id) == f.destination);
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

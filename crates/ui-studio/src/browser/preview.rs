//! The Files tab's preview (issue #211): hear a file before dragging it in.
//!
//! Convention (Ableton and Logic agree, so transit follows them): a play control on the selected
//! file's row and a headphone toggle that previews each file as it is selected; the preview has
//! its own volume (Ableton's Preview Volume on the cue path, Logic's preview volume) and never
//! plays through a track, so it does not record, meter or route anywhere; it stops when another
//! file is selected, when the transport starts, on Escape, and at the end of the file. A footer
//! shows the file's waveform, length, rate and channels. Ableton's tempo-synced preview of warped
//! clips is not built.
//!
//! The engine side is `transit_playback::preview` (outside the plan; the document is never
//! touched: no undo step, no source). The toggle and the volume are view preferences in
//! [`super::BrowserPrefs`]. The footer's waveform is read on a background thread with the same
//! decoder the import uses; the UI thread never reads a file. The control channel drives it with
//! `preview.play {path?}`, `preview.stop`, `preview.volume {db}`, `preview.auto {on?}` and
//! `preview.status`.

use super::{FileInfo, Kind, file_name, kind_of, r4};
use crate::StudioApp;
use crate::arrange::R4;
use crate::icons::Icon;
use crate::theme::TOKENS;
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use transit_playback::preview::{MAX_DB, MIN_DB, PreviewState};

/// Height of the Files tab's preview footer.
pub const FOOTER_H: f32 = 76.0;
/// The preview volume a new install starts at.
pub const DEFAULT_DB: f32 = -6.0;
/// Columns of a footer waveform.
const BINS: usize = 240;
/// Overviews kept in memory.
const MAX_OVERVIEWS: usize = 64;

/// A file's waveform for the footer: (min, max) per column over every channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub bins: Vec<(f32, f32)>,
}

/// The footer as last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PreviewLayout {
    pub footer: R4,
    pub play: R4,
    pub auto: R4,
    pub volume: R4,
    pub wave: R4,
    /// The file the footer shows (the previewing one, else the single selected audio file).
    pub file: Option<String>,
    /// Its length, rate and channels, from the header.
    pub facts: String,
    /// "idle", "loading", "playing", "silent" (no audio device) or "failed".
    pub state: &'static str,
    pub auto_on: bool,
    pub db: f32,
    pub overview: bool,
}

/// The preview's UI state (never saved).
pub struct PreviewUi {
    /// The file previewing now; `None` when nothing plays.
    pub path: Option<PathBuf>,
    pub state: &'static str,
    /// Played fraction of the file, for the footer's playhead.
    pub progress: f32,
    /// The selection last seen, to notice another file being selected.
    last_selected: Vec<PathBuf>,
    was_playing: bool,
    overviews: HashMap<PathBuf, Option<Overview>>,
    pending: Option<PathBuf>,
    tx: Option<Sender<PathBuf>>,
    rx: Option<Receiver<(PathBuf, Option<Overview>)>>,
    /// Overviews read since launch.
    pub overviews_read: u64,
}

impl Default for PreviewUi {
    fn default() -> Self {
        PreviewUi {
            path: None,
            state: "idle",
            progress: 0.0,
            last_selected: Vec::new(),
            was_playing: false,
            overviews: HashMap::new(),
            pending: None,
            tx: None,
            rx: None,
            overviews_read: 0,
        }
    }
}

impl PreviewUi {
    /// Ask the overview thread for `path`'s waveform (the newest ask wins).
    fn request_overview(&mut self, path: &Path, ctx: &egui::Context) {
        if self.overviews.contains_key(path) || self.pending.as_deref() == Some(path) {
            return;
        }
        if self.tx.is_none() {
            let (tx, jobs) = channel::<PathBuf>();
            let (out, rx) = channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("browser-overview".into()).spawn(move || {
                while let Ok(mut p) = jobs.recv() {
                    // Only the newest file matters: the user has moved on from the others.
                    while let Ok(newer) = jobs.try_recv() {
                        p = newer;
                    }
                    let o = transit_playback::preview::overview(&p, BINS).map(|bins| Overview { bins });
                    if out.send((p, o)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
            if spawned.is_err() {
                return;
            }
            (self.tx, self.rx) = (Some(tx), Some(rx));
        }
        if let Some(tx) = &self.tx
            && tx.send(path.to_path_buf()).is_ok()
        {
            self.pending = Some(path.to_path_buf());
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((p, o)) = rx.try_recv() {
            if self.pending.as_deref() == Some(p.as_path()) {
                self.pending = None;
            }
            if self.overviews.len() >= MAX_OVERVIEWS {
                self.overviews.clear();
            }
            self.overviews.insert(p, o);
            self.overviews_read += 1;
        }
    }

    /// The overview of `path`, once read (`Some(None)`: it could not be read).
    pub fn overview(&self, path: &Path) -> Option<Option<&Overview>> {
        self.overviews.get(path).map(Option::as_ref)
    }
}

fn state_name(s: PreviewState) -> &'static str {
    match s {
        PreviewState::Idle | PreviewState::Ended | PreviewState::Cut => "idle",
        PreviewState::Loading => "loading",
        PreviewState::Playing => "playing",
        PreviewState::Failed => "failed",
    }
}

impl StudioApp {
    /// Preview `path` from its start (the row's play control, the footer's, auto-preview).
    pub fn preview_play(&mut self, path: &Path) -> Result<(), String> {
        if kind_of(&path.to_string_lossy(), false) != Some(Kind::Audio) {
            self.status = format!("{}: not an audio file this app previews", file_name(path));
            return Err(self.status.clone());
        }
        let db = self.ui.browser.preview_db;
        let pv = &mut self.browser.preview;
        pv.path = Some(path.to_path_buf());
        pv.progress = 0.0;
        match self.player.as_ref().and_then(transit_playback::Player::preview) {
            Some(p) => {
                p.set_volume_db(db);
                p.play(path);
                pv.state = "loading";
            }
            // No audio device: nothing can be heard (the footer says so).
            None => pv.state = "silent",
        }
        Ok(())
    }

    /// Stop the preview (Escape, the stop control, another file selected, the transport).
    pub fn preview_stop(&mut self) {
        if let Some(p) = self.player.as_ref().and_then(transit_playback::Player::preview) {
            p.stop();
        }
        let pv = &mut self.browser.preview;
        (pv.path, pv.state, pv.progress) = (None, "idle", 0.0);
    }

    /// The preview volume in dB (a view preference).
    pub fn set_preview_db(&mut self, db: f32) {
        let db = if db.is_finite() { db.clamp(MIN_DB, MAX_DB) } else { DEFAULT_DB };
        self.ui.browser.preview_db = db;
        if let Some(p) = self.player.as_ref().and_then(transit_playback::Player::preview) {
            p.set_volume_db(db);
        }
    }

    /// The `preview.*` commands of the control channel.
    pub(crate) fn preview_command(&mut self, id: &str, p: &Value) -> Result<Value, String> {
        match id {
            "preview.play" => {
                let path = match p.get("path").and_then(Value::as_str) {
                    Some(s) => PathBuf::from(s),
                    None => match self.browser.selected.as_slice() {
                        [one] => one.clone(),
                        _ => return Err("preview.play needs a path, or one selected file".into()),
                    },
                };
                self.preview_play(&path)?;
            }
            "preview.stop" => self.preview_stop(),
            "preview.volume" => {
                let db = p.get("db").and_then(Value::as_f64).ok_or("preview.volume needs db")?;
                self.set_preview_db(db as f32);
            }
            "preview.auto" => {
                let on = p.get("on").and_then(Value::as_bool).unwrap_or(!self.ui.browser.auto_preview);
                self.ui.browser.auto_preview = on;
            }
            "preview.status" => {}
            _ => return Err(format!("unknown command: {id}")),
        }
        Ok(self.preview_json())
    }

    pub(crate) fn preview_json(&self) -> Value {
        let pv = &self.browser.preview;
        json!({
            "path": pv.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "state": pv.state,
            "auto": self.ui.browser.auto_preview,
            "db": self.ui.browser.preview_db,
        })
    }

    /// The diagnostics report's line.
    pub(crate) fn preview_line(&self) -> String {
        match &self.browser.preview.path {
            Some(p) => format!("Preview playing: {} ({})", file_name(p), self.browser.preview.state),
            None => "Preview playing: no".into(),
        }
    }

    /// Every frame, before the panel draws: follow the engine, stop on a transport start, follow
    /// the selection.
    pub(crate) fn preview_frame(&mut self) {
        self.browser.preview.poll();
        if self.browser.preview.path.is_some()
            && let Some(p) = self.player.as_ref().and_then(transit_playback::Player::preview)
        {
            let st = p.status();
            let pv = &mut self.browser.preview;
            pv.state = state_name(st.state);
            if let Some(f) = st.file.filter(|f| f.frames > 0 && f.rate > 0) {
                let length = f.frames as f64 * f64::from(st.rate) / f64::from(f.rate);
                pv.progress = (st.played as f64 / length).clamp(0.0, 1.0) as f32;
            }
            match st.state {
                PreviewState::Ended | PreviewState::Cut | PreviewState::Idle => pv.path = None,
                PreviewState::Failed => {
                    pv.path = None;
                    self.status = format!("Preview: {}", st.error.unwrap_or_default());
                }
                PreviewState::Loading | PreviewState::Playing => {}
            }
        }
        // The transport starting stops it (the engine stops it too, on the audio thread).
        let playing = self.is_playing();
        if playing && !self.browser.preview.was_playing && self.browser.preview.path.is_some() {
            self.preview_stop();
        }
        self.browser.preview.was_playing = playing;
        self.preview_follow_selection();
    }

    /// Another file selected stops the preview; with auto-preview on, a single selected audio
    /// file starts.
    pub(crate) fn preview_follow_selection(&mut self) {
        if self.browser.selected == self.browser.preview.last_selected {
            return;
        }
        self.browser.preview.last_selected = self.browser.selected.clone();
        let only = match self.browser.selected.as_slice() {
            [one] if kind_of(&one.to_string_lossy(), false) == Some(Kind::Audio) => Some(one.clone()),
            _ => None,
        };
        if self.browser.preview.path.is_some() && self.browser.preview.path != only {
            self.preview_stop();
        }
        if self.ui.browser.auto_preview
            && let Some(one) = only
            && self.browser.preview.path.as_ref() != Some(&one)
        {
            let _ = self.preview_play(&one);
        }
    }

    /// The row's or the footer's play control: stop `path` if it previews, else preview it.
    pub(crate) fn preview_toggle(&mut self, path: &Path) {
        if self.browser.preview.path.as_deref() == Some(path) {
            self.preview_stop();
        } else {
            let _ = self.preview_play(path);
        }
    }
}

/// The footer of the Files tab in `area`: play control, file name, headphone toggle, waveform,
/// facts and the preview volume.
pub(super) fn footer(app: &mut StudioApp, ui: &mut Ui, area: Rect, info: Option<FileInfo>) -> PreviewLayout {
    let p = ui.painter().with_clip_rect(area);
    p.line_segment([area.left_top(), area.right_top()], Stroke::new(1.0, TOKENS.line));
    let shown = app.browser.preview.path.clone().or_else(|| match app.browser.selected.as_slice() {
        [one] if kind_of(&one.to_string_lossy(), false) == Some(Kind::Audio) => Some(one.clone()),
        _ => None,
    });
    let mut out = PreviewLayout {
        footer: r4(area),
        file: shown.as_ref().map(|s| s.to_string_lossy().into_owned()),
        state: app.browser.preview.state,
        auto_on: app.ui.browser.auto_preview,
        db: app.ui.browser.preview_db,
        ..PreviewLayout::default()
    };
    let x0 = area.min.x + 8.0;
    let x1 = area.max.x - 10.0;
    // Row 1: play, name, headphones.
    let y = area.min.y + 5.0;
    let play = Rect::from_min_size(pos2(x0, y), vec2(22.0, 20.0));
    let playing = shown.is_some() && app.browser.preview.path == shown;
    let resp = ui.interact(play, ui.id().with("browser_preview_play"), Sense::click());
    p.rect_filled(play, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    let ink = if shown.is_some() { TOKENS.text } else { TOKENS.text_dim.gamma_multiply(0.6) };
    crate::icons::paint(&p, play.center(), if playing { Icon::Stop } else { Icon::Play }, 11.0, ink);
    if resp.on_hover_text(if playing { "Stop the preview (Esc)" } else { "Preview" }).clicked()
        && let Some(s) = &shown
    {
        app.preview_toggle(s);
    }
    out.play = r4(play);
    let auto = Rect::from_min_size(pos2(x1 - 22.0, y), vec2(22.0, 20.0));
    let aresp = ui.interact(auto, ui.id().with("browser_preview_auto"), Sense::click());
    let on = app.ui.browser.auto_preview;
    let fill = if on {
        TOKENS.accent.gamma_multiply(0.45)
    } else if aresp.hovered() {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(auto, crate::theme::RADIUS, fill);
    crate::icons::paint(&p, auto.center(), Icon::Headphones, 13.0, if on { TOKENS.text } else { TOKENS.text_dim });
    if aresp.on_hover_text("Auto-preview the selected file").clicked() {
        app.ui.browser.auto_preview = !on;
        out.auto_on = !on;
    }
    out.auto = r4(auto);
    let name = shown.as_ref().map_or_else(|| "Select an audio file to preview".to_string(), |s| file_name(s));
    let name_r = Rect::from_min_max(pos2(play.max.x + 8.0, y), pos2(auto.min.x - 6.0, y + 20.0));
    let font = FontId::proportional(11.5);
    let elided = super::elide(&p, &name, &font, name_r.width());
    p.with_clip_rect(name_r).text(
        name_r.left_center(),
        Align2::LEFT_CENTER,
        elided,
        font,
        if shown.is_some() { TOKENS.text } else { TOKENS.text_dim },
    );
    // Row 2: the waveform, and the playhead while it plays.
    let wave = Rect::from_min_max(pos2(x0, y + 25.0), pos2(x1, y + 49.0));
    p.rect_filled(wave, crate::theme::RADIUS_SM, TOKENS.lane);
    if let Some(s) = &shown {
        match app.browser.preview.overview(s) {
            Some(Some(o)) => {
                draw_overview(&p, wave, o);
                out.overview = true;
            }
            Some(None) => {}
            None => app.browser.preview.request_overview(s, ui.ctx()),
        }
        if playing {
            let x = wave.min.x + wave.width() * app.browser.preview.progress;
            p.line_segment([pos2(x, wave.min.y), pos2(x, wave.max.y)], Stroke::new(1.5, TOKENS.cursor));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
    out.wave = r4(wave);
    // Row 3: facts, and the preview volume.
    let y3 = wave.max.y + 3.0;
    out.facts = info.filter(|_| shown.is_some()).map(facts).unwrap_or_default();
    let note = match app.browser.preview.state {
        "silent" if playing => "No audio device".to_string(),
        "loading" if playing => "Loading…".to_string(),
        _ => out.facts.clone(),
    };
    p.text(pos2(x0, y3 + 8.0), Align2::LEFT_CENTER, note, FontId::proportional(10.5), TOKENS.text_dim);
    let vol = Rect::from_min_max(pos2(x1 - 110.0, y3 + 2.0), pos2(x1, y3 + 14.0));
    volume(app, ui, &p, vol);
    out.volume = r4(vol);
    out.db = app.ui.browser.preview_db;
    out
}

/// "0:12 · 48 kHz · stereo".
fn facts(i: FileInfo) -> String {
    let rate = if i.rate.is_multiple_of(1000) { format!("{} kHz", i.rate / 1000) } else { format!("{:.1} kHz", f64::from(i.rate) / 1000.0) };
    let label = i.label();
    match label.split_once(" · ") {
        Some((len, ch)) => format!("{len} · {rate} · {ch}"),
        None => label,
    }
}

fn draw_overview(p: &egui::Painter, r: Rect, o: &Overview) {
    let mid = r.center().y;
    let half = r.height() / 2.0 - 1.0;
    let n = o.bins.len().max(1);
    for (i, (lo, hi)) in o.bins.iter().enumerate() {
        let x = r.min.x + (i as f32 + 0.5) * r.width() / n as f32;
        let (top, bottom) = (mid - hi.clamp(-1.0, 1.0) * half, mid - lo.clamp(-1.0, 1.0) * half);
        p.line_segment([pos2(x, top), pos2(x, bottom.max(top + 1.0))], Stroke::new(1.0, TOKENS.accent.gamma_multiply(0.85)));
    }
}

/// The preview volume: drag along the bar, wheel over it, double-click for the default.
fn volume(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, r: Rect) {
    let resp = ui.interact(r, ui.id().with("browser_preview_volume"), Sense::click_and_drag());
    let db = app.ui.browser.preview_db;
    let span = MAX_DB - MIN_DB;
    let label_w = 46.0;
    let bar = Rect::from_min_max(pos2(r.min.x + label_w, r.center().y - 2.0), pos2(r.max.x, r.center().y + 2.0));
    p.rect_filled(bar, 2.0, TOKENS.button);
    let x = bar.min.x + bar.width() * (db - MIN_DB) / span;
    p.rect_filled(Rect::from_min_max(bar.min, pos2(x, bar.max.y)), 2.0, TOKENS.accent);
    p.circle_filled(pos2(x, bar.center().y), 4.0, TOKENS.text);
    let text = if db <= MIN_DB { "-∞ dB".to_string() } else { format!("{db:+.1} dB") };
    p.text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, text, FontId::proportional(10.0), TOKENS.text_dim);
    let mut next = db;
    if resp.double_clicked() {
        next = DEFAULT_DB;
    } else if (resp.dragged() || resp.clicked())
        && let Some(at) = resp.interact_pointer_pos()
    {
        next = MIN_DB + span * ((at.x - bar.min.x) / bar.width().max(1.0)).clamp(0.0, 1.0);
    }
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            next = db + dy.signum();
        }
    }
    let _ = resp.on_hover_text("Preview volume");
    if next != db {
        app.set_preview_db(next);
    }
}

/// The selected row's play control at `r` (the row's right end): returns its rect when drawn.
pub(super) fn row_play(app: &StudioApp, ui: &mut Ui, p: &egui::Painter, row: Rect, path: &Path, clicked: &mut Option<PathBuf>) -> R4 {
    let b = Rect::from_center_size(pos2(row.max.x - 16.0, row.center().y), vec2(16.0, 16.0));
    let resp = ui.interact(b, ui.id().with(("browser_row_play", path)), Sense::click());
    let playing = app.browser.preview.path.as_deref() == Some(path);
    let ink = if resp.hovered() || playing { TOKENS.text } else { TOKENS.text_dim };
    crate::icons::paint(p, b.center(), if playing { Icon::Stop } else { Icon::Play }, 10.0, ink);
    if resp.on_hover_text(if playing { "Stop the preview" } else { "Preview" }).clicked() {
        *clicked = Some(path.to_path_buf());
    }
    r4(b)
}

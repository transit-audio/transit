//! MIDI Learn in the studio (ADR 0028, `docs/contributing/knowledge/midi-learn.md`): right-click a
//! mixer control to learn a controller knob or button onto it, the marker on mapped controls, and
//! Window > MIDI Mappings. The ingress worker does the matching; this module arms it, applies
//! what it publishes with the same setters a mouse drag uses, and keeps the two stores in step
//! with the engine and the preferences.

use crate::fine::Target;
use crate::mixer::{MenuItem, db_to_k, k_to_db};
use crate::theme::TOKENS;
use crate::{StudioApp, mixer};
use egui::{Color32, Rect, Stroke, pos2};
use serde_json::{Value, json};
use soundcraft_model::{AutoParam, Control, ControlTarget, MapMode, Mapping, MappingSet, Resolved, Scope, Session, SourceKind, Strip, TrackId};
use soundcraft_playback::{MapMsg, MapSpec};

/// A mapped sweep stays one undo step until the controller has been still this long (seconds,
/// the UI clock).
pub const GESTURE_END_S: f64 = 0.4;

/// How soon the UI looks again while the controller is active, and while it is quiet.
pub const BUSY_POLL: std::time::Duration = std::time::Duration::from_millis(16);
pub const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// The merge key of every mapped move (see `learned_move`).
const GESTURE_KEY: &str = "midi:gesture";

/// Window state saved with the preferences (`ui.json`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub open: bool,
    pub size: [f32; 2],
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { open: false, size: [760.0, 320.0] }
    }
}

/// A marker or learn outline as drawn this frame, for `ui.inspect` and tests.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Drawn {
    pub track: u64,
    pub control: String,
    pub learning: bool,
    pub mapped: bool,
    pub overridden: bool,
    pub rect: crate::arrange::R4,
}

#[derive(Default)]
pub struct State {
    /// The mappings in force (the active half of [`soundcraft_model::resolve`]) and the table the
    /// worker's event indexes refer to, rebuilt each frame the stores are not both empty.
    resolved: Vec<Resolved>,
    /// Every stored mapping with its overridden flag, for menus and the window.
    all: Vec<Resolved>,
    applied: Vec<u32>,
    epoch: u32,
    armed_sent: bool,
    /// Which Player's MIDI side the worker state belongs to (changes on an audio reconnect).
    generation: u32,
    /// Button presses and candidates the worker could not queue, as last read.
    pub dropped: u64,
    /// The worker's activity counter at the last frame, and whether it moved since.
    activity_seen: u64,
    news: bool,
    profile_loaded: bool,
    profile_rev: u64,
    /// The UI time of the last mapped move, while a sweep may still be going.
    last_move: Option<f64>,
    /// "Learn next touched parameter" waits on this hosted plug-in's editor (track, slot).
    pub(crate) next: Option<(TrackId, usize)>,
    pub drawn: Vec<Drawn>,
    /// The learn menu's entries as last drawn (while it is open).
    pub menu: Vec<MenuItem>,
    pub window: [f32; 4],
}

impl State {
    /// Makes the next frame see the worker's counter as changed (tests).
    #[cfg(test)]
    pub fn pretend_news(&mut self) {
        self.activity_seen = u64::MAX;
    }

    /// The per-mapping applied counters (tests).
    pub fn applied_counts(&self) -> &[u32] {
        &self.applied
    }

    /// A mapped sweep is going (the controller moved within `GESTURE_END_S`): the frame must not
    /// end its undo step.
    pub fn sweeping(&self) -> bool {
        self.last_move.is_some()
    }

    /// How many mappings are in force (not overridden).
    pub fn resolved_len(&self) -> usize {
        self.resolved.len()
    }
}

/// What a control is mapped to, for its menu and marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub scope: Scope,
    pub text: String,
    pub overridden: bool,
}

/// One entry of a control's right-click menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Disabled line: "Mapped: KeyStep ch 1 CC 74".
    Mapped,
    Learn,
    LearnProfile,
    Forget,
    Cancel,
}

/// The menu state machine: which entries a control offers, given what it is mapped to and
/// whether it is the control being learned.
pub fn menu_entries(mapped: Option<&Info>, learning: bool) -> Vec<Entry> {
    if learning {
        return vec![Entry::Cancel];
    }
    match mapped {
        Some(_) => vec![Entry::Mapped, Entry::Learn, Entry::Forget],
        None => vec![Entry::Learn, Entry::LearnProfile],
    }
}

fn entry_text(entry: Entry, info: Option<&Info>) -> String {
    match entry {
        Entry::Mapped => {
            let info = info.cloned().unwrap_or(Info { scope: Scope::Session, text: String::new(), overridden: false });
            let scope = if info.scope == Scope::Profile { " (controller)" } else { "" };
            let over = if info.overridden { ", overridden by this session" } else { "" };
            format!("Mapped: {}{scope}{over}", info.text)
        }
        Entry::Learn => "MIDI Learn".into(),
        Entry::LearnProfile => "MIDI Learn for this controller (all sessions)".into(),
        Entry::Forget => "Forget MIDI Mapping".into(),
        Entry::Cancel => "Cancel MIDI Learn (Esc)".into(),
    }
}

/// The target as `scope` stores it: a session mapping names the track, the profile its position.
pub fn target_for(s: &Session, track: TrackId, control: Control, scope: Scope) -> Option<ControlTarget> {
    let strip = match scope {
        Scope::Session => Strip::Track(track),
        Scope::Profile => Strip::Position(u16::try_from(s.tracks.iter().position(|t| t.id == track)? + 1).ok()?),
    };
    Some(ControlTarget { strip, control })
}

fn source_text(m: &Mapping) -> String {
    m.source.describe()
}

/// What the control is mapped to: the mapping in force, else one a session mapping overrides.
pub fn info(app: &StudioApp, track: TrackId, control: &Control) -> Option<Info> {
    let st = &app.midi_learn;
    let r = st.all.iter().filter(|r| r.track == track && r.control == *control).min_by_key(|r| r.overridden)?;
    Some(Info { scope: r.scope, text: source_text(&r.mapping), overridden: r.overridden })
}

/// Whether `track`'s `control` is the one an armed learn is waiting for.
pub fn learning(app: &StudioApp, track: TrackId, control: &Control) -> bool {
    app.engine.learn().is_some_and(|arm| {
        target_for(app.engine.session(), track, control.clone(), arm.scope).as_ref() == Some(&arm.target) && arm.target.control == *control
    })
}

/// The control of insert `slot` (or the instrument) and parameter `param`, in the addressing the
/// automation picker uses.
pub fn plugin_control(slot: usize, param: &str) -> Option<Control> {
    let slot = if slot == soundcraft_mix::INSTRUMENT_SLOT {
        AutoParam::INSTRUMENT_SLOT
    } else {
        u8::try_from(slot).ok().filter(|s| usize::from(*s) < soundcraft_model::INSERT_SLOTS)?
    };
    Some(Control::Plugin { slot, param: param.to_string() })
}

/// Whether a hosted plug-in's parameter may be mapped: the host registries list only parameters
/// that are neither hidden nor read-only (the same table the automation picker offers), so a
/// parameter missing from a known table is refused. Without a cached table (never loaded this
/// run) a parameter the plug-in just reported, or the session holds, is accepted.
pub fn check_param(table: Option<&soundcraft_dsp::PluginInfo>, held: bool, param: &str) -> Result<(), String> {
    match table {
        Some(info) if info.param(param).is_some() => Ok(()),
        Some(_) => Err(format!("`{param}` is read-only or cannot be automated, so it cannot be mapped")),
        None if held => Ok(()),
        None => Err(format!("Cannot map `{param}`: the plug-in's parameter list is not loaded yet")),
    }
}

impl StudioApp {
    /// The plug-in a Plugin control addresses.
    fn plugin_device(&self, track: TrackId, slot: u8) -> Option<&soundcraft_model::Insert> {
        let t = self.engine.session().track(track)?;
        if slot == AutoParam::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(usize::from(slot))?.as_ref() }
    }

    /// Whether `control` may be learned: a Plugin control needs a parameter the device lists.
    fn control_mappable(&self, track: TrackId, control: &Control) -> Result<(), String> {
        let Control::Plugin { slot, param } = control else { return Ok(()) };
        let ins = self.plugin_device(track, *slot).ok_or("That slot has no plug-in")?;
        check_param(crate::automation::device_info(&ins.plugin), ins.params.contains_key(param), param)
    }
}

/// The parameter value the knob position `norm` (0..1) stands for: the parameter's own range, the
/// taper honored, whole numbers where the parameter is one (the same step a panel knob takes).
pub fn plugin_value(p: &soundcraft_dsp::ParamInfo, norm: f32) -> f32 {
    p.clamp(crate::widgets::knob::value_at(p, norm.clamp(0.0, 1.0)))
}

/// The control's value, 0..1 of its travel, as the session holds it now.
fn norm_value(s: &Session, track: TrackId, control: &Control) -> f32 {
    let Some(t) = s.track(track) else { return 0.0 };
    match control {
        Control::Volume => db_to_k(t.mixer.volume_db),
        Control::Pan => (mixer::balance_of(&t.mixer.pan) + 1.0) / 2.0,
        Control::Mute => f32::from(u8::from(t.mixer.mute)),
        Control::Solo => f32::from(u8::from(t.mixer.solo)),
        Control::Arm => f32::from(u8::from(t.mixer.record_arm)),
        Control::Send(n) => t.mixer.sends.get(usize::from(*n)).and_then(Option::as_ref).map_or(0.0, |x| db_to_k(x.level_db)),
        Control::Plugin { slot, param } => {
            let lane = crate::automation::Lane::Plugin { slot: *slot, param: param.clone() };
            lane.info(t).map_or(0.0, |p| p.to_normalized(lane.static_value(t)))
        }
    }
}

impl StudioApp {
    /// Right-click menu entries of a mixer control (call inside its context menu).
    pub(crate) fn midi_learn_menu(&mut self, ui: &mut egui::Ui, track: TrackId, control: Control) {
        let mapped = info(self, track, &control);
        let learning = learning(self, track, &control);
        self.learn_entries(ui, track, control, mapped, learning);
    }

    fn learn_entries(&mut self, ui: &mut egui::Ui, track: TrackId, control: Control, mapped: Option<Info>, learning: bool) {
        for entry in menu_entries(mapped.as_ref(), learning) {
            let text = entry_text(entry, mapped.as_ref());
            let b = ui.add_enabled(entry != Entry::Mapped, egui::Button::new(text.clone()));
            self.midi_learn.menu.push(MenuItem::at(ui, text, format!("{entry:?}"), b.rect));
            if b.clicked() {
                let _ = self.learn_choose(entry, track, control.clone(), mapped.as_ref());
                ui.close();
            }
        }
    }

    /// Does what a menu entry says.
    pub(crate) fn learn_choose(&mut self, entry: Entry, track: TrackId, control: Control, mapped: Option<&Info>) -> Result<Value, String> {
        let scope = if entry == Entry::LearnProfile { Scope::Profile } else { mapped.map_or(Scope::Session, |m| m.scope) };
        if matches!(entry, Entry::Learn | Entry::LearnProfile)
            && let Err(why) = self.control_mappable(track, &control)
        {
            self.status = why.clone();
            return Err(why);
        }
        let target = target_for(self.engine.session(), track, control, scope).ok_or("no such track")?;
        match entry {
            Entry::Mapped => Ok(json!({})),
            Entry::Learn | Entry::LearnProfile => self.run("midi.learn", json!({"target": target.to_string(), "scope": scope})),
            Entry::Forget => self.run("midi.unmap", json!({"target": target.to_string(), "scope": scope})),
            Entry::Cancel => self.run("midi.learn_cancel", json!({})),
        }
    }
}

/// Draws the marker of a mapped control and the outline of one being learned at `r`, and records
/// them for `ui.inspect`. Call after drawing the control.
pub fn mark(app: &mut StudioApp, p: &egui::Painter, r: Rect, track: TrackId, control: Control) {
    let learning = learning(app, track, &control);
    let mapped = info(app, track, &control);
    if !learning && mapped.is_none() {
        return;
    }
    if learning {
        // A pulse, so the state reads as waiting.
        let t = p.ctx().input(|i| i.time) as f32;
        let a = 0.55 + 0.45 * (t * 4.0).sin().abs();
        p.rect_stroke(r.expand(1.0), crate::theme::RADIUS_SM, Stroke::new(2.0, TOKENS.accent.gamma_multiply(a)), egui::StrokeKind::Outside);
        p.ctx().request_repaint_after(std::time::Duration::from_millis(60));
    }
    if let Some(m) = &mapped {
        let c = pos2(r.max.x - 3.5, r.min.y + 3.5);
        let ink = if m.overridden { TOKENS.text_dim.gamma_multiply(0.7) } else { TOKENS.accent };
        p.circle_filled(c, 2.5, ink);
        p.circle_stroke(c, 2.5, Stroke::new(0.5, TOKENS.bg));
    }
    app.midi_learn.drawn.push(Drawn {
        track: track.0,
        control: format!("{control:?}"),
        learning,
        mapped: mapped.is_some(),
        overridden: mapped.is_some_and(|m| m.overridden),
        rect: mixer::r4(r),
    });
}

/// Escape cancels an armed learn. Runs before the other Escape handlers and takes the key only
/// while a learn is armed.
pub fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    if (app.engine.learn().is_some() || app.midi_learn.next.is_some())
        && app.mixer.edit.is_none()
        && crate::shortcuts::pressed(ctx, crate::shortcuts::Action::ReadoutCancel)
    {
        app.midi_learn.next = None;
        let _ = app.run("midi.learn_cancel", json!({}));
    }
}

/// The note name of a MIDI key (C3 = 60), as the Monitor writes it.
fn note_name(n: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[usize::from(n % 12)], i32::from(n / 12) - 2)
}

/// A row of the Window > MIDI Mappings table.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Row {
    pub scope: Scope,
    pub overridden: bool,
    pub port: String,
    pub channel: String,
    pub source: String,
    pub target: String,
    pub target_id: String,
    pub mode: MapMode,
    pub range: [f32; 2],
    pub toggle: bool,
    /// The mapped message also reaches the track's routes.
    pub pass_through: bool,
    /// The track exists in this session (a mapping to a missing one is stored but idle).
    pub live: bool,
    /// This row is being re-learned.
    pub learning: bool,
}

/// Every stored mapping, session first, as the window lists it.
pub fn rows(app: &StudioApp) -> Vec<Row> {
    let s = app.engine.session();
    let learn = app.engine.learn();
    let mut out = Vec::new();
    for (scope, set) in [(Scope::Session, &s.midi_mappings), (Scope::Profile, app.engine.midi_profile())] {
        for m in set.iter() {
            let resolved = app.midi_learn.all.iter().find(|r| r.scope == scope && r.mapping == *m);
            let track = resolved.and_then(|r| s.track(r.track));
            let control = m.target.control.label();
            let target = match (track, m.target.strip) {
                (Some(t), _) => format!("{}: {control}", t.name),
                (None, Strip::Track(id)) => format!("Missing track {}: {control}", id.0),
                (None, Strip::Position(n)) => format!("Strip {n}: {control}"),
            };
            let source = match m.source.kind {
                SourceKind::Cc => format!("CC {}", m.source.number),
                SourceKind::Note => format!("Note {} ({})", note_name(m.source.number), m.source.number),
            };
            out.push(Row {
                scope,
                overridden: resolved.is_some_and(|r| r.overridden),
                port: m.source.port.clone(),
                channel: m.source.channel.map_or_else(|| "any".into(), |c| (c + 1).to_string()),
                source,
                target,
                target_id: m.target.to_string(),
                mode: m.mode,
                range: [m.min, m.max],
                toggle: m.toggle,
                pass_through: m.pass_through,
                live: track.is_some(),
                learning: learn.as_ref().is_some_and(|l| l.scope == scope && l.target == m.target),
            });
        }
    }
    out
}

/// `ui.inspect` view of the feature.
pub fn inspect(app: &StudioApp) -> Value {
    json!({
        "open": app.ui.midi_mappings.open,
        "armed": app.engine.learn().map(|l| json!({"target": l.target.to_string(), "scope": l.scope})),
        "next_touched": app.midi_learn.next.map(|(t, slot)| json!({"track": t.0, "slot": slot})),
        "active": app.midi_learn.resolved.len(),
        "rows": rows(app),
        "dropped": app.midi_learn.dropped,
        "markers": app.midi_learn.drawn,
        "menu": app.midi_learn.menu,
        "window": app.midi_learn.window,
    })
}

impl StudioApp {
    /// `window.midi_mappings {value?}`: open or close the window (without `value` it flips).
    pub(crate) fn toggle_midi_mappings(&mut self, value: Option<bool>) -> Value {
        let open = value.unwrap_or(!self.ui.midi_mappings.open);
        self.ui.midi_mappings.open = open;
        json!({"open": open})
    }

    /// Once a frame, before the panes draw: keep the stores, the worker and the preferences in
    /// step, apply what the controller did, and end a finished sweep's undo step.
    pub(crate) fn midi_learn_feed(&mut self, now: f64) -> Option<std::time::Duration> {
        self.midi_learn.drawn.clear();
        self.midi_learn.menu.clear();
        // The preferences' profile goes to the engine once; later changes come back out.
        if !self.midi_learn.profile_loaded {
            self.midi_learn.profile_loaded = true;
            self.engine.set_midi_profile(self.ui.midi_profile.clone());
            self.midi_learn.profile_rev = self.engine.midi_profile_revision();
        } else if self.midi_learn.profile_rev != self.engine.midi_profile_revision() {
            self.midi_learn.profile_rev = self.engine.midi_profile_revision();
            self.ui.midi_profile = self.engine.midi_profile().clone();
        }
        let ids: Vec<TrackId> = self.engine.session().tracks.iter().map(|t| t.id).collect();
        let all = if self.engine.session().midi_mappings.is_empty() && self.engine.midi_profile().is_empty() {
            Vec::new()
        } else {
            soundcraft_model::resolve(&ids, &self.engine.session().midi_mappings, self.engine.midi_profile())
        };
        // A plug-in mapping whose device is gone is stored but idle.
        let live = |r: &Resolved| match &r.control {
            Control::Plugin { slot, .. } => self.plugin_device(r.track, *slot).is_some(),
            _ => true,
        };
        let active: Vec<Resolved> = all.iter().filter(|r| !r.overridden && live(r)).cloned().collect();
        self.midi_learn.all = all;
        self.midi_learn.resolved = active;
        let msgs = self.midi_learn_sync();
        self.midi_learn_apply(msgs, now);
        if let Some(t) = self.midi_learn.last_move
            && now - t > GESTURE_END_S
        {
            self.midi_learn.last_move = None;
            if !crate::automation::passing(self) {
                self.engine.end_merge();
            }
        }
        // The worker cannot wake the UI, so while anything is mapped (or a learn waits) the UI looks
        // in now and then: at the normal rate while the controller is moving, a learn waits or a
        // sweep's undo step is still open, else at a light poll (a first move waits at most that).
        if self.player.is_none() || (self.midi_learn.resolved.is_empty() && self.engine.learn().is_none()) {
            return None;
        }
        let busy = self.midi_learn.news || self.engine.learn().is_some() || self.midi_learn.last_move.is_some();
        Some(if busy { BUSY_POLL } else { IDLE_POLL })
    }

    /// Hand the worker the mappings and the learn flag; take what it published.
    fn midi_learn_sync(&mut self) -> (u32, Vec<MapMsg>) {
        let Some(player) = &self.player else {
            self.midi_learn.armed_sent = false;
            return (0, Vec::new());
        };
        // A reconnect swaps in a Player whose worker is not armed and whose counters start over.
        let generation = player.midi_map_generation();
        if generation != self.midi_learn.generation {
            self.midi_learn.generation = generation;
            self.midi_learn.armed_sent = false;
            self.midi_learn.applied.clear();
        }
        let specs: Vec<MapSpec> = self.midi_learn.resolved.iter().map(MapSpec::from_resolved).collect();
        player.set_midi_mappings(specs);
        self.midi_learn.dropped = player.midi_map_dropped();
        let activity = player.midi_map_activity();
        self.midi_learn.news = activity != self.midi_learn.activity_seen;
        self.midi_learn.activity_seen = activity;
        let armed = self.engine.learn().is_some();
        if armed != self.midi_learn.armed_sent {
            player.set_midi_learn(armed);
            self.midi_learn.armed_sent = armed;
        }
        (player.midi_map_epoch(), player.take_midi_map_events())
    }

    /// Apply what the controller did, then tell the worker the values the session holds now.
    pub(crate) fn midi_learn_apply(&mut self, (epoch, msgs): (u32, Vec<MapMsg>), now: f64) {
        if epoch != self.midi_learn.epoch || self.midi_learn.applied.len() != self.midi_learn.resolved.len() {
            self.midi_learn.epoch = epoch;
            self.midi_learn.applied = vec![0; self.midi_learn.resolved.len()];
        }
        for msg in msgs {
            match msg {
                MapMsg::Move { epoch: e, index, value, seq } if e == epoch => self.learned_move(index as usize, value, seq, now),
                MapMsg::Press { epoch: e, index, set } if e == epoch => self.learned_press(index as usize, set),
                MapMsg::Candidate { port, channel, kind, number } => self.learned_candidate(&port, channel, kind, number),
                _ => {}
            }
        }
        if let Some(player) = &self.player {
            let s = self.engine.session();
            let values: Vec<(f32, u32)> =
                self.midi_learn.resolved.iter().zip(&self.midi_learn.applied).map(|(r, n)| (norm_value(s, r.track, &r.control), *n)).collect();
            player.sync_midi_map(&values);
        }
    }

    fn learned_move(&mut self, index: usize, value: f32, seq: u32, now: f64) {
        let Some(r) = self.midi_learn.resolved.get(index).cloned() else { return };
        if let Some(n) = self.midi_learn.applied.get_mut(index) {
            *n = seq;
        }
        // One key for the whole gesture, so two knobs moved together are one undo step.
        let key = GESTURE_KEY;
        let value = value.clamp(0.0, 1.0);
        let target = match &r.control {
            Control::Volume => Some((Target::Fader(r.track), k_to_db(value))),
            Control::Pan => Some((Target::Pan(r.track), value * 2.0 - 1.0)),
            Control::Send(n) => Some((Target::Send(r.track, usize::from(*n)), k_to_db(value))),
            Control::Plugin { slot, param } => {
                self.mapped_param_move(r.track, *slot, param, value, key);
                self.midi_learn.last_move = Some(now);
                None
            }
            _ => None,
        };
        if let Some((target, v)) = target {
            let _ = self.set_value_merged(target, v, Some(key));
            self.midi_learn.last_move = Some(now);
        }
    }

    /// A mapped move of a plug-in parameter: the knob's 0..1 is the parameter's normalized travel
    /// (the taper honored, as a panel knob does), applied through the commands a mouse drag uses
    /// and touched like a drag, so Write, Touch and Latch record it and Read does not. A hosted
    /// editor holds the touch `HOSTED_HOLD_S` after the last value (a controller has no pointer).
    fn mapped_param_move(&mut self, track: TrackId, slot: u8, param: &str, norm: f32, key: &str) {
        let Some(ins) = self.plugin_device(track, slot) else { return };
        let Some(info) = crate::automation::device_info(&ins.plugin) else { return };
        let Some(p) = info.param(param) else { return };
        let value = plugin_value(p, norm);
        let slot_key = if slot == AutoParam::INSTRUMENT_SLOT { soundcraft_mix::INSTRUMENT_SLOT } else { usize::from(slot) };
        self.touch_param(track, slot_key, param, true);
        let r = if slot == AutoParam::INSTRUMENT_SLOT {
            self.engine.execute_merged("mix.instrument_param", &json!({"track": track.0, "param": param, "value": value}), key)
        } else {
            self.engine.execute_merged("mix.insert_param", &json!({"track": track.0, "slot": slot, "param": param, "value": value}), key)
        };
        crate::automation::note_pass_edit(self);
        if let Err(e) = r {
            self.status = e.to_string();
        }
    }

    /// The header's "MIDI Learn: next touched parameter" on the hosted editor at `key`: arm it, or
    /// cancel it when it is already armed there.
    pub(crate) fn learn_next_touched(&mut self, key: (TrackId, usize)) {
        if self.midi_learn.next == Some(key) {
            self.midi_learn.next = None;
            self.status = "MIDI Learn cancelled".into();
            return;
        }
        if self.engine.learn().is_some() {
            let _ = self.run("midi.learn_cancel", json!({}));
        }
        self.midi_learn.next = Some(key);
        self.status = "MIDI Learn: touch a parameter in the plug-in window".into();
    }

    /// A hosted editor reported `param` of (`track`, `slot`) as edited. While "next touched" waits
    /// on that plug-in this edit picks the target and arms the learn for the next CC or note.
    pub(crate) fn learn_touched(&mut self, track: TrackId, slot: usize, param: &str) {
        if self.midi_learn.next != Some((track, slot)) {
            return;
        }
        let Some(control) = plugin_control(slot, param) else { return };
        // The plug-in just reported it, so a table that is not cached yet does not refuse it.
        let device = |ins: &soundcraft_model::Insert| check_param(crate::automation::device_info(&ins.plugin), true, param);
        let refused = match self.plugin_device(
            track,
            if slot == soundcraft_mix::INSTRUMENT_SLOT { AutoParam::INSTRUMENT_SLOT } else { u8::try_from(slot).unwrap_or(u8::MAX) },
        ) {
            Some(ins) => device(ins).err(),
            None => Some("That slot has no plug-in".to_string()),
        };
        if let Some(why) = refused {
            self.midi_learn.next = None;
            self.status = why;
            return;
        }
        self.midi_learn.next = None;
        let Some(target) = target_for(self.engine.session(), track, control.clone(), Scope::Session) else { return };
        match self.run("midi.learn", json!({"target": target.to_string(), "scope": Scope::Session})) {
            Ok(_) => self.status = format!("MIDI Learn: move a control to map {}", control.label()),
            Err(e) => self.status = e,
        }
    }

    fn learned_press(&mut self, index: usize, set: Option<bool>) {
        let Some(r) = self.midi_learn.resolved.get(index).cloned() else { return };
        if let Some(n) = self.midi_learn.applied.get_mut(index) {
            *n = n.wrapping_add(1);
        }
        let id = match r.control {
            Control::Mute => "mix.mute",
            Control::Solo => "mix.solo",
            Control::Arm => "mix.record_arm",
            _ => return,
        };
        let on = set.unwrap_or_else(|| norm_value(self.engine.session(), r.track, &r.control) < 0.5);
        let _ = self.run(id, json!({"tracks": [r.track.0], "value": on}));
    }

    /// The armed learn caught a message: map it, keeping the old mapping's mode, range and toggle
    /// when the control (or a row being re-learned) had one.
    fn learned_candidate(&mut self, port: &str, channel: u8, kind: SourceKind, number: u8) {
        let Some(arm) = self.engine.learn() else { return };
        let old = match arm.scope {
            Scope::Session => self.engine.session().midi_mappings.for_target(&arm.target).cloned(),
            Scope::Profile => self.engine.midi_profile().for_target(&arm.target).cloned(),
        };
        let mut params = json!({
            "source": {"port": port, "channel": channel + 1, "kind": if kind == SourceKind::Cc { "cc" } else { "note" }, "number": number},
            "target": arm.target.to_string(),
            "scope": arm.scope,
        });
        if let (Some(old), Some(o)) = (old, params.as_object_mut()) {
            o.insert("mode".into(), json!(old.mode));
            o.insert("range".into(), json!([old.min, old.max]));
            o.insert("toggle".into(), json!(old.toggle));
            o.insert("pass_through".into(), json!(old.pass_through));
        }
        match self.run("midi.map", params) {
            Ok(_) => {
                self.status = format!(
                    "Mapped {} to {} ch {} {} {number}",
                    arm.target.control.label(),
                    port,
                    channel + 1,
                    if kind == SourceKind::Cc { "CC" } else { "note" }
                )
            }
            Err(e) => self.status = e,
        }
    }
}

fn kind_color(over: bool) -> Color32 {
    if over { TOKENS.text_dim } else { TOKENS.text }
}

/// Window > MIDI Mappings.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    app.midi_learn.window = [0.0; 4];
    if !app.ui.midi_mappings.open {
        return;
    }
    let mut open = true;
    let size = app.ui.midi_mappings.size;
    let rows = rows(app);
    let dropped = app.midi_learn.dropped;
    let mut actions: Vec<(usize, Action)> = Vec::new();
    let r = egui::Window::new("MIDI Mappings")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(size)
        .default_pos(egui::pos2(ctx.content_rect().center().x - size[0] / 2.0, crate::theme::TRANSPORT_H + 80.0))
        .show(ctx, |ui| {
            if dropped > 0 {
                ui.label(egui::RichText::new(format!("{dropped} controller button presses or learn messages were dropped (the UI was not draining fast enough).")).color(TOKENS.record));
            }
            if rows.is_empty() {
                ui.label(
                    egui::RichText::new("No mappings yet. Right-click a fader, pan, mute, solo, record arm or send level and choose MIDI Learn, then move a knob or press a key on your controller.")
                        .color(TOKENS.text_dim),
                );
                return;
            }
            let mono = crate::theme::mono(ui.ctx(), 12.0);
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("midi_mappings").num_columns(10).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                    for h in ["Port", "Ch", "Source", "Target", "Mode", "Range", "Pass through", "Scope", "", ""] {
                        ui.label(egui::RichText::new(h).font(mono.clone()).strong().color(TOKENS.text_dim));
                    }
                    ui.end_row();
                    for (i, row) in rows.iter().enumerate() {
                        let ink = kind_color(row.overridden || !row.live);
                        ui.label(egui::RichText::new(&row.port).font(mono.clone()).color(ink));
                        ui.label(egui::RichText::new(&row.channel).font(mono.clone()).color(ink));
                        ui.label(egui::RichText::new(&row.source).font(mono.clone()).color(ink));
                        ui.label(egui::RichText::new(&row.target).font(mono.clone()).color(ink));
                        let mut mode = row.mode;
                        egui::ComboBox::from_id_salt(("mm_mode", i))
                            .selected_text(if mode == MapMode::Pickup { "Pickup" } else { "Jump" })
                            .width(70.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut mode, MapMode::Pickup, "Pickup");
                                ui.selectable_value(&mut mode, MapMode::Jump, "Jump");
                            });
                        if mode != row.mode {
                            actions.push((i, Action::Mode(mode)));
                        }
                        ui.horizontal(|ui| {
                            let mut lo = row.range[0] * 100.0;
                            let mut hi = row.range[1] * 100.0;
                            let a = ui.add(egui::DragValue::new(&mut lo).range(0.0..=100.0).suffix("%").speed(0.5));
                            let b = ui.add(egui::DragValue::new(&mut hi).range(0.0..=100.0).suffix("%").speed(0.5));
                            let moved = (lo / 100.0 - row.range[0]).abs() > 1e-4 || (hi / 100.0 - row.range[1]).abs() > 1e-4;
                            if moved && (a.drag_stopped() || a.lost_focus() || b.drag_stopped() || b.lost_focus()) {
                                actions.push((i, Action::Range(lo / 100.0, hi / 100.0)));
                            }
                        });
                        let mut thru = row.pass_through;
                        if ui
                            .checkbox(&mut thru, "")
                            .on_hover_text("Also send the mapped message to the track (its instrument, a recording). Off: only the control hears it.")
                            .changed()
                        {
                            actions.push((i, Action::PassThrough(thru)));
                        }
                        let scope = match (row.scope, row.overridden) {
                            (Scope::Session, _) => "Session".to_string(),
                            (Scope::Profile, true) => "Controller (overridden)".to_string(),
                            (Scope::Profile, false) => "Controller".to_string(),
                        };
                        ui.label(egui::RichText::new(scope).font(mono.clone()).color(ink));
                        let label = if row.learning { "Listening..." } else { "Learn" };
                        if ui.button(label).on_hover_text("Press a key or move a knob: it replaces this row's source.").clicked() {
                            actions.push((i, if row.learning { Action::Cancel } else { Action::Learn }));
                        }
                        if ui.button("Delete").clicked() {
                            actions.push((i, Action::Delete));
                        }
                        ui.end_row();
                    }
                });
            });
        });
    if let Some(r) = r {
        app.midi_learn.window = mixer::r4(r.response.rect);
        app.ui.midi_mappings.size = [r.response.rect.width(), r.response.rect.height()];
    }
    app.ui.midi_mappings.open = open;
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
    Mode(MapMode),
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
            Action::Mode(_) | Action::Range(..) | Action::PassThrough(_) => {
                let target: ControlTarget = row.target_id.parse()?;
                let set: &MappingSet = match scope {
                    Scope::Session => &self.engine.session().midi_mappings,
                    Scope::Profile => self.engine.midi_profile(),
                };
                let old = set.for_target(&target).cloned().ok_or("that mapping is gone")?;
                let (mode, lo, hi, pass_through) = match action {
                    Action::Mode(m) => (m, old.min, old.max, old.pass_through),
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

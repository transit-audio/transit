//! Lane addresses, values, ranges and header labels.

use crate::StudioApp;
use crate::fine::{self, Target};
use crate::mixer::{balance_of, db_to_k, k_to_db, pans_for};
use serde_json::{Value, json};
use transit_model::{AutoParam, Route, Session, Track, TrackId};
use transit_time::Samples;

/// What a lane automates, as the UI sees it. Ids are the engine's `AutoParam` ids, except that
/// `pan` is the balance of all the track's panners (L10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lane {
    Volume,
    Pan,
    Mute,
    SendLevel(u8),
    SendPan(u8),
    SendMute(u8),
    Plugin { slot: u8, param: String },
}

/// A slot's letter, A..J (the model's own helper is private to it).
fn slot_letter(i: u8) -> char {
    char::from(b'A'.saturating_add(i.min(25)))
}

fn letter(i: u8) -> char {
    slot_letter(i).to_ascii_lowercase()
}

impl Lane {
    pub fn parse(id: &str) -> Option<Lane> {
        Some(match AutoParam::parse(id)? {
            AutoParam::Volume => Lane::Volume,
            AutoParam::Pan(_) => Lane::Pan,
            AutoParam::Mute => Lane::Mute,
            AutoParam::SendLevel(i) => Lane::SendLevel(i),
            AutoParam::SendPan(i) => Lane::SendPan(i),
            AutoParam::SendMute(i) => Lane::SendMute(i),
            AutoParam::Plugin { slot, param } => Lane::Plugin { slot, param },
        })
    }

    pub fn id(&self) -> String {
        match self {
            Lane::Volume => "volume".into(),
            Lane::Pan => "pan".into(),
            Lane::Mute => "mute".into(),
            Lane::SendLevel(i) => format!("send_{}_level", letter(*i)),
            Lane::SendPan(i) => format!("send_{}_pan", letter(*i)),
            Lane::SendMute(i) => format!("send_{}_mute", letter(*i)),
            Lane::Plugin { slot, param } if *slot == AutoParam::INSTRUMENT_SLOT => format!("plugin:instrument:{param}"),
            Lane::Plugin { slot, param } => format!("plugin:{}:{param}", letter(*slot)),
        }
    }

    /// The lane of a plug-in parameter: an insert's (`slot` the insert index) or, for
    /// [`transit_mix::INSTRUMENT_SLOT`], the instrument's. (`fine::Target` is `Copy`, which a
    /// parameter id is not, so plug-in gestures reach the write pass as [`super::ParamTouch`]es.)
    pub fn of_param(track: TrackId, slot: usize, param: &str) -> Option<(TrackId, Lane)> {
        let slot =
            if slot == transit_mix::INSTRUMENT_SLOT { AutoParam::INSTRUMENT_SLOT } else { u8::try_from(slot).ok().filter(|s| *s < u8::MAX - 1)? };
        Some((track, Lane::Plugin { slot, param: param.to_string() }))
    }

    /// The device a plug-in lane addresses.
    pub(crate) fn device<'a>(&self, t: &'a Track) -> Option<&'a transit_model::Insert> {
        let Lane::Plugin { slot, .. } = self else { return None };
        if *slot == AutoParam::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(usize::from(*slot))?.as_ref() }
    }

    /// The control a lane belongs to, for the write pass and the mixer link.
    pub fn of_target(t: Target) -> (TrackId, Lane) {
        match t {
            Target::Fader(id) => (id, Lane::Volume),
            Target::Pan(id) => (id, Lane::Pan),
            Target::Send(id, s) => (id, Lane::SendLevel(u8::try_from(s).unwrap_or(0))),
            Target::SendPan(id, s) => (id, Lane::SendPan(u8::try_from(s).unwrap_or(0))),
        }
    }

    /// The engine parameter ids this lane writes: the Pan lane writes every panner.
    pub(super) fn param_ids(&self, t: &Track) -> Vec<String> {
        match self {
            Lane::Pan => (0..t.mixer.pan.len().max(1)).map(|i| if i == 0 { "pan".to_string() } else { format!("pan{}", i + 1) }).collect(),
            other => vec![other.id()],
        }
    }

    pub(super) fn params(&self, t: &Track) -> Vec<AutoParam> {
        self.param_ids(t).iter().filter_map(|p| AutoParam::parse(p)).collect()
    }

    /// Whether the lane still has something to automate (a removed send or device has not).
    pub fn valid(&self, t: &Track) -> bool {
        match self {
            Lane::SendLevel(i) | Lane::SendPan(i) | Lane::SendMute(i) => t.mixer.sends.get(usize::from(*i)).is_some_and(Option::is_some),
            Lane::Plugin { .. } => self.device(t).is_some(),
            _ => true,
        }
    }

    pub(super) fn is_step(&self) -> bool {
        matches!(self, Lane::Mute | Lane::SendMute(_))
    }

    pub(crate) fn info(&self, t: &Track) -> Option<&'static transit_dsp::ParamInfo> {
        let Lane::Plugin { param, .. } = self else { return None };
        device_info(&self.device(t)?.plugin)?.params.iter().find(|p| p.id == param)
    }

    /// The value the parameter has without automation.
    pub fn static_value(&self, t: &Track) -> f32 {
        let send = |i: &u8| t.mixer.sends.get(usize::from(*i)).and_then(Option::as_ref);
        match self {
            Lane::Volume => t.mixer.volume_db,
            Lane::Pan => balance_of(&t.mixer.pan),
            Lane::Mute => f32::from(u8::from(t.mixer.mute)),
            Lane::SendLevel(i) => send(i).map_or(fine::DB_MIN, |s| s.level_db),
            Lane::SendPan(i) => send(i).map_or(0.0, |s| s.pan),
            Lane::SendMute(i) => send(i).map_or(0.0, |s| f32::from(u8::from(s.mute))),
            Lane::Plugin { param, .. } => {
                self.device(t).and_then(|i| i.params.get(param).copied()).or_else(|| self.info(t).map(|i| i.default)).unwrap_or(0.0)
            }
        }
    }

    /// The lane's value at `at`: its curve where it has points, else the static value.
    pub fn value_at(&self, t: &Track, at: Samples) -> f32 {
        match self {
            Lane::Pan => {
                let pans: Vec<f32> = self
                    .params(t)
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let base = t.mixer.pan.get(i).copied().unwrap_or(0.0);
                        t.lane(p).filter(|l| !l.points.is_empty()).map_or(base, |l| l.value_at(at, base))
                    })
                    .collect();
                balance_of(&pans)
            }
            other => {
                let base = other.static_value(t);
                other.params(t).first().and_then(|p| t.lane(p)).filter(|l| !l.points.is_empty()).map_or(base, |l| l.value_at(at, base))
            }
        }
    }

    /// The lane's points (the first panner's times for the Pan lane), with the lane's value there.
    pub fn points(&self, t: &Track) -> Vec<(Samples, f32)> {
        let Some(l) = self.params(t).first().and_then(|p| t.lane(p)) else { return Vec::new() };
        l.points.iter().map(|p| (p.at, if *self == Lane::Pan { self.value_at(t, p.at) } else { p.value })).collect()
    }

    /// Value to lane height, 0 (bottom) .. 1 (top): the fader law for levels.
    pub fn norm(&self, t: &Track, v: f32) -> f32 {
        match self {
            Lane::Volume | Lane::SendLevel(_) => db_to_k(v),
            Lane::Pan | Lane::SendPan(_) => (v + 1.0) / 2.0,
            Lane::Mute | Lane::SendMute(_) => v.clamp(0.0, 1.0),
            Lane::Plugin { .. } => self.info(t).map_or(v.clamp(0.0, 1.0), |i| i.to_normalized(v)),
        }
    }

    pub fn denorm(&self, t: &Track, k: f32) -> f32 {
        let k = k.clamp(0.0, 1.0);
        match self {
            Lane::Volume | Lane::SendLevel(_) => k_to_db(k),
            Lane::Pan | Lane::SendPan(_) => k * 2.0 - 1.0,
            Lane::Mute | Lane::SendMute(_) => k.round(),
            Lane::Plugin { .. } => self.info(t).map_or(k, |i| i.from_normalized(k)),
        }
    }

    /// A value in the parameter's unit: dB, L/C/R, Muted, the device's own format.
    pub fn text(&self, t: &Track, v: f32) -> String {
        match self {
            Lane::Volume | Lane::SendLevel(_) => format!("{} dB", fine::db_text(v)),
            Lane::Pan | Lane::SendPan(_) => fine::pan_text(v),
            Lane::Mute | Lane::SendMute(_) => if v >= 0.5 { "Muted" } else { "On" }.into(),
            Lane::Plugin { .. } => self.info(t).map_or_else(|| format!("{:.0} %", v * 100.0), |i| i.format(v)),
        }
    }

    /// What the lane's header says.
    pub fn label(&self, app: &mut StudioApp, s: &Session, t: &Track) -> String {
        let send = |i: &u8| {
            let target = t.mixer.sends.get(usize::from(*i)).and_then(Option::as_ref).map(|x| match &x.target {
                Route::Bus(b) => crate::mixer::bus_label(s, *b),
                other => crate::mixer::route_label(s, other, false, 0),
            });
            format!("Send {} · {}", slot_letter(*i), target.unwrap_or_default())
        };
        match self {
            Lane::Volume => "Volume".into(),
            Lane::Pan => "Pan".into(),
            Lane::Mute => "Mute".into(),
            Lane::SendLevel(i) => format!("{} level", send(i)),
            Lane::SendPan(i) => format!("{} pan", send(i)),
            Lane::SendMute(i) => format!("{} mute", send(i)),
            Lane::Plugin { param, .. } => {
                let dev = self.device(t).map(|i| i.plugin.clone()).unwrap_or_default();
                let name = self.info(t).map_or_else(|| param.clone(), |i| i.name.to_string());
                format!("{} · {name}", app.plugin_name(&dev, false))
            }
        }
    }

    /// `automation.set_point` parameters for a point at `at` (one per panner for Pan).
    pub(super) fn set_cmds(&self, t: &Track, at: Samples, v: f32, from: Option<Samples>) -> Vec<(&'static str, Value)> {
        let values = if *self == Lane::Pan { pans_for(v, t.mixer.pan.len().max(1)) } else { vec![v] };
        self.param_ids(t)
            .into_iter()
            .zip(values)
            .map(|(param, value)| {
                let mut p = json!({"track": t.id.0, "param": param, "at": at, "value": value});
                if let Some(f) = from {
                    p["from"] = f.into();
                }
                ("automation.set_point", p)
            })
            .collect()
    }

    /// `automation.clear` parameters removing exactly the point at `at` (never an empty range,
    /// which would clear the whole lane).
    pub(super) fn clear_cmds(&self, t: &Track, at: Samples) -> Vec<(&'static str, Value)> {
        self.param_ids(t)
            .into_iter()
            .map(|param| ("automation.clear", json!({"track": t.id.0, "param": param, "start": at, "end": at.saturating_add(1)})))
            .collect()
    }
}

/// A device's parameter table: a built-in's, or a hosted plug-in's from the host registries'
/// cache. The cache is read only (no instantiation), so this never blocks the UI thread; a type
/// is cached once any instance of it has loaded (a project's plug-ins do on open).
pub fn device_info(plugin: &str) -> Option<&'static transit_dsp::PluginInfo> {
    transit_dsp::plugin_info(plugin)
        .or_else(|| plugin.starts_with(transit_clap_host::ID_PREFIX).then(|| transit_clap_host::cached_info(plugin)).flatten())
        .or_else(|| plugin.starts_with(transit_vst3_host::ID_PREFIX).then(|| transit_vst3_host::cached_info(plugin)).flatten())
}


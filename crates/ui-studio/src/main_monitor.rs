//! The main bus monitoring set (#366): Mono, Dim and Ref with the per-machine monitor level, the
//! live integrated loudness readout.
//!
//! Monitor state is a per-machine preference kept with the audio settings in `ui.json`
//! ([`MonitorPrefs`], `UiState::monitor`), never in the session and never an undo step. It reaches
//! the audio thread through `Player::monitor` (plain atomic stores, every frame, like the click
//! level). Everything here acts on the monitor path only: what the device plays after the master,
//! never a bounce, a freeze, a stem, a send or the meters (`transit_playback::monitor_path`).
//!
//! `mix.monitor {mono?, dim?, ref?, level_db?, store_ref?}` changes it (one command, each field
//! optional, not undoable); `ui.inspect` shows it under `monitor` with the loudness readout.

use crate::StudioApp;
use serde_json::{Value, json};
use transit_playback::monitor_path::{LEVEL_FLOOR_DB, MonitorSettings};

/// The monitoring set as this machine keeps it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MonitorPrefs {
    /// The monitor level in dB, −∞ (stored as the floor, −144) to 0. The master fader stays the
    /// mix level; this is the listening level after it.
    pub level_db: f32,
    /// The level Ref recalls (Alt+click on Ref stores the current monitor level here).
    pub reference_db: f32,
    pub mono: bool,
    pub dim: bool,
    pub reference: bool,
}

impl Default for MonitorPrefs {
    fn default() -> Self {
        Self { level_db: 0.0, reference_db: 0.0, mono: false, dim: false, reference: false }
    }
}

/// A level in dB for the monitor: −∞ (the floor) to 0; anything not a number is refused by the
/// command and read as 0 dB from a hand-edited file.
fn level(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(LEVEL_FLOOR_DB, 0.0) }
}

impl MonitorPrefs {
    /// The values a hand-edited or older `ui.json` may carry, brought into range.
    #[must_use]
    pub fn sane(self) -> Self {
        Self { level_db: level(self.level_db), reference_db: level(self.reference_db), ..self }
    }

    /// What the player's monitor controls take, every frame and at a device open.
    #[must_use]
    pub fn settings(self) -> MonitorSettings {
        let p = self.sane();
        MonitorSettings { mono: p.mono, dim: p.dim, reference: p.reference, level_db: p.level_db, reference_db: p.reference_db }
    }
}

/// The footer's text: `INT` and the figure ("−14.2 LUFS"), or "—" before the first gated block.
#[must_use]
pub fn loudness_text(lufs: Option<f32>) -> String {
    match lufs {
        // A true minus (U+2212), as the board sets it.
        Some(v) if v.is_finite() => format!("{v:.1} LUFS").chars().map(|c| if c == '-' { '\u{2212}' } else { c }).collect(),
        _ => "—".into(),
    }
}

impl StudioApp {
    /// Hand the monitoring set to the player. Atomic stores only; safe to call every frame.
    pub(crate) fn push_monitor(&self) {
        if let Some(p) = &self.player {
            p.monitor().store(&self.ui.monitor.settings());
        }
    }

    /// The live integrated loudness of the mix since its last reset (`None`: no figure yet, or no
    /// device open).
    pub fn loudness(&self) -> Option<f32> {
        self.player.as_ref().and_then(|p| p.monitor().integrated_lufs())
    }

    /// A click on the readout: the measurement starts over.
    pub(crate) fn reset_loudness(&self) {
        if let Some(p) = &self.player {
            p.monitor().reset_loudness();
        }
    }

    /// Why Mono cannot act, when the session's main bus is wider than stereo (the monitor path
    /// folds a stereo pair only; a summing rule for a surround main bus is not defined), else `None`.
    pub(crate) fn mono_unavailable(&self) -> Option<String> {
        let channels = self.engine.session().main_format().channels();
        (channels > 2).then(|| format!("Mono acts on a stereo main bus only; this session's main bus has {channels} channels"))
    }

    /// `mix.monitor`: sets any of Mono, Dim, Ref and the monitor level, and with `store_ref` stores
    /// the monitor level (after `level_db`, when both are given) as the reference. A preference,
    /// not a session edit: no undo step, the session stays clean. Answers with the state.
    pub(crate) fn monitor_command(&mut self, p: &Value) -> Result<Value, String> {
        let id = "mix.monitor";
        let flag = |k: &str| match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("{id}: `{k}` must be true or false")),
        };
        let (mono, dim, reference, store) = (flag("mono")?, flag("dim")?, flag("ref")?, flag("store_ref")?);
        let level_db = match p.get("level_db") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.trim().trim_start_matches(['-', '−']).eq_ignore_ascii_case("inf") => Some(LEVEL_FLOOR_DB),
            Some(v) => match v.as_f64() {
                Some(db) if db.is_finite() && db <= 0.0 => Some(level(db as f32)),
                _ => return Err(format!("{id}: `level_db` must be a number of dB from \"-inf\" to 0")),
            },
        };
        if mono == Some(true)
            && let Some(why) = self.mono_unavailable()
        {
            return Err(format!("{id}: {why}"));
        }
        let m = &mut self.ui.monitor;
        if let Some(v) = mono {
            m.mono = v;
        }
        if let Some(v) = dim {
            m.dim = v;
        }
        if let Some(v) = reference {
            m.reference = v;
        }
        if let Some(v) = level_db {
            m.level_db = v;
        }
        if store == Some(true) {
            m.reference_db = m.level_db;
        }
        self.push_monitor();
        Ok(self.monitor_state())
    }

    /// The monitoring set and the loudness readout, for `ui.inspect` and `mix.monitor`'s answer.
    pub(crate) fn monitor_state(&self) -> Value {
        let m = self.ui.monitor.sane();
        let lufs = self.loudness();
        json!({
            // What the app does: Mono acts on a stereo main bus only. The stored choice is kept
            // (it returns on a stereo bus) and shown beside it.
            "mono": m.mono && self.mono_unavailable().is_none(),
            "mono_preference": m.mono,
            "dim": m.dim,
            "ref": m.reference,
            "level_db": m.level_db,
            "ref_db": m.reference_db,
            "lufs": lufs,
            "lufs_text": loudness_text(lufs),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readout_reads_as_the_board_does() {
        assert_eq!(loudness_text(Some(-14.24)), "\u{2212}14.2 LUFS");
        assert_eq!(loudness_text(Some(3.0)), "3.0 LUFS");
        assert_eq!(loudness_text(None), "—");
    }

    #[test]
    fn hand_edited_levels_come_back_into_range() {
        let wild = MonitorPrefs { level_db: 12.0, reference_db: f32::NAN, ..MonitorPrefs::default() }.sane();
        assert_eq!((wild.level_db, wild.reference_db), (0.0, 0.0));
        let low = MonitorPrefs { level_db: -500.0, ..MonitorPrefs::default() }.sane();
        assert_eq!(low.level_db, LEVEL_FLOOR_DB);
    }
}

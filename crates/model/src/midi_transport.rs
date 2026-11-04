//! Per-machine transport assignments (ADR 0043). Never part of a Session.

use crate::{Control, ControlTarget, Mapping, Strip};

pub const MACHINE_ONLY: &str = "Transport mappings are per-machine; choose Controller scope";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TransportControl {
    Play,
    Stop,
    Record,
    Cycle,
    ReturnToZero,
    Pause,
}

impl TransportControl {
    pub const ALL: [Self; 6] = [Self::Play, Self::Stop, Self::Record, Self::Cycle, Self::ReturnToZero, Self::Pause];

    pub fn key(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Stop => "stop",
            Self::Record => "record",
            Self::Cycle => "cycle",
            Self::ReturnToZero => "rtz",
            Self::Pause => "pause",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Stop => "Stop",
            Self::Record => "Record",
            Self::Cycle => "Cycle",
            Self::ReturnToZero => "Return to Zero",
            Self::Pause => "Pause",
        }
    }

    pub fn target(self) -> ControlTarget {
        // Transport never resolves this strip. Zero is deliberately not a valid strip index.
        ControlTarget { strip: Strip::Position(0), control: Control::Transport(self) }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TransportPrefs {
    pub transport_buttons: bool,
    #[serde(deserialize_with = "device_id")]
    pub mmc_device_id: u8,
    pub mappings: Vec<Mapping>,
}

fn device_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let id = <u8 as serde::Deserialize>::deserialize(d)?;
    if id > 126 { Err(serde::de::Error::custom("MMC device ID must be 0 to 126")) } else { Ok(id) }
}

impl Default for TransportPrefs {
    fn default() -> Self {
        Self { transport_buttons: true, mmc_device_id: 0, mappings: Vec::new() }
    }
}

pub type TransportProfiles = std::collections::BTreeMap<String, TransportPrefs>;

impl TransportPrefs {
    pub fn set(&mut self, mapping: Mapping) -> Result<usize, String> {
        mapping.validate()?;
        if !matches!(mapping.target.control, Control::Transport(_)) {
            return Err("A transport assignment needs a transport target".into());
        }
        let before = self.mappings.len();
        self.mappings.retain(|m| m.target != mapping.target && !m.source.overlaps(&mapping.source));
        let replaced = before - self.mappings.len();
        self.mappings.push(mapping);
        Ok(replaced)
    }
}

/// Remove imported machine assignments before session deserialization, with a visible load note.
pub(crate) fn strip_session_rows(value: &mut serde_json::Value) -> bool {
    let Some(rows) = value.get_mut("midi_mappings").and_then(serde_json::Value::as_array_mut) else { return false };
    let before = rows.len();
    rows.retain(|m| !m.get("target").and_then(serde_json::Value::as_str).is_some_and(|s| s.starts_with("transport:")));
    rows.len() != before
}

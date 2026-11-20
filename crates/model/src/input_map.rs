//! The input half of the machine's I/O table (ADR 0015, amendment for #431, rules 8, 9, 11 and
//! 14). Input paths belong to the input device actually opened, keyed by the audio system and the
//! capture endpoint's identity as the backend reports it, not to the output's table. The engine
//! holds one composed [`IoMap`]: the output record's outputs, Main and measurements, plus the input
//! record's count and paths ([`IoMap::compose`]); saving splits it again ([`IoMap::output_record`],
//! [`IoMap::input_record`]).

use crate::io_map::{IoDirection, IoMap, IoPath, MAX_DEVICE_CHANNELS, default_input_paths};

/// One input device's paths. `device` is the input identity (rule 8), `inputs` the open input's
/// channel count, `defaults_for` the width the default input paths were last made for (rule 9).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct InputMap {
    pub system: String,
    pub device: String,
    pub inputs: u16,
    pub defaults_for: u16,
    pub paths: Vec<IoPath>,
}

impl InputMap {
    /// The key the app stores this record under.
    pub fn key(&self) -> (&str, &str) {
        (&self.system, &self.device)
    }
}

impl IoMap {
    /// The table the engine holds: this output record's outputs, Main and measurements, with the
    /// input half of `input` (its count, its paths, its identity in [`IoMap::input_device`]), or
    /// no input half. Output paths come first, so a clashing name finds the output ([`IoMap::path`])
    /// and the input stays unavailable until one is renamed (rule 8, "Clashes").
    #[must_use]
    pub fn compose(&self, input: Option<&InputMap>) -> IoMap {
        let mut map = self.clone();
        map.paths.retain(|p| p.direction == IoDirection::Output);
        match input {
            Some(input) => {
                map.input_device.clone_from(&input.device);
                map.inputs = input.inputs.min(MAX_DEVICE_CHANNELS as u16);
                map.paths.extend(input.paths.iter().filter(|p| p.direction == IoDirection::Input).cloned());
            }
            None => {
                map.input_device.clear();
                map.inputs = 0;
            }
        }
        map
    }

    /// The output record to save for this composed table: everything but the input half, which is
    /// `old`'s exactly as it was read when an older build saved one under this key (rule 11, "Never
    /// erased"), else none (`inputs: 0`, no input paths).
    #[must_use]
    pub fn output_record(&self, old: Option<&IoMap>) -> IoMap {
        let mut record = self.clone();
        record.input_device.clear();
        record.paths.retain(|p| p.direction == IoDirection::Output);
        record.inputs = old.map_or(0, |o| o.inputs);
        record.paths.extend(old.into_iter().flat_map(|o| o.paths.iter().filter(|p| p.direction == IoDirection::Input).cloned()));
        record.buffer = None;
        record.system_audio = None;
        record
    }

    /// The input record to save for this composed table, with `defaults_for`; `None` when no input
    /// is composed in (nothing to save, and nothing saved is erased).
    #[must_use]
    pub fn input_record(&self, defaults_for: u16) -> Option<InputMap> {
        (!self.input_device.is_empty()).then(|| InputMap {
            system: self.system.clone(),
            device: self.input_device.clone(),
            inputs: self.inputs,
            defaults_for,
            paths: self.paths_of(IoDirection::Input).cloned().collect(),
        })
    }

    /// The default input paths for an input `width` channels wide that this table lacks: a default
    /// is present when any path has its name (generated defaults never overwrite a definition) or an
    /// input path reads exactly its channels (a renamed default, never duplicated). Adds them and
    /// returns how many (rules 9 and 14: "Add default paths" is this, idempotent).
    pub fn add_default_inputs(&mut self, width: u16) -> usize {
        let missing: Vec<IoPath> = default_input_paths(width)
            .into_iter()
            .filter(|d| self.path(&d.name).is_none() && !self.paths_of(IoDirection::Input).any(|p| p.channels == d.channels))
            .collect();
        let added = missing.len();
        self.paths.extend(missing);
        added
    }

    /// Whether inputs are open and no input path can be used: the I/O paths page says "No input
    /// paths" and offers Add default paths (rule 14).
    #[must_use]
    pub fn lacks_input_paths(&self) -> bool {
        self.inputs > 0 && self.paths_of(IoDirection::Input).all(|p| self.input_unavailable(p).is_some())
    }
}

/// The engine's table after an input opened successfully, `width` channels wide, as `identity` on
/// `output`'s system, and the input record to save with it (rules 8, 9 and 11).
///
/// The record is `inputs`' saved one for that identity; else, when an older build saved a
/// both-halves table under exactly that key (`outputs`; ASIO's driver, a JACK group that was the
/// output), its input half with no `defaults_for`; else an empty record. Paths beyond `width` stay
/// defined and unavailable. When `width` is wider than `defaults_for`, the defaults for it that the
/// table lacks are added ([`IoMap::add_default_inputs`]), except to a saved record the user left
/// empty, which is never refilled automatically (rule 8, "The record"). `defaults_for` becomes
/// `width`. A failed input open calls nothing here, so it changes no record.
#[must_use]
pub fn adopt_input(output: &IoMap, inputs: &[InputMap], outputs: &[IoMap], identity: &str, width: u16) -> (IoMap, InputMap) {
    let key = (output.system.as_str(), identity);
    let saved = inputs.iter().rfind(|r| r.key() == key);
    let empty = saved.is_some_and(|r| r.paths.is_empty());
    let record = saved.cloned().unwrap_or_else(|| {
        let mut record = InputMap { system: output.system.clone(), device: identity.into(), ..InputMap::default() };
        if let Some(old) = outputs.iter().rfind(|m| m.key() == key) {
            record.paths = old.paths_of(IoDirection::Input).cloned().collect();
        }
        record
    });
    let width = width.min(MAX_DEVICE_CHANNELS as u16);
    let mut map = output.compose(Some(&InputMap { inputs: width, ..record.clone() }));
    if width > record.defaults_for && !empty {
        map.add_default_inputs(width);
    }
    let record = map.input_record(width).unwrap_or(record);
    (map, record)
}

#[cfg(test)]
#[path = "input_map_tests.rs"]
mod tests;

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
    /// A JACK virtual input's ordered port names when this record was first made, or when the
    /// user last used the current ports (rule 5's baseline); written only for virtual devices. A
    /// reopen never replaces it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
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
                map.input_baseline.clone_from(&input.ports);
            }
            None => {
                map.input_device.clear();
                map.inputs = 0;
                map.input_baseline.clear();
                map.input_ports.clear();
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
        record.input_ports.clear();
        record.input_baseline.clear();
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
            ports: self.input_baseline.clone(),
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

    /// Whether the port-mapping guard holds `p` back (rule 5): the open input is a virtual device
    /// and the port under one of `p`'s channels is not the one the baseline has there (changed, or
    /// gone; a channel past the baseline counts as changed: it was never confirmed). Port names are
    /// addresses: a different stream reusing a port name is not detected.
    #[must_use]
    pub fn ports_changed(&self, p: &IoPath) -> bool {
        !self.input_ports.is_empty()
            && p.channels.iter().any(|&c| self.input_baseline.get(usize::from(c)).is_none_or(|b| self.input_ports.get(usize::from(c)) != Some(b)))
    }

    /// Whether the open virtual input's ports differ from the baseline (the I/O paths page then
    /// offers Use the current ports); false for every other input.
    #[must_use]
    pub fn ports_mismatch(&self) -> bool {
        !self.input_ports.is_empty() && self.input_ports != self.input_baseline
    }

    /// Use the current ports: the open virtual input's ports become the baseline, so every path over
    /// them is available again. The caller refuses it while the transport runs.
    pub fn use_current_ports(&mut self) {
        self.input_baseline.clone_from(&self.input_ports);
    }

    /// Whether a measurement whose return came through `input_device`, with a virtual input's
    /// `ports` (rule 13), applies to the input open now: through that input
    /// ([`IoMap::returns_through`]) and over exactly the ports open now (both empty for any input
    /// but a virtual device). A figure without ports through a virtual input needs measuring again.
    #[must_use]
    pub fn figure_applies(&self, input_device: &str, ports: &[String]) -> bool {
        self.returns_through(input_device) && ports == self.input_ports.as_slice()
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
    adopt_input_ports(output, inputs, outputs, identity, width, &[])
}

/// [`adopt_input`] for an input whose ports guard its paths: `ports` are a JACK virtual input's ordered port names at this open (rule 5), empty for any
/// other input: the record's first creation takes them as its baseline; a saved record keeps its
/// own (a reopen never replaces it), so paths over ports that changed are unavailable
/// ([`IoMap::ports_changed`]).
#[must_use]
pub fn adopt_input_ports(output: &IoMap, inputs: &[InputMap], outputs: &[IoMap], identity: &str, width: u16, ports: &[String]) -> (IoMap, InputMap) {
    let key = (output.system.as_str(), identity);
    let saved = inputs.iter().rfind(|r| r.key() == key);
    let empty = saved.is_some_and(|r| r.paths.is_empty());
    let record = saved.cloned().unwrap_or_else(|| {
        let mut record = InputMap { system: output.system.clone(), device: identity.into(), ports: ports.to_vec(), ..InputMap::default() };
        if let Some(old) = outputs.iter().rfind(|m| m.key() == key) {
            record.paths = old.paths_of(IoDirection::Input).cloned().collect();
        }
        record
    });
    let width = width.min(MAX_DEVICE_CHANNELS as u16);
    let mut map = output.compose(Some(&InputMap { inputs: width, ..record.clone() }));
    map.input_ports = ports.to_vec();
    if width > record.defaults_for && !empty {
        map.add_default_inputs(width);
    }
    let record = map.input_record(width).unwrap_or(record);
    (map, record)
}

#[cfg(test)]
#[path = "input_map_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "input_ports_tests.rs"]
mod ports_tests;

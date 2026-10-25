//! Persisted MIDI output destination of a MIDI or instrument track (#61), independent of the
//! native MIDI backend. The port is stored by name (plus the backend's stable id when it has one),
//! so a session opened where the device is absent keeps the choice and reports it missing.

/// The largest offset a destination accepts, in milliseconds either way.
pub const MAX_MIDI_OUT_OFFSET_MS: f32 = 500.0;

/// An output port, identified like a MIDI input device: backend, name, ordinal among same-name
/// ports, and the backend's stable id when it has one (it takes precedence when nonempty).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct MidiOutPort {
    pub backend: String,
    pub name: String,
    #[serde(default)]
    pub ordinal: u32,
    #[serde(default)]
    pub id: String,
}

/// Which channel an external destination sends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiOutChannel {
    /// Each event keeps the channel it was recorded on.
    #[default]
    AsRecorded,
    /// Every event is sent on this channel, zero-based (0 is MIDI channel 1).
    Channel(u8),
}

impl MidiOutChannel {
    /// The status-byte channel nibble for an event recorded on `recorded` (zero-based).
    #[must_use]
    pub fn resolve(self, recorded: u8) -> u8 {
        match self {
            Self::AsRecorded => recorded & 0x0F,
            Self::Channel(c) => c & 0x0F,
        }
    }
}

/// Where a track's MIDI goes.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiOutput {
    /// The track's hosted instrument (the behavior of every session before #61).
    #[default]
    Internal,
    /// A hardware or virtual MIDI port. The hosted instrument, if any, receives nothing.
    External {
        port: MidiOutPort,
        #[serde(default)]
        channel: MidiOutChannel,
        /// Sends happen this much later (negative: earlier) than the audio they belong with.
        #[serde(default)]
        offset_ms: f32,
    },
}

impl MidiOutput {
    #[must_use]
    pub fn is_internal(&self) -> bool {
        matches!(self, Self::Internal)
    }

    #[must_use]
    pub fn is_external(&self) -> bool {
        !self.is_internal()
    }

    /// The offset in milliseconds, finite and clamped to [`MAX_MIDI_OUT_OFFSET_MS`]; 0 when internal.
    /// Files are hostile: a huge value from a hand-edited session cannot schedule a send for
    /// next week.
    #[must_use]
    pub fn offset_ms(&self) -> f32 {
        match self {
            Self::External { offset_ms, .. } if offset_ms.is_finite() => offset_ms.clamp(-MAX_MIDI_OUT_OFFSET_MS, MAX_MIDI_OUT_OFFSET_MS),
            _ => 0.0,
        }
    }
}

/// Whether the input port (`backend`, `name`, `id`, as the backend reports it) and the output
/// `port` belong to the same device (#99). Echoing live input there loops when the device merges
/// its MIDI In to its Out (MIDI thru), so the echo is off unless the track allows it.
///
/// The rule: same backend, and either the backend's ids name the same device ([`device_of_id`]:
/// on Windows the WinMM interface path up to its interface-class suffix), or the port names are
/// equal once their direction suffix is removed ([`device_name`]). Names are compared even when
/// both ids are present: the in and out interface paths of one USB device have not been checked
/// on hardware, and blocking the echo between two distinct devices that share a name costs only
/// the explicit opt-in.
#[must_use]
pub fn same_midi_device(backend: &str, name: &str, id: &str, port: &MidiOutPort) -> bool {
    if backend != port.backend {
        return false;
    }
    if !id.is_empty() && !port.id.is_empty() && device_of_id(id) == device_of_id(&port.id) {
        return true;
    }
    let device = device_name(name);
    !device.is_empty() && device == device_name(&port.name)
}

/// A backend port id reduced to its device: lowercase, and a Windows interface path
/// (`\\?\usb#vid_1c75&pid_0288&mi_00#7&2a8b1d5&0&0000#{6994ad04-...}\global`) cut before its
/// interface-class GUID, which may differ between a device's in and out interfaces.
#[must_use]
pub fn device_of_id(id: &str) -> String {
    let id = id.trim().to_lowercase();
    match id.rfind("#{") {
        Some(at) => id.get(..at).unwrap_or_default().to_owned(),
        None => id,
    }
}

/// A port name reduced to its device: lowercase, Windows' `MIDIIN2 (x)` / `MIDIOUT2 (x)` wrapper
/// removed, then a trailing direction word with its number (`In`, `Out 2`, `Input`, `Output`,
/// `MIDI In`, `MIDI Out`) and a bare trailing `MIDI`. `KeyStep Pro MIDI In` and `KeyStep Pro`
/// both give `keystep pro`; `Synth 2` keeps its number.
#[must_use]
pub fn device_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for prefix in ["midiin", "midiout"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start();
            if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
                name = inner.trim().to_owned();
            }
            break;
        }
    }
    let mut words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '-' || c == '_').filter(|w| !w.is_empty()).collect();
    let direction = |w: &str| matches!(w.trim_end_matches(|c: char| c.is_ascii_digit()), "in" | "out" | "input" | "output" | "midiin" | "midiout");
    let number = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    if words.len() > 2 && words.last().is_some_and(|w| number(w)) && words.get(words.len() - 2).is_some_and(|w| direction(w)) {
        words.pop();
    }
    if words.len() > 1 && words.last().is_some_and(|w| direction(w)) {
        words.pop();
    }
    if words.len() > 1 && words.last() == Some(&"midi") {
        words.pop();
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelFormat, Track, TrackId, TrackKind};

    fn out(name: &str, id: &str) -> MidiOutPort {
        MidiOutPort { backend: "winmm".into(), name: name.into(), ordinal: 0, id: id.into() }
    }

    #[test]
    fn a_devices_in_and_out_ports_are_one_device() {
        // Names as Windows, ALSA and CoreMIDI give them.
        for (input, output) in [
            ("KeyStep Pro", "KeyStep Pro"),
            ("KeyStep Pro MIDI In", "KeyStep Pro MIDI Out"),
            ("KeyStep Pro", "KeyStep Pro MIDI OUT"),
            ("MIDIIN2 (KeyStep Pro)", "MIDIOUT2 (KeyStep Pro)"),
            ("MIDIIN3 (KeyStep Pro)", "KeyStep Pro"),
            ("Synth In 2", "Synth Out 2"),
            ("Synth In1", "Synth Out1"),
            ("Synth-Input", "Synth-Output"),
            ("  minilogue xd ", "MINILOGUE XD"),
        ] {
            assert!(same_midi_device("winmm", input, "", &out(output, "")), "{input} / {output}");
        }
        for (input, output) in [("KeyStep Pro", "MODEL D"), ("Synth 1", "Synth 2"), ("MIDI", "Synth"), ("", "")] {
            assert!(!same_midi_device("winmm", input, "", &out(output, "")), "{input} / {output}");
        }
        assert_eq!(device_name("Synth 2"), "synth 2");
        assert_eq!(device_name("MIDI In"), "midi");
        // Another backend is never the same device.
        assert!(!same_midi_device("alsa", "KeyStep Pro", "", &out("KeyStep Pro", "")));
    }

    #[test]
    fn interface_paths_name_the_device_without_their_class_suffix() {
        let input = r"\\?\USB#VID_1C75&PID_0288&MI_01#7&2a8b1d5&0&0001#{504be32c-ccf6-4d2c-b73f-6f8b3747e22b}\global";
        let output = r"\\?\usb#vid_1c75&pid_0288&mi_01#7&2a8b1d5&0&0001#{6dc23320-ab33-4ce4-80d4-bbb3ebbf2814}\global";
        let other = r"\\?\usb#vid_1c75&pid_0289&mi_01#7&11111&0&0001#{6dc23320-ab33-4ce4-80d4-bbb3ebbf2814}\global";
        // Different names (a renamed port), one device by id.
        assert!(same_midi_device("winmm", "Arturia In", input, &out("Arturia Out B", output)));
        assert!(!same_midi_device("winmm", "Arturia In", input, &out("Arturia Out B", other)));
        assert_eq!(device_of_id("plain-id"), "plain-id");
    }

    #[test]
    fn old_tracks_load_with_the_internal_destination_and_internal_is_not_written() {
        let track = Track::new(TrackId(1), "Keys", TrackKind::Midi, ChannelFormat::Mono, [0; 3]);
        let value = serde_json::to_value(&track).unwrap();
        assert!(value.get("midi_output").is_none(), "the default destination is not written, so older builds read the file unchanged");
        let restored: Track = serde_json::from_value(value).unwrap();
        assert_eq!(restored.midi_output, MidiOutput::Internal);
    }

    #[test]
    fn the_echo_opt_in_is_off_for_old_files_unwritten_while_off_and_saved_when_on() {
        let mut track = Track::new(TrackId(1), "Keys", TrackKind::Midi, ChannelFormat::Mono, [0; 3]);
        let value = serde_json::to_value(&track).unwrap();
        assert!(value.get("allow_echo_to_device").is_none());
        assert!(!serde_json::from_value::<Track>(value).unwrap().allow_echo_to_device);
        track.allow_echo_to_device = true;
        let value = serde_json::to_value(&track).unwrap();
        assert_eq!(value["allow_echo_to_device"], true);
        assert!(serde_json::from_value::<Track>(value).unwrap().allow_echo_to_device);
    }

    #[test]
    fn external_destination_round_trips() {
        let mut track = Track::new(TrackId(1), "Synth", TrackKind::Midi, ChannelFormat::Mono, [0; 3]);
        track.midi_output = MidiOutput::External {
            port: MidiOutPort { backend: "winmm".into(), name: "KeyStep Pro MIDI OUT".into(), ordinal: 0, id: "usb-path".into() },
            channel: MidiOutChannel::Channel(9),
            offset_ms: -3.5,
        };
        let text = serde_json::to_string(&track).unwrap();
        let restored: Track = serde_json::from_str(&text).unwrap();
        assert_eq!(restored.midi_output, track.midi_output);
        let as_recorded = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms: 0.0 };
        assert_eq!(serde_json::from_str::<MidiOutput>(&serde_json::to_string(&as_recorded).unwrap()).unwrap(), as_recorded);
    }

    #[test]
    fn minimal_external_object_loads_with_defaults() {
        let v: MidiOutput = serde_json::from_value(serde_json::json!({"external": {"port": {"backend": "winmm", "name": "Synth"}}})).unwrap();
        assert_eq!(
            v,
            MidiOutput::External {
                port: MidiOutPort { backend: "winmm".into(), name: "Synth".into(), ordinal: 0, id: String::new() },
                channel: MidiOutChannel::AsRecorded,
                offset_ms: 0.0
            }
        );
    }

    #[test]
    fn hostile_offsets_are_clamped() {
        let out = |offset_ms| MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms };
        assert_eq!(out(1e9).offset_ms(), MAX_MIDI_OUT_OFFSET_MS);
        assert_eq!(out(-1e9).offset_ms(), -MAX_MIDI_OUT_OFFSET_MS);
        assert_eq!(out(f32::NAN).offset_ms(), 0.0);
        assert_eq!(MidiOutput::Internal.offset_ms(), 0.0);
        assert_eq!(MidiOutChannel::Channel(200).resolve(3), 200 & 0x0F);
        assert_eq!(MidiOutChannel::AsRecorded.resolve(3), 3);
    }
}

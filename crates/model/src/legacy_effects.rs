//! The inherited built-in effects retired in stage 1 (issue #264, ADR 0040).
//!
//! Builds before v0.6.0 shipped twelve devices inherited from the SoundCraft fork that transit
//! retires. A session that still names one in an insert slot (or Drum Synth in an instrument
//! slot) opens with that slot emptied: the device's parameters go with it, and so do the MIDI
//! mappings, automation lanes and rack macro targets that pointed at it. The load notes list each
//! track, slot and device by name and end with one status line, the count. Nothing else on the
//! track changes. Alpha software: no other shim, and the same rule as the retired Subtractive
//! Synth ([`crate::legacy_synth`], ADR 0033).

use crate::{AutoParam, Control, Session, Strip, TrackId};

/// The retired devices: plug-in id and the name the panels showed.
pub const RETIRED_EFFECTS: [(&str, &str); 12] = [
    ("pitch_shifter", "Pitch Shifter"),
    ("drum_synth", "Drum Synth"),
    ("channel_strip", "Channel Strip"),
    ("maximizer", "Maximizer"),
    ("de_esser", "De-Esser"),
    ("rectifier", "Rectifier"),
    ("lofi", "Lo-Fi"),
    ("time_shift", "Time Shift"),
    ("mod_delay", "Mod Delay"),
    ("chorus", "Chorus"),
    ("flanger", "Flanger"),
    ("phaser", "Phaser"),
];

/// The name of a retired device, `None` for any other plug-in id.
#[must_use]
pub fn retired_name(plugin: &str) -> Option<&'static str> {
    RETIRED_EFFECTS.iter().find(|(id, _)| *id == plugin).map(|(_, name)| *name)
}

/// The one-line status of a load that removed devices: `N insert(s) used retired built-in effects and were removed`.
#[must_use]
pub fn status_line(removed: usize) -> String {
    format!("{removed} insert(s) used retired built-in effects and were removed")
}

/// One occupied slot of a track that names a retired device: the automation slot, the label the
/// load report gives it and the device's name.
fn retired_slots(t: &crate::Track) -> Vec<(u8, String, &'static str)> {
    let mut out = Vec::new();
    if let Some(name) = t.instrument.as_ref().and_then(|i| retired_name(&i.plugin)) {
        out.push((AutoParam::INSTRUMENT_SLOT, "instrument".to_string(), name));
    }
    for (slot, ins) in t.mixer.inserts.iter().enumerate() {
        if let (Some(name), Ok(n)) = (ins.as_ref().and_then(|i| retired_name(&i.plugin)), u8::try_from(slot)) {
            out.push((n, format!("insert {}", (b'A' + n.min(25)) as char), name));
        }
    }
    out
}

/// Does any track's instrument or insert slot still name a retired device?
#[must_use]
pub fn uses_retired_effects(s: &Session) -> bool {
    s.tracks.iter().any(|t| !retired_slots(t).is_empty())
}

/// Empty every slot that names a retired device, drop what targeted it, and push the load notes:
/// one line per device, then the status line. Returns how many slots were emptied (0: nothing
/// changed, no note).
pub fn retire_inherited_effects(s: &mut Session) -> usize {
    let found: Vec<(TrackId, Vec<(u8, String, &'static str)>)> =
        s.tracks.iter().map(|t| (t.id, retired_slots(t))).filter(|(_, slots)| !slots.is_empty()).collect();
    let mut notes = Vec::new();
    let mut removed = 0;
    for (id, slots) in found {
        let name = s.track(id).map(|t| t.name.clone()).unwrap_or_default();
        for (slot, label, device) in slots {
            removed += 1;
            let mut lanes = 0;
            if let Some(t) = s.track_mut(id) {
                if slot == AutoParam::INSTRUMENT_SLOT {
                    t.instrument = None;
                } else if let Some(x) = t.mixer.inserts.get_mut(usize::from(slot)) {
                    *x = None;
                }
                let before = t.automation.len();
                t.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { slot: x, .. } if *x == slot));
                lanes = before - t.automation.len();
            }
            let maps = s
                .midi_mappings
                .remove_where(|m| matches!((&m.strip, &m.control), (Strip::Track(x), Control::Plugin { slot: y, .. }) if *x == id && *y == slot));
            let macros: usize = s
                .tracks
                .iter_mut()
                .filter_map(|r| r.rack.as_mut())
                .flat_map(|r| r.macros.iter_mut())
                .map(|m| {
                    let n = m.mappings.len();
                    m.mappings.retain(|x| !(x.track == id && x.slot == slot));
                    n - m.mappings.len()
                })
                .sum();
            let mut dropped = Vec::new();
            if lanes > 0 {
                dropped.push(format!("{lanes} automation lane(s)"));
            }
            if maps > 0 {
                dropped.push(format!("{maps} MIDI mapping(s)"));
            }
            if macros > 0 {
                dropped.push(format!("{macros} rack macro target(s)"));
            }
            let tail = if dropped.is_empty() { String::new() } else { format!("; dropped {}", dropped.join(", ")) };
            notes.push(format!("track \"{name}\", {label}: the retired {device} was removed{tail}"));
        }
    }
    if removed > 0 {
        notes.push(status_line(removed));
        s.load_notes.extend(notes);
    }
    removed
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{ChannelFormat, Insert, MacroMapping, Mapping, MidiSource, Rack, SourceKind, TrackKind};

    fn map(t: TrackId, slot: u8, param: &str, cc: u8) -> Mapping {
        let source = MidiSource { port: "kb".into(), channel: Some(0), kind: SourceKind::Cc, number: cc };
        Mapping::new(source, crate::ControlTarget { strip: Strip::Track(t), control: Control::Plugin { slot, param: param.into() } })
    }

    #[test]
    fn the_list_names_twelve_devices_and_no_kept_one() {
        assert_eq!(RETIRED_EFFECTS.len(), 12);
        assert_eq!(retired_name("chorus"), Some("Chorus"));
        for kept in ["compressor", "expander_gate", "eq_7band", "eq_1band", "room_reverb", "plate_reverb", "saturator", "gain", "macro_oscillator"] {
            assert_eq!(retired_name(kept), None, "{kept}");
        }
    }

    #[test]
    fn a_retired_slot_is_emptied_what_pointed_at_it_goes_and_the_rest_of_the_track_stays() {
        let mut s = Session::default();
        let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
        let b = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
        let host = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Rack"));
        let mut kept = Insert::new("compressor");
        kept.params.insert("ratio".into(), 8.0);
        {
            let t = s.track_mut(a).unwrap();
            t.mixer.inserts[0] = Some(kept.clone());
            t.mixer.inserts[1] = Some(Insert::new("chorus"));
            t.mixer.inserts[3] = Some(Insert::new("maximizer"));
            t.lane_mut(&AutoParam::Plugin { slot: 1, param: "rate".into() }).set_point(0, 1.0);
            t.lane_mut(&AutoParam::Plugin { slot: 0, param: "ratio".into() }).set_point(0, 1.0);
            t.lane_mut(&AutoParam::Volume).set_point(0, -3.0);
        }
        s.track_mut(b).unwrap().mixer.inserts[1] = Some(Insert::new("chorus"));
        s.midi_mappings.set(map(a, 1, "rate", 1)).unwrap();
        s.midi_mappings.set(map(a, 0, "ratio", 2)).unwrap();
        s.midi_mappings.set(map(b, 1, "rate", 3)).unwrap();
        let mut rack = Rack::default();
        for (t, slot) in [(a, 1), (a, 0), (a, 3)] {
            rack.macros[0].mappings.push(MacroMapping { track: t, slot, param: "p".into(), ..MacroMapping::default() });
        }
        s.track_mut(host).unwrap().rack = Some(rack);

        assert!(uses_retired_effects(&s));
        assert_eq!(retire_inherited_effects(&mut s), 3);
        assert!(!uses_retired_effects(&s));

        let t = s.track(a).unwrap();
        assert_eq!(t.mixer.inserts[0].as_ref(), Some(&kept), "a kept insert is untouched");
        assert!(t.mixer.inserts[1].is_none() && t.mixer.inserts[3].is_none());
        assert!(t.lane(&AutoParam::Plugin { slot: 1, param: "rate".into() }).is_none());
        assert!(t.lane(&AutoParam::Plugin { slot: 0, param: "ratio".into() }).is_some());
        assert!(t.lane(&AutoParam::Volume).is_some());
        assert_eq!(s.midi_mappings.len(), 1, "only the kept insert's mapping stays");
        let left = &s.track(host).unwrap().rack.as_ref().unwrap().macros[0].mappings;
        assert_eq!(left.iter().map(|m| (m.track, m.slot)).collect::<Vec<_>>(), vec![(a, 0)]);

        assert_eq!(s.load_notes.len(), 4, "{:?}", s.load_notes);
        assert_eq!(
            s.load_notes[0],
            "track \"Vox\", insert B: the retired Chorus was removed; dropped 1 automation lane(s), 1 MIDI mapping(s), 1 rack macro target(s)"
        );
        assert_eq!(s.load_notes[1], "track \"Vox\", insert D: the retired Maximizer was removed; dropped 1 rack macro target(s)");
        assert_eq!(s.load_notes[2], "track \"Gtr\", insert B: the retired Chorus was removed; dropped 1 MIDI mapping(s)");
        assert_eq!(s.load_notes[3], "3 insert(s) used retired built-in effects and were removed");
        assert_eq!(retire_inherited_effects(&mut s), 0, "a second pass changes nothing");
        assert_eq!(s.load_notes.len(), 4);
    }

    #[test]
    fn a_retired_instrument_slot_is_emptied_too() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Kit"));
        s.track_mut(t).unwrap().instrument = Some(Insert::new("drum_synth"));
        assert_eq!(retire_inherited_effects(&mut s), 1);
        assert!(s.track(t).unwrap().instrument.is_none());
        assert_eq!(s.load_notes[0], "track \"Kit\", instrument: the retired Drum Synth was removed");
    }

    #[test]
    fn a_session_without_retired_devices_is_untouched() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
        let before = s.clone();
        assert_eq!(retire_inherited_effects(&mut s), 0);
        assert_eq!(s, before);
    }

    #[test]
    fn from_json_as_saved_returns_the_session_as_stored_only_when_it_changed() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().mixer.inserts[2] = Some(Insert::new("phaser"));
        let text = s.to_json().unwrap();
        let (loaded, stored) = Session::from_json_as_saved(&text).unwrap();
        assert!(loaded.track(t).unwrap().mixer.inserts[2].is_none());
        assert_eq!(stored.unwrap().track(t).unwrap().mixer.inserts[2].as_ref().unwrap().plugin, "phaser");
        let saved = loaded.to_json().unwrap();
        assert!(!saved.contains("phaser") && !saved.contains("load_notes"));
        assert!(Session::from_json_as_saved(&saved).unwrap().1.is_none(), "a saved removal loads clean");
    }
}

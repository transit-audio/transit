//! The retired Subtractive Synth (issue #234, ADR 0033).
//!
//! Builds before v0.6.0 had a built-in Subtractive Synth (`subtractive_synth`) and gave every new
//! instrument track one. It is gone: the Macro Oscillator is the default instrument. A session
//! that still names the old id gets a Macro Oscillator with its defaults in the slot when it
//! loads. The old synth's parameters mean nothing to the new one, so they are dropped, and so are
//! the MIDI mappings, automation lanes and rack macro targets that pointed at them; each
//! substitution is reported in the session's load notes. Alpha software: no other shim.

use crate::{AutoParam, Control, Insert, Session, Strip, TrackId};

/// The plug-in id of the retired synth.
pub const RETIRED_SYNTH_ID: &str = "subtractive_synth";

/// The plug-in id of the default instrument (the Macro Oscillator; `transit-dsp` registers it).
pub const DEFAULT_INSTRUMENT: &str = "macro_oscillator";

/// How the one-line status of a load that substituted begins and ends: `N track(s) used ...`.
pub const RETIRED_SYNTH_NOTE: &str = "used the retired Subtractive Synth and now carry the Macro Oscillator";

/// Does any track's instrument slot still name the retired synth?
#[must_use]
pub fn uses_retired_synth(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == RETIRED_SYNTH_ID))
}

/// Put a default Macro Oscillator in every instrument slot that names the retired synth, drop what
/// targeted its parameters, and push the load notes: one line per track that lost something, then
/// the summary. Returns how many tracks were substituted (0: nothing changed, no note).
pub fn retire_subtractive_synth(s: &mut Session) -> usize {
    let swapped: Vec<TrackId> =
        s.tracks.iter().filter(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == RETIRED_SYNTH_ID)).map(|t| t.id).collect();
    if swapped.is_empty() {
        return 0;
    }
    let mut notes = Vec::new();
    for &id in &swapped {
        let (mut lanes, mut name) = (0, String::new());
        if let Some(t) = s.track_mut(id) {
            t.instrument = Some(Insert::new(DEFAULT_INSTRUMENT));
            let before = t.automation.len();
            t.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { slot, .. } if *slot == AutoParam::INSTRUMENT_SLOT));
            lanes = before - t.automation.len();
            name = t.name.clone();
        }
        let mut dropped = Vec::new();
        if lanes > 0 {
            dropped.push(format!("{lanes} automation lane(s)"));
        }
        let maps = s.midi_mappings.remove_where(
            |m| matches!((&m.strip, &m.control), (Strip::Track(x), Control::Plugin { slot, .. }) if *x == id && *slot == AutoParam::INSTRUMENT_SLOT),
        );
        if maps > 0 {
            dropped.push(format!("{maps} MIDI mapping(s)"));
        }
        let macros: usize = s
            .tracks
            .iter_mut()
            .filter_map(|r| r.rack.as_mut())
            .flat_map(|r| r.macros.iter_mut())
            .map(|m| {
                let n = m.mappings.len();
                m.mappings.retain(|x| !(x.track == id && x.slot == AutoParam::INSTRUMENT_SLOT));
                n - m.mappings.len()
            })
            .sum();
        if macros > 0 {
            dropped.push(format!("{macros} rack macro target(s)"));
        }
        if !dropped.is_empty() {
            notes.push(format!("track \"{name}\": the retired Subtractive Synth's parameters are gone; dropped {}", dropped.join(", ")));
        }
    }
    notes.push(format!("{} track(s) {RETIRED_SYNTH_NOTE}", swapped.len()));
    s.load_notes.extend(notes);
    swapped.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{ChannelFormat, MacroMapping, Mapping, MidiSource, Rack, SourceKind, TrackKind};

    fn old_synth() -> Insert {
        let mut i = Insert::new(RETIRED_SYNTH_ID);
        i.params.insert("cutoff".into(), 300.0);
        i.preset = "Bass".into();
        i
    }

    fn plugin_lane(slot: u8, param: &str) -> AutoParam {
        AutoParam::Plugin { slot, param: param.into() }
    }

    fn map(t: TrackId, slot: u8, param: &str, cc: u8) -> Mapping {
        let source = MidiSource { port: "kb".into(), channel: Some(0), kind: SourceKind::Cc, number: cc };
        Mapping::new(source, crate::ControlTarget { strip: Strip::Track(t), control: Control::Plugin { slot, param: param.into() } })
    }

    #[test]
    fn a_slot_naming_the_old_synth_gets_a_default_macro_oscillator_and_what_pointed_at_it_goes() {
        let mut s = Session::default();
        let old = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
        let other = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Pad"));
        let mut keep = Insert::new(DEFAULT_INSTRUMENT);
        keep.params.insert("harmonics".into(), 12.0);
        s.track_mut(old).unwrap().instrument = Some(old_synth());
        s.track_mut(other).unwrap().instrument = Some(keep.clone());
        for t in [old, other] {
            s.track_mut(t).unwrap().lane_mut(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "cutoff")).set_point(0, 1.0);
            s.track_mut(t).unwrap().lane_mut(&plugin_lane(0, "gain")).set_point(0, 1.0);
        }
        s.midi_mappings.set(map(old, AutoParam::INSTRUMENT_SLOT, "cutoff", 1)).unwrap();
        s.midi_mappings.set(map(old, 0, "gain", 2)).unwrap();
        s.midi_mappings.set(map(other, AutoParam::INSTRUMENT_SLOT, "harmonics", 3)).unwrap();
        let mut rack = Rack::default();
        for (t, slot) in [(old, AutoParam::INSTRUMENT_SLOT), (old, 0), (other, AutoParam::INSTRUMENT_SLOT)] {
            rack.macros[0].mappings.push(MacroMapping { track: t, slot, param: "p".into(), ..MacroMapping::default() });
        }
        let host = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Rack"));
        s.track_mut(host).unwrap().rack = Some(rack);

        assert!(uses_retired_synth(&s));
        assert_eq!(retire_subtractive_synth(&mut s), 1);
        assert!(!uses_retired_synth(&s));

        let now = s.track(old).unwrap().instrument.as_ref().unwrap();
        assert_eq!(now.plugin, "macro_oscillator");
        assert!(now.params.is_empty(), "defaults: the old parameters are dropped");
        assert_eq!(now.preset, Insert::new("x").preset);
        assert_eq!(s.track(other).unwrap().instrument.as_ref().unwrap().params, keep.params, "other tracks are untouched");
        assert!(s.track(old).unwrap().lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "cutoff")).is_none());
        assert!(s.track(old).unwrap().lane(&plugin_lane(0, "gain")).is_some(), "an insert's lane stays");
        assert!(s.track(other).unwrap().lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "cutoff")).is_some());
        assert_eq!(s.midi_mappings.len(), 2, "the old instrument's mapping went, the insert's and the other track's stay");
        let kept = &s.track(host).unwrap().rack.as_ref().unwrap().macros[0].mappings;
        assert_eq!(kept.iter().map(|m| (m.track, m.slot)).collect::<Vec<_>>(), vec![(old, 0), (other, AutoParam::INSTRUMENT_SLOT)]);

        assert_eq!(s.load_notes.len(), 2, "{:?}", s.load_notes);
        assert!(
            s.load_notes[0].contains("\"Lead\"") && s.load_notes[0].contains("1 automation lane(s), 1 MIDI mapping(s), 1 rack macro target(s)"),
            "{:?}",
            s.load_notes
        );
        assert_eq!(s.load_notes[1], "1 track(s) used the retired Subtractive Synth and now carry the Macro Oscillator");
        assert_eq!(retire_subtractive_synth(&mut s), 0, "a second pass changes nothing");
        assert_eq!(s.load_notes.len(), 2);
    }

    #[test]
    fn a_session_without_the_old_synth_is_untouched() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(Insert::new(DEFAULT_INSTRUMENT));
        let before = s.clone();
        assert_eq!(retire_subtractive_synth(&mut s), 0);
        assert_eq!(s, before);
    }

    #[test]
    fn from_json_as_saved_returns_the_session_as_stored_only_when_it_changed() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        s.track_mut(t).unwrap().instrument = Some(old_synth());
        let text = s.to_json().unwrap();
        let (loaded, stored) = Session::from_json_as_saved(&text).unwrap();
        assert_eq!(loaded.track(t).unwrap().instrument.as_ref().unwrap().plugin, DEFAULT_INSTRUMENT);
        assert_eq!(stored.unwrap().track(t).unwrap().instrument.as_ref().unwrap().plugin, RETIRED_SYNTH_ID);
        let saved = loaded.to_json().unwrap();
        assert!(!saved.contains(RETIRED_SYNTH_ID) && !saved.contains("load_notes"));
        assert!(Session::from_json_as_saved(&saved).unwrap().1.is_none(), "a saved substitution loads clean");
    }
}

//! The MIDI effect slot of a track (#201, ADR 0036): a chain of MIDI effects ahead of the
//! instrument, processed in order, each bypassable. Ableton's MIDI effects sit before the
//! instrument in the device chain and Logic's MIDI FX slots above the instrument slot; both run
//! them top to bottom (left to right) and let each be switched off.

use crate::{AutoParam, InsertUid, Track};
use std::collections::BTreeMap;

/// MIDI effects a track holds at most. The mix keeps a fixed slot per effect.
pub const MAX_MIDI_EFFECTS: usize = 8;

/// The [`AutoParam::Plugin`] slot of a track's first MIDI effect; effect `i` is this plus `i`.
/// Above every insert slot and below the rack macros, the instrument and the trim lane, so a
/// build that predates the slot reads such a lane and ignores it (no insert has that slot).
pub const MIDI_EFFECT_SLOT: u8 = 240;

/// One MIDI effect in a track's chain.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MidiEffectSlot {
    /// Stable identity while the slot lives (see [`InsertUid`]): the mix knows its chain by it.
    #[serde(skip)]
    pub uid: InsertUid,
    /// Registry id of a built-in MIDI effect (`transit_dsp::midi_fx`). Hosted MIDI effects are not
    /// accepted yet; an id this build does not know (or none) passes events through unchanged.
    #[serde(default)]
    pub plugin: String,
    #[serde(default, deserialize_with = "crate::mixer::finite_params")]
    pub params: BTreeMap<String, f32>,
    #[serde(default)]
    pub bypass: bool,
    /// Opaque effect state (hosted MIDI effects, later), standard base64. Built-ins keep none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Loads a chain of at most [`MAX_MIDI_EFFECTS`]: a hand-edited file with more keeps the first
/// ones (logged), so every effect has an automation slot and index-based edits reach them all.
pub(crate) fn at_most_max<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MidiEffectSlot>, D::Error> {
    let mut chain: Vec<MidiEffectSlot> = serde::Deserialize::deserialize(d)?;
    if chain.len() > MAX_MIDI_EFFECTS {
        log::warn!("a track had {} MIDI effects; the first {MAX_MIDI_EFFECTS} are kept", chain.len());
        chain.truncate(MAX_MIDI_EFFECTS);
    }
    Ok(chain)
}

impl MidiEffectSlot {
    pub fn new(plugin: impl Into<String>) -> Self {
        MidiEffectSlot { uid: InsertUid::default(), plugin: plugin.into(), params: BTreeMap::new(), bypass: false, state: None }
    }
}

impl AutoParam {
    /// The [`AutoParam::Plugin`] slot of MIDI effect `index` ([`MIDI_EFFECT_SLOT`] + index).
    #[must_use]
    pub fn midi_effect_slot(index: usize) -> Option<u8> {
        (index < MAX_MIDI_EFFECTS).then(|| MIDI_EFFECT_SLOT + index as u8)
    }

    /// The MIDI effect index a [`AutoParam::Plugin`] slot names, if it names one.
    #[must_use]
    pub fn midi_effect_index(slot: u8) -> Option<usize> {
        slot.checked_sub(MIDI_EFFECT_SLOT).map(usize::from).filter(|i| *i < MAX_MIDI_EFFECTS)
    }
}

impl Track {
    /// Adds `fx` at `at` (the end when `None` or past it); later effects and their automation lanes
    /// move down one. `None`, changing nothing, when the chain is full.
    pub fn add_midi_effect(&mut self, fx: MidiEffectSlot, at: Option<usize>) -> Option<usize> {
        if self.midi_effects.len() >= MAX_MIDI_EFFECTS {
            return None;
        }
        let at = at.unwrap_or(usize::MAX).min(self.midi_effects.len());
        self.midi_effects.insert(at, fx);
        self.remap_midi_effect_lanes(|i| Some(if i >= at { i + 1 } else { i }));
        Some(at)
    }

    /// Removes MIDI effect `index` with its automation lanes; later effects and their lanes move
    /// up one.
    pub fn remove_midi_effect(&mut self, index: usize) -> Option<MidiEffectSlot> {
        if index >= self.midi_effects.len() {
            return None;
        }
        let fx = self.midi_effects.remove(index);
        self.remap_midi_effect_lanes(|i| match i.cmp(&index) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(i - 1),
        });
        Some(fx)
    }

    /// Moves MIDI effect `from` to position `to`; the effects between shift by one and every
    /// automation lane follows its effect. False, changing nothing, when either is out of range or
    /// they are equal.
    pub fn move_midi_effect(&mut self, from: usize, to: usize) -> bool {
        let n = self.midi_effects.len();
        if from == to || from >= n || to >= n {
            return false;
        }
        let fx = self.midi_effects.remove(from);
        self.midi_effects.insert(to, fx);
        self.remap_midi_effect_lanes(|i| {
            Some(if i == from {
                to
            } else if from < to && i > from && i <= to {
                i - 1
            } else if to < from && i >= to && i < from {
                i + 1
            } else {
                i
            })
        });
        true
    }

    /// Re-points each MIDI effect lane at `map(index)`, dropping it on `None`.
    fn remap_midi_effect_lanes(&mut self, map: impl Fn(usize) -> Option<usize>) {
        self.automation.retain_mut(|l| {
            let AutoParam::Plugin { slot, .. } = &mut l.param else { return true };
            let Some(i) = AutoParam::midi_effect_index(*slot) else { return true };
            match map(i).and_then(AutoParam::midi_effect_slot) {
                Some(s) => {
                    *slot = s;
                    true
                }
                None => false,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelFormat, TRACK_COLORS, TrackId, TrackKind};

    fn track(ids: &[&str]) -> Track {
        let mut t = Track::new(TrackId(1), "Keys", TrackKind::Instrument, ChannelFormat::Stereo, TRACK_COLORS[0]);
        for id in ids {
            t.add_midi_effect(MidiEffectSlot::new(*id), None);
        }
        t
    }

    fn order(t: &Track) -> String {
        t.midi_effects.iter().map(|f| f.plugin.as_str()).collect()
    }

    fn lane(i: usize) -> AutoParam {
        AutoParam::Plugin { slot: AutoParam::midi_effect_slot(i).unwrap(), param: "x".into() }
    }

    fn lane_values(t: &Track) -> Vec<Option<f32>> {
        (0..t.midi_effects.len()).map(|i| t.lane(&lane(i)).map(|l| l.points[0].value)).collect()
    }

    #[test]
    fn slots_and_indices_round_trip_and_stay_clear_of_the_others() {
        for i in 0..MAX_MIDI_EFFECTS {
            assert_eq!(AutoParam::midi_effect_slot(i).and_then(AutoParam::midi_effect_index), Some(i));
        }
        assert_eq!(AutoParam::midi_effect_slot(MAX_MIDI_EFFECTS), None);
        for other in [0u8, 9, 25, AutoParam::INSTRUMENT_SLOT, u8::MAX, u8::MAX - 2] {
            assert_eq!(AutoParam::midi_effect_index(other), None, "slot {other}");
        }
    }

    #[test]
    fn add_remove_and_move_carry_their_lanes() {
        let mut t = track(&["a", "b", "c"]);
        for i in 0..3 {
            t.lane_mut(&lane(i)).set_point(0, i as f32);
        }
        assert_eq!(t.add_midi_effect(MidiEffectSlot::new("n"), Some(1)), Some(1));
        assert_eq!(order(&t), "anbc");
        assert_eq!(lane_values(&t), vec![Some(0.0), None, Some(1.0), Some(2.0)]);
        assert!(t.move_midi_effect(3, 0));
        assert_eq!(order(&t), "canb");
        assert_eq!(lane_values(&t), vec![Some(2.0), Some(0.0), None, Some(1.0)]);
        assert!(t.move_midi_effect(0, 3));
        assert_eq!(order(&t), "anbc");
        assert_eq!(t.remove_midi_effect(0).map(|f| f.plugin), Some("a".into()));
        assert_eq!(order(&t), "nbc");
        assert_eq!(lane_values(&t), vec![None, Some(1.0), Some(2.0)]);
        assert_eq!(t.automation.len(), 2, "the removed effect's lane went with it");
    }

    #[test]
    fn edits_out_of_range_change_nothing() {
        let mut t = track(&["a", "b"]);
        assert!(!t.move_midi_effect(0, 0));
        assert!(!t.move_midi_effect(0, 2));
        assert!(!t.move_midi_effect(5, 0));
        assert!(t.remove_midi_effect(2).is_none());
        let mut full = track(&["a"; MAX_MIDI_EFFECTS]);
        assert_eq!(full.add_midi_effect(MidiEffectSlot::new("z"), None), None);
        assert_eq!(full.midi_effects.len(), MAX_MIDI_EFFECTS);
        assert_eq!(order(&t), "ab");
    }

    #[test]
    fn a_slot_round_trips_and_old_tracks_load_with_none() {
        let mut t = track(&["midi_transpose"]);
        t.midi_effects[0].params.insert("semitones".into(), 7.0);
        t.midi_effects[0].bypass = true;
        let json = serde_json::to_string(&t).unwrap();
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v.as_object_mut().unwrap().remove("midi_effects");
        let old: Track = serde_json::from_value(v).unwrap();
        assert!(old.midi_effects.is_empty());
        let plain = Track::new(TrackId(2), "A", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        assert!(!serde_json::to_string(&plain).unwrap().contains("midi_effects"), "an empty chain is not written");
        let mut many = serde_json::to_value(&t).unwrap();
        many["midi_effects"] = serde_json::Value::Array(vec![many["midi_effects"][0].clone(); MAX_MIDI_EFFECTS + 3]);
        let capped: Track = serde_json::from_value(many).unwrap();
        assert_eq!(capped.midi_effects.len(), MAX_MIDI_EFFECTS, "a hand-edited chain keeps the first eight");
        let hostile: MidiEffectSlot = serde_json::from_str(r#"{"plugin":"midi_velocity","params":{"scale":null,"min":3}}"#).unwrap();
        assert_eq!(hostile.params.len(), 1, "a non-finite value is dropped");
    }
}

//! Racks (ADR 0034): a group track that sums its members and plays them from one MIDI input.
//!
//! A rack is a routing folder ([`TrackKind::Folder`](crate::TrackKind::Folder)) whose
//! [`Track::rack`](crate::Track::rack) is set. Its members are ordinary instrument tracks inside the
//! folder: each keeps its own clips, inserts, fader and automation, and the folder sums them
//! through its bus exactly as a group does. What the rack adds:
//!
//! - **One MIDI input for all members.** The rack's own MIDI (its clips and live input while it is
//!   armed or monitored) reaches every member whose key zone and velocity zone both contain the
//!   note. This is Ableton's Instrument Rack (chains with key and velocity zones) and Logic's Track
//!   Stack (a summing stack whose main track plays every subtrack); Logic has no zones, so zones
//!   follow Ableton.
//! - **Fades.** Each zone has a fade range at each edge. Inside a fade range the note's velocity is
//!   scaled by a linear ramp ([`Zone::gain`]); the key-zone and velocity-zone factors multiply. This
//!   is Ableton's rule: "Key zone fade ranges attenuate the velocities of notes entering a chain",
//!   and the same for velocity zones (Live manual, Instrument, Drum and Effect Racks, Zones). Only
//!   Ableton's chain select zones fade the chain's output level, and those are out of scope.
//! - **Macros.** Eight knobs on the rack, each mapped to any number of member parameters (an
//!   instrument or an insert parameter, by its automation address) with a range and a curve, like
//!   Ableton's Macro Controls and Logic's Smart Controls. The range is the MIDI Learn range model
//!   (ADR 0028: `min`/`max` in 0..1 of the parameter's normalized travel, inverted when
//!   `min > max`); the curve is new here ([`MacroMapping::travel`]).
//!
//! Pure data, like the rest of the model; the mix reads it on the audio thread without allocating.

use crate::{MidiSource, TrackId};

/// Macros per rack (Ableton's default bank, Logic's eight Smart Controls on screen).
pub const RACK_MACROS: usize = 8;

/// Most members a rack plays (its MIDI fan-out is a fixed table on the audio thread). Members
/// past this are refused by the commands and silent in a hand-edited file.
pub const MAX_RACK_MEMBERS: usize = 16;

/// The [`AutoParam::Plugin`](crate::AutoParam::Plugin) slot of a rack's macro lanes (`param` is
/// `macro1` to `macro8`). A plain slot like the instrument's, so a build that predates racks reads
/// the lane and ignores it.
pub const MACRO_SLOT: u8 = u8::MAX - 2;

/// The automation `param` of macro `i` (0-based): `macro1` to `macro8`.
#[must_use]
pub fn macro_param(i: usize) -> String {
    format!("macro{}", i + 1)
}

/// The macro index (0-based) of an automation `param` written by [`macro_param`].
#[must_use]
pub fn macro_index(param: &str) -> Option<usize> {
    param.strip_prefix("macro").and_then(|n| n.parse::<usize>().ok()).and_then(|n| n.checked_sub(1)).filter(|&i| i < RACK_MACROS)
}

fn top() -> u8 {
    127
}

/// A key zone (note numbers) or a velocity zone (velocities), inclusive at both ends, with a fade
/// range inside each edge: full level from `fade_low` to `fade_high`, ramping down to the edges.
/// [`Zone::normalized`] keeps `low <= fade_low <= fade_high <= high <= 127`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Zone {
    #[serde(default)]
    pub low: u8,
    #[serde(default = "top")]
    pub high: u8,
    /// Where the fade-in ends: values from `low` up to here ramp up. Equal to `low`: no fade.
    #[serde(default)]
    pub fade_low: u8,
    /// Where the fade-out starts: values from here up to `high` ramp down. Equal to `high`: no fade.
    #[serde(default = "top")]
    pub fade_high: u8,
}

impl Default for Zone {
    fn default() -> Self {
        Zone::FULL
    }
}

impl Zone {
    /// The whole range, no fades: every note (or velocity) at full level.
    pub const FULL: Zone = Zone { low: 0, high: 127, fade_low: 0, fade_high: 127 };

    /// A zone without fades.
    #[must_use]
    pub fn new(low: u8, high: u8) -> Self {
        Zone { low, high, fade_low: low, fade_high: high }.normalized()
    }

    /// The zone with every value in 0..=127 and in order (`low <= fade_low <= fade_high <= high`):
    /// a swapped pair is put back in order, and a fade handle outside the zone is moved to its edge.
    #[must_use]
    pub fn normalized(self) -> Self {
        let (a, b) = (self.low.min(127), self.high.min(127));
        let (low, high) = (a.min(b), a.max(b));
        let fade_low = self.fade_low.clamp(low, high);
        let fade_high = self.fade_high.clamp(fade_low, high);
        Zone { low, high, fade_low, fade_high }
    }

    /// Whether `x` is inside the zone (fade ranges included).
    #[must_use]
    pub fn contains(&self, x: u8) -> bool {
        (self.low..=self.high).contains(&x)
    }

    /// The fade factor at `x`: 0 outside the zone, 1 from `fade_low` to `fade_high`, and a linear
    /// ramp in each fade range that never reaches 0 inside the zone. With a fade-in of `n` values
    /// (`fade_low - low = n`), `low + k` gets `(k + 1) / (n + 1)`: a fade from 60 to 63 gives 60,
    /// 61, 62, 63 the factors 0.25, 0.5, 0.75 and 1. The fade-out mirrors it. Exact in `f32` for
    /// the small ratios involved, so tests compare it with `==`.
    #[must_use]
    pub fn gain(&self, x: u8) -> f32 {
        let z = self.normalized();
        if !z.contains(x) {
            return 0.0;
        }
        if x < z.fade_low {
            return f32::from(x - z.low + 1) / f32::from(z.fade_low - z.low + 1);
        }
        if x > z.fade_high {
            return f32::from(z.high - x + 1) / f32::from(z.high - z.fade_high + 1);
        }
        1.0
    }
}

/// One member of a rack: an instrument track inside the rack's folder, and its zones.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RackMember {
    /// Defaulted only for the field rule: a member without a track (id 0) never plays.
    #[serde(default)]
    pub track: TrackId,
    #[serde(default)]
    pub key_zone: Zone,
    #[serde(default)]
    pub vel_zone: Zone,
}

impl RackMember {
    #[must_use]
    pub fn new(track: TrackId) -> Self {
        RackMember { track, key_zone: Zone::FULL, vel_zone: Zone::FULL }
    }

    /// The velocity this member plays a note-on of `key` at `velocity` with, or `None` when a zone
    /// excludes it. The two fade factors multiply and the product scales the velocity, rounded to
    /// the nearest step and never below 1 (a velocity of 0 would be a note-off): a note inside both
    /// zones always sounds.
    #[must_use]
    pub fn velocity(&self, key: u8, velocity: u8) -> Option<u8> {
        let g = self.key_zone.gain(key) * self.vel_zone.gain(velocity);
        if g <= 0.0 {
            return None;
        }
        Some((f32::from(velocity.min(127)) * g).round().clamp(1.0, 127.0) as u8)
    }
}

/// A parameter's plain range, as the parameter table gave it when the mapping was made: what the
/// audio thread needs to turn a macro's travel into the parameter's own units without the table.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamRange {
    #[serde(default)]
    pub lo: f32,
    #[serde(default = "one")]
    pub hi: f32,
    /// A logarithmic taper (`lo > 0`): the travel is spread geometrically.
    #[serde(default)]
    pub log: bool,
    /// A whole-number parameter (a choice, a toggle, semitones): the value is rounded.
    #[serde(default)]
    pub step: bool,
}

impl Default for ParamRange {
    fn default() -> Self {
        ParamRange { lo: 0.0, hi: 1.0, log: false, step: false }
    }
}

impl ParamRange {
    /// The value at `t` (0..1) of the travel: the same rule as `ParamInfo::from_normalized`.
    #[must_use]
    pub fn plain(&self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let v = if self.log && self.lo > 0.0 && self.hi > 0.0 { self.lo * (self.hi / self.lo).powf(t) } else { self.lo + (self.hi - self.lo) * t };
        let v = if self.step { v.round() } else { v };
        let (a, b) = (self.lo.min(self.hi), self.lo.max(self.hi));
        if v.is_finite() { v.clamp(a, b) } else { a }
    }
}

fn one() -> f32 {
    1.0
}

/// One target of a macro: a member's parameter by its automation address.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MacroMapping {
    /// Defaulted only for the field rule (as `slot` and `param`): a mapping without a target is idle.
    #[serde(default)]
    pub track: TrackId,
    /// The automation slot: an insert (0 to 9) or [`AutoParam::INSTRUMENT_SLOT`](crate::AutoParam::INSTRUMENT_SLOT).
    #[serde(default)]
    pub slot: u8,
    /// The parameter's stable id.
    #[serde(default)]
    pub param: String,
    /// The part of the parameter's normalized travel the macro spans (ADR 0028's range):
    /// the macro at 0 gives `min`, at 1 `max`. `min > max` inverts the mapping.
    #[serde(default)]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    /// The response curve, -1..1: 0 is linear, positive is slow at first (exponential), negative
    /// is fast at first (logarithmic). See [`shape`].
    #[serde(default)]
    pub curve: f32,
    /// The parameter's plain range when the mapping was made.
    #[serde(default)]
    pub range: ParamRange,
}

/// The macro curve: `x` (0..1) raised to `4^curve`, so curve 0 is linear, 0.5 squares, 1 is the
/// fourth power, and -0.5 is the square root. Monotonic, 0 at 0 and 1 at 1 for every curve.
#[must_use]
pub fn shape(x: f32, curve: f32) -> f32 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    let c = if curve.is_finite() { curve.clamp(-1.0, 1.0) } else { 0.0 };
    if c == 0.0 { x } else { x.powf(4f32.powf(c)) }
}

impl MacroMapping {
    /// The parameter's normalized travel when the macro is at `x`: `min + (max - min) * shape(x)`.
    #[must_use]
    pub fn travel(&self, x: f32) -> f32 {
        let (min, max) = (finite01(self.min), finite01(self.max));
        (min + (max - min) * shape(x, self.curve)).clamp(0.0, 1.0)
    }

    /// The parameter's value, in its own units, when the macro is at `x`.
    #[must_use]
    pub fn plain(&self, x: f32) -> f32 {
        self.range.plain(self.travel(x))
    }
}

fn finite01(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// One macro knob.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Macro {
    /// Empty: "Macro n".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The knob, 0..1.
    #[serde(default)]
    pub value: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<MacroMapping>,
    /// A MIDI Learn source for the knob (ADR 0028 rules: consumed by default, takeover per
    /// machine). Kept here rather than in `Session::midi_mappings`, so a build that predates racks
    /// still opens the file (it ignores the whole rack).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi: Option<MidiSource>,
}

/// A rack's members and macros. See the module docs.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Rack {
    #[serde(default)]
    pub members: Vec<RackMember>,
    #[serde(default)]
    pub macros: [Macro; RACK_MACROS],
}

impl Rack {
    #[must_use]
    pub fn member(&self, track: TrackId) -> Option<&RackMember> {
        self.members.iter().find(|m| m.track == track)
    }

    pub fn member_mut(&mut self, track: TrackId) -> Option<&mut RackMember> {
        self.members.iter_mut().find(|m| m.track == track)
    }

    /// The display name of macro `i`.
    #[must_use]
    pub fn macro_name(&self, i: usize) -> String {
        self.macros.get(i).map(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Macro {}", i + 1))
    }

    /// The macro mappings to `(track, slot, param)`, with their macro index.
    pub fn mappings_to<'a>(&'a self, track: TrackId, slot: u8, param: &'a str) -> impl Iterator<Item = (usize, &'a MacroMapping)> + 'a {
        self.macros
            .iter()
            .enumerate()
            .flat_map(|(i, m)| m.mappings.iter().map(move |x| (i, x)))
            .filter(move |(_, x)| x.track == track && x.slot == slot && x.param == param)
    }
}

impl crate::Session {
    /// The rack `member` plays in, and its entry there: `member` is a MIDI or instrument track
    /// inside a rack's folder and listed among its members. Allocation-free (the mix calls it on
    /// the audio thread).
    #[must_use]
    pub fn rack_of(&self, member: &crate::Track) -> Option<(&crate::Track, &RackMember)> {
        if !member.kind.is_midi() {
            return None;
        }
        let rack = self.track(member.folder?)?;
        // Only the members the rack plays (the first `MAX_RACK_MEMBERS` that are present).
        self.rack_members(rack).find(|(t, _)| t.id == member.id).map(|(_, m)| (rack, m))
    }

    /// The members of `rack` that play: listed, present, inside its folder, MIDI or instrument
    /// tracks. In the rack's order.
    pub fn rack_members<'a>(&'a self, rack: &'a crate::Track) -> impl Iterator<Item = (&'a crate::Track, &'a RackMember)> + 'a {
        rack.rack
            .as_ref()
            .filter(|_| rack.is_rack())
            .into_iter()
            .flat_map(|r| r.members.iter())
            .filter_map(move |m| self.track(m.track).filter(|t| t.folder == Some(rack.id) && t.kind.is_midi()).map(|t| (t, m)))
            .take(MAX_RACK_MEMBERS)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]
    use super::*;

    #[test]
    fn fades_ramp_linearly_and_never_reach_zero_inside_the_zone() {
        let z = Zone { low: 60, fade_low: 63, fade_high: 70, high: 72 };
        let g: Vec<f32> = (58..=74).map(|k| z.gain(k)).collect();
        assert_eq!(
            g,
            [0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0 / 3.0, 1.0 / 3.0, 0.0, 0.0],
            "58..=74: outside, fade-in 60..63, full 63..70, fade-out 70..72, outside"
        );
        assert_eq!(Zone::FULL.gain(0), 1.0);
        assert_eq!(Zone::FULL.gain(127), 1.0);
        assert_eq!(Zone::new(10, 10).gain(10), 1.0);
    }

    #[test]
    fn normalized_puts_handles_in_order() {
        let z = Zone { low: 90, high: 30, fade_low: 10, fade_high: 200 }.normalized();
        assert_eq!(z, Zone { low: 30, high: 90, fade_low: 30, fade_high: 90 });
        let z = Zone { low: 0, high: 255, fade_low: 50, fade_high: 40 }.normalized();
        assert_eq!(z, Zone { low: 0, high: 127, fade_low: 50, fade_high: 50 });
    }

    #[test]
    fn a_member_scales_velocity_by_both_fades_and_excludes_outside_notes() {
        let m = RackMember { track: TrackId(1), key_zone: Zone { low: 60, fade_low: 63, fade_high: 127, high: 127 }, vel_zone: Zone::FULL };
        assert_eq!(m.velocity(59, 100), None, "below the key zone");
        assert_eq!(m.velocity(60, 100), Some(25));
        assert_eq!(m.velocity(61, 100), Some(50));
        assert_eq!(m.velocity(62, 100), Some(75));
        assert_eq!(m.velocity(63, 100), Some(100));
        let m = RackMember { vel_zone: Zone { low: 1, fade_low: 4, fade_high: 100, high: 101 }, ..m };
        assert_eq!(m.velocity(61, 101), Some(25), "key 0.5 x velocity 0.5 of 101 = 25.25");
        assert_eq!(m.velocity(61, 102), None, "above the velocity zone");
        assert_eq!(m.velocity(60, 1), Some(1), "0.25 x 0.25 x 1 rounds to 0, kept at 1");
    }

    #[test]
    fn macro_travel_honors_range_inversion_and_curve() {
        let mut m = MacroMapping {
            track: TrackId(2),
            slot: 0,
            param: "cutoff".into(),
            min: 0.25,
            max: 0.75,
            curve: 0.0,
            range: ParamRange { lo: 0.0, hi: 100.0, log: false, step: false },
        };
        assert_eq!(m.travel(0.0), 0.25);
        assert_eq!(m.travel(1.0), 0.75);
        assert_eq!(m.plain(0.5), 50.0);
        m.curve = 0.5;
        assert_eq!(shape(0.5, 0.5), 0.25);
        assert_eq!(m.plain(0.5), 37.5, "0.25 + 0.5 x 0.5^2 = 0.375 of 0..100");
        m.curve = -0.5;
        assert_eq!(shape(0.25, -0.5), 0.5);
        m.curve = 0.0;
        (m.min, m.max) = (1.0, 0.0);
        assert_eq!(m.plain(0.25), 75.0, "inverted");
        m.range = ParamRange { lo: 20.0, hi: 20_000.0, log: true, step: false };
        (m.min, m.max) = (0.0, 1.0);
        assert!((m.plain(0.5) - 632.455_5).abs() < 0.01, "log taper: the geometric middle");
        m.range = ParamRange { lo: 0.0, hi: 3.0, log: false, step: true };
        assert_eq!(m.plain(0.6), 2.0, "a choice lands on a whole step");
    }

    #[test]
    fn macro_params_round_trip() {
        for i in 0..RACK_MACROS {
            assert_eq!(macro_index(&macro_param(i)), Some(i));
        }
        assert_eq!(macro_index("macro0"), None);
        assert_eq!(macro_index("macro9"), None);
        assert_eq!(macro_index("cutoff"), None);
    }

    #[test]
    fn an_empty_rack_object_loads_with_defaults() {
        let r: Rack = serde_json::from_str("{}").unwrap();
        assert_eq!(r, Rack::default());
        let m: RackMember = serde_json::from_str(r#"{"track":3}"#).unwrap();
        assert_eq!(m, RackMember::new(TrackId(3)));
        let z: Zone = serde_json::from_str(r#"{"low":40}"#).unwrap();
        assert_eq!(z, Zone { low: 40, high: 127, fade_low: 0, fade_high: 127 });
        assert_eq!(z.normalized().fade_low, 40);
    }
}

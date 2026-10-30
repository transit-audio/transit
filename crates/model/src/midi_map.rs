//! MIDI Learn mappings (ADR 0028): a controller message to a mixer or track control.
//!
//! Pure data. The session keeps a [`MappingSet`] (tracks by id); the host keeps another per
//! machine, the controller profile (tracks by position). [`resolve`] merges the two with the
//! session winning.

use crate::{AutoParam, TrackId};
use std::fmt;
use std::str::FromStr;

/// Which track a target addresses: by id (session mappings) or by 1-based position in session
/// order (the controller profile, whose mappings must make sense in any session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Strip {
    Track(TrackId),
    Position(u16),
}

/// What a mapping moves.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Control {
    Volume,
    Pan,
    Mute,
    Solo,
    Arm,
    /// The level of send slot `n`.
    Send(u8),
    /// A plug-in parameter (slice 2): the insert slot (`AutoParam::INSTRUMENT_SLOT` for the
    /// track's instrument, as in automation lanes) and the stable parameter id, never an index.
    /// Continuous: the knob's 0..1 is the parameter's normalized travel.
    Plugin {
        slot: u8,
        param: String,
    },
    /// Macro `n` (0-based) of a rack (ADR 0034). Continuous: the knob's 0..1 is the macro's value.
    /// Session mappings to a macro are stored on the rack ([`crate::Macro::midi`]), never in
    /// [`MappingSet`], so a build that predates racks still opens the file.
    Macro(u8),
}

impl Control {
    /// Mute, solo and arm are on/off; the rest are continuous.
    pub fn is_button(&self) -> bool {
        matches!(self, Control::Mute | Control::Solo | Control::Arm)
    }

    pub fn label(&self) -> String {
        match self {
            Control::Volume => "Volume".into(),
            Control::Pan => "Pan".into(),
            Control::Mute => "Mute".into(),
            Control::Solo => "Solo".into(),
            Control::Arm => "Record arm".into(),
            Control::Send(n) => format!("Send {}", (b'A' + (*n).min(25)) as char),
            Control::Plugin { slot, param } if *slot == AutoParam::INSTRUMENT_SLOT => format!("Instrument {param}"),
            Control::Plugin { slot, param } => format!("Insert {} {param}", (b'A' + (*slot).min(25)) as char),
            Control::Macro(n) => format!("Macro {}", u16::from(*n) + 1),
        }
    }
}

/// A control of one strip: the unit a mapping targets. Text form `track:3:volume`,
/// `strip:2:send:0` (`strip` positions count from 1), `track:3:plugin:a:cutoff` and
/// `track:3:plugin:instrument:cutoff` (the parameter id is the rest of the text, colons included).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ControlTarget {
    pub strip: Strip,
    pub control: Control,
}

impl fmt::Display for ControlTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.strip {
            Strip::Track(t) => write!(f, "track:{}:", t.0)?,
            Strip::Position(n) => write!(f, "strip:{n}:")?,
        }
        match &self.control {
            Control::Volume => f.write_str("volume"),
            Control::Pan => f.write_str("pan"),
            Control::Mute => f.write_str("mute"),
            Control::Solo => f.write_str("solo"),
            Control::Arm => f.write_str("arm"),
            Control::Send(n) => write!(f, "send:{n}"),
            Control::Plugin { slot, param } if *slot == AutoParam::INSTRUMENT_SLOT => write!(f, "plugin:instrument:{param}"),
            Control::Plugin { slot, param } => write!(f, "plugin:{}:{param}", (b'a' + (*slot).min(25)) as char),
            Control::Macro(n) => write!(f, "macro:{}", u16::from(*n) + 1),
        }
    }
}

impl FromStr for ControlTarget {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("`{s}` is not a control (track:<id>:volume|pan|mute|solo|arm|send:<slot>|plugin:<slot>:<param id>)");
        // kind, id, control, and the rest (a send slot, or a plug-in's slot and parameter id).
        let mut it = s.splitn(4, ':');
        let strip = match (it.next(), it.next().map(str::parse::<u64>)) {
            (Some("track"), Some(Ok(id))) => Strip::Track(TrackId(id)),
            (Some("strip"), Some(Ok(n))) if (1..=u64::from(u16::MAX)).contains(&n) => Strip::Position(n as u16),
            _ => return Err(bad()),
        };
        let (name, rest) = (it.next(), it.next());
        let control = match (name, rest) {
            (Some("volume"), None) => Control::Volume,
            (Some("pan"), None) => Control::Pan,
            (Some("mute"), None) => Control::Mute,
            (Some("solo"), None) => Control::Solo,
            (Some("arm"), None) => Control::Arm,
            (Some("send"), Some(n)) => match n.parse::<u8>() {
                Ok(n) if usize::from(n) < crate::SEND_SLOTS => Control::Send(n),
                _ => return Err(bad()),
            },
            (Some("macro"), Some(n)) => match n.parse::<u8>() {
                Ok(n) if (1..=crate::RACK_MACROS).contains(&usize::from(n)) => Control::Macro(n - 1),
                _ => return Err(bad()),
            },
            (Some("plugin"), Some(rest)) => {
                let (slot, param) = rest.split_once(':').ok_or_else(bad)?;
                let slot = if slot.eq_ignore_ascii_case("instrument") {
                    AutoParam::INSTRUMENT_SLOT
                } else {
                    match slot.as_bytes() {
                        [c] if c.is_ascii_alphabetic() && usize::from(c.to_ascii_lowercase() - b'a') < crate::INSERT_SLOTS => {
                            c.to_ascii_lowercase() - b'a'
                        }
                        _ => return Err(bad()),
                    }
                };
                if param.is_empty() || param.len() > 512 || param.chars().any(char::is_control) {
                    return Err(bad());
                }
                Control::Plugin { slot, param: param.to_string() }
            }
            _ => return Err(bad()),
        };
        Ok(ControlTarget { strip, control })
    }
}

impl serde::Serialize for ControlTarget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> serde::Deserialize<'de> for ControlTarget {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Cc,
    Note,
}

/// Where a mapping listens. `channel` is zero-based; `None` accepts every channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MidiSource {
    pub port: String,
    #[serde(default)]
    pub channel: Option<u8>,
    pub kind: SourceKind,
    pub number: u8,
}

impl MidiSource {
    /// Whether two sources can be triggered by the same message.
    pub fn overlaps(&self, other: &MidiSource) -> bool {
        self.port == other.port
            && self.kind == other.kind
            && self.number == other.number
            && match (self.channel, other.channel) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    }

    /// "KeyStep ch 1 CC 74" for menus and the window.
    pub fn describe(&self) -> String {
        let ch = self.channel.map_or_else(|| "any ch".to_string(), |c| format!("ch {}", c + 1));
        match self.kind {
            SourceKind::Cc => format!("{} {ch} CC {}", self.port, self.number),
            SourceKind::Note => format!("{} {ch} note {}", self.port, self.number),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapMode {
    /// The control follows once the knob passes the control's current value.
    #[default]
    Pickup,
    /// The control jumps to the knob.
    Jump,
    /// The control moves toward the knob by a scaled step, so both meet at the end of the knob's
    /// travel (Logic's default, Ableton's Value Scaling).
    Scaled,
}

impl MapMode {
    /// The per-machine takeover default (ADR 0028, amendment 2026-10-25): scaled.
    pub fn takeover_default() -> Self {
        MapMode::Scaled
    }

    pub fn label(self) -> &'static str {
        match self {
            MapMode::Scaled => "Scaled",
            MapMode::Jump => "Jump",
            MapMode::Pickup => "Pickup",
        }
    }
}

fn zero() -> f32 {
    0.0
}
fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mapping {
    pub source: MidiSource,
    pub target: ControlTarget,
    #[serde(default)]
    pub mode: MapMode,
    /// The part of the control's travel (0..1) the knob spans: knob 0 is `min`, knob 127 is `max`.
    #[serde(default = "zero")]
    pub min: f32,
    #[serde(default = "one")]
    pub max: f32,
    /// Buttons: flip on press (true) or follow the key or CC >= 64 (false).
    #[serde(default = "yes")]
    pub toggle: bool,
    /// Also deliver the message to the track's routes (the instrument, a recording). Off by
    /// default: a mapped message drives its control and nothing else, as in Logic and Ableton.
    #[serde(default)]
    pub pass_through: bool,
}

impl Mapping {
    pub fn new(source: MidiSource, target: ControlTarget) -> Self {
        Mapping { source, target, mode: MapMode::Pickup, min: 0.0, max: 1.0, toggle: true, pass_through: false }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.source.channel.is_some_and(|c| c > 15) {
            return Err("channel must be 1 to 16".into());
        }
        if self.source.number > 127 {
            return Err("CC or note number must be 0 to 127".into());
        }
        if self.source.port.is_empty() {
            return Err("a mapping needs a port".into());
        }
        if !(self.min.is_finite() && self.max.is_finite() && (0.0..=1.0).contains(&self.min) && (0.0..=1.0).contains(&self.max)) {
            return Err("min and max must be between 0 and 1".into());
        }
        if (self.max - self.min).abs() < 0.001 {
            return Err("min and max must differ".into());
        }
        Ok(())
    }
}

/// One store of mappings. A target has at most one source and a source at most one target.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MappingSet {
    items: Vec<Mapping>,
}

impl MappingSet {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn iter(&self) -> std::slice::Iter<'_, Mapping> {
        self.items.iter()
    }
    pub fn for_target(&self, target: &ControlTarget) -> Option<&Mapping> {
        self.items.iter().find(|m| m.target == *target)
    }

    /// Add `m`, replacing the mapping of the same target and any that the same source drives.
    pub fn set(&mut self, m: Mapping) -> Result<(), String> {
        m.validate()?;
        self.items.retain(|old| old.target != m.target && !old.source.overlaps(&m.source));
        self.items.push(m);
        Ok(())
    }

    /// Remove the mapping of `target`; whether there was one.
    pub fn remove(&mut self, target: &ControlTarget) -> bool {
        let n = self.items.len();
        self.items.retain(|m| m.target != *target);
        self.items.len() != n
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Profile,
}

/// A mapping with its target resolved to a track of the open session.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub scope: Scope,
    pub mapping: Mapping,
    pub track: TrackId,
    pub control: Control,
    /// A session mapping shares this profile mapping's source or control: it is stored but idle.
    pub overridden: bool,
}

/// The mappings in force for a session whose tracks are `tracks` (in order): every session mapping
/// whose track exists, then every profile mapping whose strip exists, flagged `overridden` when a
/// session mapping has the same control or an overlapping source. Idle ones stay in the list for
/// display; callers drive only those not overridden.
pub fn resolve(tracks: &[TrackId], session: &MappingSet, profile: &MappingSet) -> Vec<Resolved> {
    let track_of = |strip: Strip| match strip {
        Strip::Track(t) => tracks.contains(&t).then_some(t),
        Strip::Position(n) => tracks.get(usize::from(n).checked_sub(1)?).copied(),
    };
    let mut out: Vec<Resolved> = Vec::new();
    for (scope, set) in [(Scope::Session, session), (Scope::Profile, profile)] {
        for m in set.iter() {
            let Some(track) = track_of(m.target.strip) else { continue };
            let overridden = scope == Scope::Profile
                && out.iter().any(|s| {
                    s.scope == Scope::Session && ((s.track == track && s.control == m.target.control) || s.mapping.source.overlaps(&m.source))
                });
            out.push(Resolved { scope, mapping: m.clone(), track, control: m.target.control.clone(), overridden });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn src(port: &str, cc: u8) -> MidiSource {
        MidiSource { port: port.into(), channel: Some(0), kind: SourceKind::Cc, number: cc }
    }
    fn vol(t: u64) -> ControlTarget {
        ControlTarget { strip: Strip::Track(TrackId(t)), control: Control::Volume }
    }
    fn plug(t: u64, slot: u8, param: &str) -> ControlTarget {
        ControlTarget { strip: Strip::Track(TrackId(t)), control: Control::Plugin { slot, param: param.into() } }
    }
    fn pos(n: u16, c: Control) -> ControlTarget {
        ControlTarget { strip: Strip::Position(n), control: c }
    }

    #[test]
    fn targets_round_trip_through_text() {
        for t in [
            vol(7),
            pos(2, Control::Send(3)),
            pos(1, Control::Arm),
            vol(1),
            plug(3, 0, "cutoff"),
            plug(3, 9, "Cell/1:Gain"),
            pos(2, Control::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "7".into() }),
            ControlTarget { strip: Strip::Track(TrackId(5)), control: Control::Macro(7) },
        ] {
            assert_eq!(t.to_string().parse::<ControlTarget>().unwrap(), t);
        }
        for bad in [
            "",
            "track:x:volume",
            "strip:0:pan",
            "track:1:volume:2",
            "track:1:send",
            "track:1:send:99",
            "mixer:1:pan",
            "track:1:plugin",
            "track:1:plugin:a",
            "track:1:plugin:a:",
            "track:1:plugin:k:x",
            "track:1:plugin:aa:x",
            "track:1:plugin:1:x",
            "track:1:pan:x",
            "track:1:macro:0",
            "track:1:macro:9",
            "track:1:macro",
        ] {
            assert!(bad.parse::<ControlTarget>().is_err(), "{bad}");
        }
    }

    #[test]
    fn plug_in_targets_use_the_pickers_addressing_and_stable_ids() {
        let t: ControlTarget = "track:4:plugin:C:Cutoff_Hz".parse().unwrap();
        assert_eq!(t, plug(4, 2, "Cutoff_Hz"), "slot letters are case-insensitive, the id keeps its case");
        assert_eq!(t.to_string(), "track:4:plugin:c:Cutoff_Hz");
        let i: ControlTarget = "strip:1:plugin:instrument:cutoff".parse().unwrap();
        assert_eq!(i.control, Control::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "cutoff".into() });
        // The same text the automation picker writes for the lane.
        assert_eq!(AutoParam::parse("plugin:c:Cutoff_Hz"), Some(AutoParam::Plugin { slot: 2, param: "Cutoff_Hz".into() }));
        assert!(!t.control.is_button());
        assert_eq!(t.control.label(), "Insert C Cutoff_Hz");
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"track:4:plugin:c:Cutoff_Hz\"");
        assert_eq!(serde_json::from_str::<ControlTarget>(&json).unwrap(), t);
    }

    #[test]
    fn a_plug_in_parameter_maps_once_per_parameter() {
        let mut set = MappingSet::default();
        set.set(Mapping::new(src("KeyStep", 74), plug(1, 0, "cutoff"))).unwrap();
        set.set(Mapping::new(src("KeyStep", 75), plug(1, 0, "reso"))).unwrap();
        assert_eq!(set.len(), 2, "different parameters of one device are different targets");
        set.set(Mapping::new(src("KeyStep", 76), plug(1, 0, "cutoff"))).unwrap();
        assert_eq!((set.len(), set.for_target(&plug(1, 0, "cutoff")).unwrap().source.number), (2, 76));
        let tracks = [TrackId(1)];
        let r = resolve(&tracks, &set, &MappingSet::default());
        assert!(r.iter().all(|x| matches!(x.control, Control::Plugin { .. })));
    }

    #[test]
    fn a_set_keeps_one_source_per_target_and_one_target_per_source() {
        let mut set = MappingSet::default();
        set.set(Mapping::new(src("KeyStep", 74), vol(1))).unwrap();
        set.set(Mapping::new(src("KeyStep", 75), vol(1))).unwrap();
        assert_eq!(set.len(), 1, "same target replaces");
        set.set(Mapping::new(src("KeyStep", 75), vol(2))).unwrap();
        assert_eq!(set.len(), 1, "same source replaces");
        assert_eq!(set.for_target(&vol(2)).unwrap().source.number, 75);
        assert!(set.remove(&vol(2)) && !set.remove(&vol(2)));
    }

    #[test]
    fn invalid_mappings_are_refused() {
        let mut set = MappingSet::default();
        let mut m = Mapping::new(src("P", 1), vol(1));
        m.min = 0.5;
        m.max = 0.5;
        assert!(set.set(m.clone()).is_err());
        m.max = 2.0;
        assert!(set.set(m.clone()).is_err());
        m.max = 1.0;
        m.source.channel = Some(16);
        assert!(set.set(m.clone()).is_err());
        m.source.channel = None;
        m.source.number = 200;
        assert!(set.set(m).is_err());
        assert!(set.is_empty());
    }

    #[test]
    fn old_files_load_with_no_mappings_and_a_mapping_loads_with_defaults() {
        let set: MappingSet = serde_json::from_str("[]").unwrap();
        assert!(set.is_empty());
        let m: Mapping = serde_json::from_str(r#"{"source":{"port":"KeyStep","kind":"cc","number":74},"target":"track:4:pan"}"#).unwrap();
        assert_eq!((m.mode, m.min, m.max, m.toggle, m.source.channel), (MapMode::Pickup, 0.0, 1.0, true, None));
        assert!(!m.pass_through, "mappings consume by default");
        assert_eq!(serde_json::from_str::<Mapping>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
    }

    #[test]
    fn the_session_wins_a_conflict_with_the_profile() {
        let tracks = [TrackId(10), TrackId(11)];
        let mut session = MappingSet::default();
        let mut profile = MappingSet::default();
        // Profile: knob 74 moves strip 1's volume, knob 75 moves strip 2's volume.
        profile.set(Mapping::new(src("KeyStep", 74), pos(1, Control::Volume))).unwrap();
        profile.set(Mapping::new(src("KeyStep", 75), pos(2, Control::Volume))).unwrap();
        // Session: knob 74 drives track 11's pan (source conflict), knob 80 drives track 11's volume (control conflict).
        session.set(Mapping::new(src("KeyStep", 74), ControlTarget { strip: Strip::Track(TrackId(11)), control: Control::Pan })).unwrap();
        session.set(Mapping::new(src("KeyStep", 80), vol(11))).unwrap();
        let r = resolve(&tracks, &session, &profile);
        assert_eq!(r.len(), 4);
        let overridden: Vec<_> = r.iter().filter(|x| x.overridden).map(|x| (x.mapping.source.number, x.track.0)).collect();
        assert_eq!(overridden, vec![(74, 10), (75, 11)]);
        assert!(r.iter().filter(|x| x.scope == Scope::Session).all(|x| !x.overridden));
    }

    #[test]
    fn mappings_to_missing_tracks_do_not_resolve() {
        let mut session = MappingSet::default();
        let mut profile = MappingSet::default();
        session.set(Mapping::new(src("P", 1), vol(99))).unwrap();
        profile.set(Mapping::new(src("P", 2), pos(5, Control::Mute))).unwrap();
        assert!(resolve(&[TrackId(1)], &session, &profile).is_empty());
    }
}

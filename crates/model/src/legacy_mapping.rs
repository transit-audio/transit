//! The Macro Oscillator's note-to-macro mapping (#196) as modulation matrix rows (#243).
//!
//! Builds before #243 gave the Macro Oscillator ten parameters that put the note's velocity and key
//! on its Harmonics, Timbre, Morph and Decay: a curve per source (`velocity_curve`, `key_curve`) and
//! a depth per source and target (`velocity_harmonics` .. `key_decay`). They are gone: the matrix
//! does the same with rows (source Velocity or Key, destination the macro, the depth as the amount,
//! the curve as the curve), and the instrument evaluates a row exactly as it evaluated the mapping,
//! so a session or preset rewritten here renders the same bits.
//!
//! [`migrate_params`] rewrites one instrument's parameters: one row per depth that is not 0 or that
//! something drives (an automation lane, a MIDI mapping, a rack macro), velocity rows first, each
//! source's targets in the order Harmonics, Timbre, Morph, Decay, in the first rows the matrix does
//! not use. [`migrate_session`] does it for every Macro Oscillator of a session as it loads and
//! moves what drove the old parameters onto the rows: a depth's lane, MIDI mapping and macro
//! targets go to the row's amount; a curve's lane and macro targets go to the curve of every row of
//! that source (a lane is copied per row), and its MIDI mapping (a source drives one target) to the
//! first such row's curve, its range rescaled from the old curve's three choices to the matrix
//! curve's five so the same travel picks the same curve.
//!
//! Every build before #243 stored all ten old ids, mostly at depth 0 with nothing driving them. A
//! track like that has nothing to migrate: the ids are stripped silently, so the session opens
//! unmodified and with no load note. Only a track that gets a row or has a lane, a MIDI mapping or
//! a macro target moved (or dropped) counts as migrated.

use crate::{AutoParam, Control, Session, Strip, TrackId};
use std::collections::BTreeMap;

/// The Macro Oscillator's plug-in id.
pub const MACRO_OSC_ID: &str = "macro_oscillator";

/// The matrix's source indices for velocity and key (`SOURCES` in the instrument's matrix).
pub const SOURCE_VELOCITY: f32 = 5.0;
pub const SOURCE_KEY: f32 = 6.0;
/// The matrix's destination indices of Harmonics, Timbre, Morph and Decay, in that order.
pub const DEST_MACROS: [f32; 4] = [2.0, 3.0, 4.0, 5.0];
/// Rows in the matrix.
pub const ROWS: usize = 16;

/// The old mapping, per source: the source index, its curve parameter, and its depth parameters
/// in the order Harmonics, Timbre, Morph, Decay.
pub const OLD_MAPPING: [(f32, &str, [&str; 4]); 2] = [
    (SOURCE_VELOCITY, "velocity_curve", ["velocity_harmonics", "velocity_timbre", "velocity_morph", "velocity_decay"]),
    (SOURCE_KEY, "key_curve", ["key_harmonics", "key_timbre", "key_morph", "key_decay"]),
];

/// How a load note about the migration begins: `N track(s) ...`.
pub const MIGRATED_NOTE: &str = "had the Macro Oscillator's note-to-macro mapping, now rows of its modulation matrix";

/// Added to the load note when a curve's MIDI mapping moved to a source with several rows.
pub const CURVE_MAPPING_NOTE: &str = "a curve's MIDI mapping now drives only the first row of its source";

/// Choices of the old velocity and key curves (Linear, Exponential, S-curve) and of a matrix row's
/// curve (the same three first, then Equal power and Logarithmic).
pub const OLD_CURVE_CHOICES: usize = 3;
pub const ROW_CURVE_CHOICES: usize = 5;

/// A MIDI mapping's travel on an old curve as travel on a row's curve that picks the same choice:
/// the old curve's travel `t` was the value `2 t`, a row curve's is `4 t`.
#[must_use]
pub fn curve_travel(t: f32) -> f32 {
    t * (OLD_CURVE_CHOICES - 1) as f32 / (ROW_CURVE_CHOICES - 1) as f32
}

/// Is `id` one of the old curve parameters?
fn is_old_curve(id: &str) -> bool {
    OLD_MAPPING.iter().any(|(_, curve, _)| *curve == id)
}

/// Is `id` one of the old mapping's parameters?
#[must_use]
pub fn is_old_param(id: &str) -> bool {
    OLD_MAPPING.iter().any(|(_, curve, depths)| *curve == id || depths.contains(&id))
}

/// The id of row `n`'s (1-based) field (`source`, `via`, `dest`, `amount`, `curve`).
#[must_use]
pub fn row_id(n: usize, field: &str) -> String {
    format!("mod{n}_{field}")
}

/// Where each old parameter went: an old id and the new ids it drives (one for a depth, one per
/// row of the source for a curve, none when no row was made).
pub type Moves = Vec<(&'static str, Vec<String>)>;

/// Rewrites one Macro Oscillator's parameters (see the module documentation). `driven` says
/// whether something other than the stored value drives an old parameter (it then gets a row even
/// at depth 0). Returns where each old parameter went; empty when there was nothing to migrate.
pub fn migrate_params(params: &mut BTreeMap<String, f32>, driven: impl Fn(&str) -> bool) -> Moves {
    if !params.keys().any(|k| is_old_param(k)) && !OLD_MAPPING.iter().any(|(_, c, d)| driven(c) || d.iter().any(|x| driven(x))) {
        return Vec::new();
    }
    let used = |p: &BTreeMap<String, f32>, n: usize| p.get(&row_id(n, "source")).is_some_and(|v| *v >= 0.5);
    let mut free = (1..=ROWS).filter(|n| !used(params, *n)).collect::<Vec<_>>().into_iter();
    let mut moves: Moves = Vec::new();
    for (source, curve_id, depths) in OLD_MAPPING {
        let curve = params.get(curve_id).copied().unwrap_or(0.0);
        let mut curve_rows = Vec::new();
        for (depth_id, dest) in depths.into_iter().zip(DEST_MACROS) {
            let depth = params.get(depth_id).copied().unwrap_or(0.0);
            if depth == 0.0 && !driven(depth_id) {
                moves.push((depth_id, Vec::new()));
                continue;
            }
            // Sixteen rows and at most eight old depths: a free row exists unless the matrix
            // already used nine or more, which only a hand-edited file can do.
            let Some(n) = free.next() else {
                moves.push((depth_id, Vec::new()));
                continue;
            };
            params.insert(row_id(n, "source"), source);
            params.insert(row_id(n, "via"), 0.0);
            params.insert(row_id(n, "dest"), dest);
            params.insert(row_id(n, "amount"), depth);
            params.insert(row_id(n, "curve"), curve);
            moves.push((depth_id, vec![row_id(n, "amount")]));
            curve_rows.push(row_id(n, "curve"));
        }
        moves.push((curve_id, curve_rows));
    }
    params.retain(|k, _| !is_old_param(k));
    moves
}

/// The new ids an old parameter moved to.
fn moved<'a>(moves: &'a Moves, old: &str) -> Option<&'a [String]> {
    moves.iter().find(|(o, _)| *o == old).map(|(_, n)| n.as_slice())
}

/// Does any Macro Oscillator of the session still carry the old mapping's parameters?
#[must_use]
pub fn uses_old_mapping(s: &Session) -> bool {
    !old_mapping_tracks(s).is_empty()
}

/// The Macro Oscillator tracks that still carry the old ids.
fn old_mapping_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == MACRO_OSC_ID && i.params.keys().any(|k| is_old_param(k))))
        .map(|t| t.id)
        .collect()
}

/// The old parameters of track `id`'s instrument that something drives: the track's lanes, the
/// session's MIDI mappings, the racks' macros.
fn driven_params(s: &Session, id: TrackId) -> Vec<String> {
    let slot = AutoParam::INSTRUMENT_SLOT;
    let mut driven: Vec<String> = Vec::new();
    if let Some(t) = s.track(id) {
        for l in &t.automation {
            if let AutoParam::Plugin { slot: sl, param } = &l.param
                && *sl == slot
                && is_old_param(param)
            {
                driven.push(param.clone());
            }
        }
    }
    for m in s.midi_mappings.iter() {
        if let (Strip::Track(t), Control::Plugin { slot: sl, param }) = (&m.target.strip, &m.target.control)
            && *t == id
            && *sl == slot
            && is_old_param(param)
        {
            driven.push(param.clone());
        }
    }
    for r in s.tracks.iter().filter_map(|t| t.rack.as_ref()) {
        for m in r.macros.iter().flat_map(|m| m.mappings.iter()) {
            if m.track == id && m.slot == slot && is_old_param(&m.param) {
                driven.push(m.param.clone());
            }
        }
    }
    driven
}

/// Does migrating track `id` change more than stripping unused ids: a depth that is not 0 (it
/// becomes a row) or an old parameter something drives (it moves, or is dropped)?
fn migration_matters(s: &Session, id: TrackId) -> bool {
    let depth_set = s
        .track(id)
        .and_then(|t| t.instrument.as_ref())
        .is_some_and(|i| OLD_MAPPING.iter().flat_map(|(_, _, d)| d.iter()).any(|d| i.params.get(*d).is_some_and(|v| *v != 0.0)));
    depth_set || !driven_params(s, id).is_empty()
}

/// Does loading change the session in a way the user should see (a row made, a lane, MIDI mapping
/// or macro target moved or dropped)? Stripping unused old ids alone does not count.
#[must_use]
pub fn migration_changes(s: &Session) -> bool {
    old_mapping_tracks(s).into_iter().any(|id| migration_matters(s, id))
}

/// Migrates every Macro Oscillator of the session (see the module documentation) and pushes one
/// load note when a track's migration mattered. Returns how many tracks it mattered for.
pub fn migrate_session(s: &mut Session) -> usize {
    let slot = AutoParam::INSTRUMENT_SLOT;
    let ids = old_mapping_tracks(s);
    let mut lost = 0usize;
    let mut noted = 0usize;
    let mut curve_split = false;
    for &id in &ids {
        let driven = driven_params(s, id);
        if migration_matters(s, id) {
            noted += 1;
        }
        let Some(moves) =
            s.track_mut(id).and_then(|t| t.instrument.as_mut()).map(|i| migrate_params(&mut i.params, |p| driven.iter().any(|d| d == p)))
        else {
            continue;
        };
        // Lanes: retargeted, copied once per further row for a curve, dropped when no row was made.
        if let Some(t) = s.track_mut(id) {
            let mut out = Vec::with_capacity(t.automation.len());
            for lane in std::mem::take(&mut t.automation) {
                let old = match &lane.param {
                    AutoParam::Plugin { slot: sl, param } if *sl == slot && is_old_param(param) => param.clone(),
                    _ => {
                        out.push(lane);
                        continue;
                    }
                };
                let to = moved(&moves, &old).unwrap_or_default();
                if to.is_empty() {
                    lost += 1;
                }
                for new in to {
                    let mut l = lane.clone();
                    l.param = AutoParam::Plugin { slot, param: new.clone() };
                    out.push(l);
                }
            }
            t.automation = out;
        }
        // MIDI mappings: a source drives one target, so a curve's goes to its first row, its
        // range rescaled to the row curve's choices.
        let mut gone = 0usize;
        s.midi_mappings.edit(|m| {
            if let (Strip::Track(t), Control::Plugin { slot: sl, param }) = (&m.target.strip, &mut m.target.control)
                && *t == id
                && *sl == slot
                && is_old_param(param)
            {
                let rows = moved(&moves, param).unwrap_or_default();
                match rows.first() {
                    Some(new) => {
                        if is_old_curve(param) {
                            m.min = curve_travel(m.min);
                            m.max = curve_travel(m.max);
                            curve_split |= rows.len() > 1;
                        }
                        *param = new.clone();
                    }
                    None => gone += 1,
                }
            }
        });
        lost += gone;
        let _ = s.midi_mappings.remove_where(|t| matches!((&t.strip, &t.control), (Strip::Track(x), Control::Plugin { slot: sl, param }) if *x == id && *sl == slot && is_old_param(param)));
        // Rack macros: one target per row.
        for r in s.tracks.iter_mut().filter_map(|t| t.rack.as_mut()) {
            for m in r.macros.iter_mut() {
                let mut out = Vec::with_capacity(m.mappings.len());
                for x in std::mem::take(&mut m.mappings) {
                    if !(x.track == id && x.slot == slot && is_old_param(&x.param)) {
                        out.push(x);
                        continue;
                    }
                    let to = moved(&moves, &x.param).unwrap_or_default();
                    if to.is_empty() {
                        lost += 1;
                    }
                    for new in to {
                        let mut y = x.clone();
                        y.param = new.clone();
                        out.push(y);
                    }
                }
                m.mappings = out;
            }
        }
    }
    if noted > 0 {
        use std::fmt::Write as _;
        let mut note = format!("{noted} track(s) {MIGRATED_NOTE}");
        if curve_split {
            let _ = write!(note, "; {CURVE_MAPPING_NOTE}");
        }
        if lost > 0 {
            let _ = write!(note, "; {lost} lane(s), mapping(s) or macro target(s) of a curve with no rows were dropped");
        }
        s.load_notes.push(note);
    }
    noted
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{AutomationLane, ChannelFormat, Insert, MacroMapping, Mapping, MidiSource, Rack, SourceKind, TrackKind};

    fn params(pairs: &[(&str, f32)]) -> BTreeMap<String, f32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn depths_become_rows_in_order_and_the_old_ids_go() {
        let mut p = params(&[
            ("model", 8.0),
            ("velocity_curve", 2.0),
            ("velocity_harmonics", 0.0),
            ("velocity_timbre", 70.0),
            ("velocity_morph", 0.0),
            ("velocity_decay", -25.0),
            ("key_curve", 1.0),
            ("key_harmonics", 0.0),
            ("key_timbre", 0.0),
            ("key_morph", -35.0),
            ("key_decay", 0.0),
        ]);
        let moves = migrate_params(&mut p, |_| false);
        let want = params(&[
            ("model", 8.0),
            ("mod1_source", 5.0),
            ("mod1_via", 0.0),
            ("mod1_dest", 3.0),
            ("mod1_amount", 70.0),
            ("mod1_curve", 2.0),
            ("mod2_source", 5.0),
            ("mod2_via", 0.0),
            ("mod2_dest", 5.0),
            ("mod2_amount", -25.0),
            ("mod2_curve", 2.0),
            ("mod3_source", 6.0),
            ("mod3_via", 0.0),
            ("mod3_dest", 4.0),
            ("mod3_amount", -35.0),
            ("mod3_curve", 1.0),
        ]);
        assert_eq!(p, want);
        assert_eq!(moved(&moves, "velocity_curve").unwrap(), ["mod1_curve", "mod2_curve"]);
        assert_eq!(moved(&moves, "key_morph").unwrap(), ["mod3_amount"]);
        assert!(moved(&moves, "key_timbre").unwrap().is_empty());
        let mut again = p.clone();
        assert!(migrate_params(&mut again, |_| false).is_empty(), "a migrated set has nothing left to migrate");
        assert_eq!(again, p);
    }

    #[test]
    fn a_driven_depth_gets_a_row_at_zero_and_used_rows_are_skipped() {
        let mut p = params(&[("key_timbre", 0.0), ("mod1_source", 1.0), ("mod1_dest", 7.0), ("mod1_amount", 10.0)]);
        let moves = migrate_params(&mut p, |id| id == "key_timbre");
        assert_eq!(p.get("mod2_source"), Some(&6.0));
        assert_eq!(p.get("mod2_amount"), Some(&0.0));
        assert_eq!(p.get("mod1_source"), Some(&1.0), "a row in use stays");
        assert_eq!(moved(&moves, "key_timbre").unwrap(), ["mod2_amount"]);
    }

    #[test]
    fn a_session_migrates_its_lanes_mappings_and_macro_targets() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
        let mut ins = Insert::new(MACRO_OSC_ID);
        ins.params = params(&[("velocity_timbre", 50.0), ("key_morph", 20.0), ("velocity_curve", 1.0)]);
        s.track_mut(t).unwrap().instrument = Some(ins);
        let lane = |p: &str| AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: p.into() };
        for p in ["velocity_timbre", "velocity_curve", "key_curve", "velocity_harmonics"] {
            let mut l = AutomationLane::new(lane(p));
            l.set_point(0, 1.0);
            s.track_mut(t).unwrap().automation.push(l);
        }
        let src = |cc| MidiSource { port: "kb".into(), channel: Some(0), kind: SourceKind::Cc, number: cc };
        let target =
            |p: &str| crate::ControlTarget { strip: Strip::Track(t), control: Control::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: p.into() } };
        s.midi_mappings.set(Mapping::new(src(1), target("key_morph"))).unwrap();
        s.midi_mappings.set(Mapping::new(src(2), target("velocity_curve"))).unwrap();
        let mut rack = Rack::default();
        rack.macros[0].mappings.push(MacroMapping {
            track: t,
            slot: AutoParam::INSTRUMENT_SLOT,
            param: "velocity_curve".into(),
            ..MacroMapping::default()
        });
        let host = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Rack"));
        s.track_mut(host).unwrap().rack = Some(rack);

        assert!(uses_old_mapping(&s));
        assert_eq!(migrate_session(&mut s), 1);
        assert!(!uses_old_mapping(&s));
        let tr = s.track(t).unwrap();
        let p = &tr.instrument.as_ref().unwrap().params;
        // Rows: velocity harmonics (driven by a lane, depth 0), velocity timbre, key morph.
        assert_eq!((p["mod1_dest"], p["mod1_amount"]), (2.0, 0.0));
        assert_eq!((p["mod2_dest"], p["mod2_amount"]), (3.0, 50.0));
        assert_eq!((p["mod3_source"], p["mod3_dest"], p["mod3_amount"]), (6.0, 4.0, 20.0));
        let lanes: Vec<String> = tr.automation.iter().map(|l| l.param.label()).collect();
        assert!(lanes.contains(&"instrument mod2_amount".to_string()), "{lanes:?}");
        assert!(
            lanes.contains(&"instrument mod1_curve".to_string()) && lanes.contains(&"instrument mod2_curve".to_string()),
            "a curve lane per row: {lanes:?}"
        );
        assert!(lanes.contains(&"instrument mod3_curve".to_string()), "the key curve's lane: {lanes:?}");
        assert!(!lanes.iter().any(|l| l.contains("velocity_") || l.contains("key_")), "{lanes:?}");
        assert!(s.midi_mappings.for_target(&target("mod3_amount")).is_some());
        assert!(s.midi_mappings.for_target(&target("mod1_curve")).is_some(), "a curve's mapping goes to its first row");
        let macro_targets: Vec<&str> = s.track(host).unwrap().rack.as_ref().unwrap().macros[0].mappings.iter().map(|m| m.param.as_str()).collect();
        assert_eq!(macro_targets, ["mod1_curve", "mod2_curve"]);
        assert!(s.load_notes.iter().any(|n| n.contains(MIGRATED_NOTE)));
        assert!(
            s.load_notes.iter().any(|n| n.contains(CURVE_MAPPING_NOTE)),
            "the velocity curve's mapping drives one of two rows: {:?}",
            s.load_notes
        );
        let m = s.midi_mappings.for_target(&target("mod1_curve")).unwrap();
        assert_eq!((m.min, m.max), (0.0, 0.5), "the curve mapping's range on the five-choice curve");
        let m = s.midi_mappings.for_target(&target("mod3_amount")).unwrap();
        assert_eq!((m.min, m.max), (0.0, 1.0), "a depth's mapping keeps its range");
    }

    /// A track that stored the old ids at depth 0 with nothing driving them (every build before
    /// #243 did) loses them silently: no row, no load note, and the session is not changed.
    #[test]
    fn unused_old_ids_are_stripped_silently() {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
        let mut ins = Insert::new(MACRO_OSC_ID);
        let mut pairs = vec![("model", 8.0)];
        for (_, curve, depths) in OLD_MAPPING {
            pairs.push((curve, 2.0));
            pairs.extend(depths.iter().map(|d| (*d, 0.0)));
        }
        ins.params = params(&pairs);
        s.track_mut(t).unwrap().instrument = Some(ins);
        assert!(uses_old_mapping(&s));
        assert!(!migration_changes(&s), "nothing to migrate");
        assert_eq!(migrate_session(&mut s), 0);
        assert!(!uses_old_mapping(&s));
        assert!(s.load_notes.is_empty(), "{:?}", s.load_notes);
        assert_eq!(s.track(t).unwrap().instrument.as_ref().unwrap().params, params(&[("model", 8.0)]));
        // A lane on a zero depth makes it matter: the depth gets a row.
        let mut s2 = Session::default();
        let t2 = s2.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
        let mut ins = Insert::new(MACRO_OSC_ID);
        ins.params = params(&pairs);
        s2.track_mut(t2).unwrap().instrument = Some(ins);
        s2.track_mut(t2)
            .unwrap()
            .automation
            .push(AutomationLane::new(AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "key_decay".into() }));
        assert!(migration_changes(&s2));
        assert_eq!(migrate_session(&mut s2), 1);
        assert!(s2.load_notes.iter().any(|n| n.starts_with("1 track(s)") && !n.contains(CURVE_MAPPING_NOTE)), "{:?}", s2.load_notes);
    }

    /// The old curve's travel picks the same choice on a row's curve: full travel S-curve (2), half
    /// travel Exponential (1), as the old three-choice curve did.
    #[test]
    fn curve_travel_picks_the_same_choice() {
        let pick = |t: f32, choices: usize| (t * (choices - 1) as f32).round() as usize;
        for t in [0.0, 0.2, 0.25, 0.5, 0.74, 0.76, 1.0] {
            assert_eq!(pick(curve_travel(t), ROW_CURVE_CHOICES), pick(t, OLD_CURVE_CHOICES), "travel {t}");
        }
        assert_eq!(pick(curve_travel(1.0), ROW_CURVE_CHOICES), 2);
        assert_eq!(pick(curve_travel(0.5), ROW_CURVE_CHOICES), 1);
    }
}

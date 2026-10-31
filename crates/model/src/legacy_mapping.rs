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
//! first such row's curve.

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
    s.tracks.iter().any(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == MACRO_OSC_ID && i.params.keys().any(|k| is_old_param(k))))
}

/// Migrates every Macro Oscillator of the session (see the module documentation) and pushes one
/// load note. Returns how many tracks changed.
pub fn migrate_session(s: &mut Session) -> usize {
    let slot = AutoParam::INSTRUMENT_SLOT;
    let ids: Vec<TrackId> = s
        .tracks
        .iter()
        .filter(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == MACRO_OSC_ID && i.params.keys().any(|k| is_old_param(k))))
        .map(|t| t.id)
        .collect();
    let mut lost = 0usize;
    for &id in &ids {
        // What drives each old parameter: the track's lanes, the session's MIDI mappings, the racks' macros.
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
        // MIDI mappings: a source drives one target, so a curve's goes to its first row.
        let mut gone = 0usize;
        s.midi_mappings.retarget(|target| {
            if let (Strip::Track(t), Control::Plugin { slot: sl, param }) = (&target.strip, &mut target.control)
                && *t == id
                && *sl == slot
                && is_old_param(param)
            {
                match moved(&moves, param).and_then(|n| n.first()) {
                    Some(new) => *param = new.clone(),
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
    if !ids.is_empty() {
        let mut note = format!("{} track(s) {MIGRATED_NOTE}", ids.len());
        if lost > 0 {
            use std::fmt::Write as _;
            let _ = write!(note, "; {lost} lane(s), mapping(s) or macro target(s) of a curve with no rows were dropped");
        }
        s.load_notes.push(note);
    }
    ids.len()
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
    }
}

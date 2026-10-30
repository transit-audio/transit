//! Rack commands (ADR 0034): make a rack, add and remove members, edit zones, macros and their
//! mappings. Every edit command is one undo step; `rack.macro` is merged by the caller's gesture
//! key like a fader drag (`execute_merged`).
//!
//! A rack is a routing folder with [`transit_model::Rack`] set, so it is made the way Group
//! Tracks makes a folder (`track.move_to_new_folder`), and a track joins or leaves it the way it
//! joins or leaves a group: a MIDI or instrument track that moves into a rack's folder becomes a
//! member with full zones ([`join`]), and one that leaves is no longer one. Logic's Track Stack
//! (summing stack) and Ableton's Instrument Rack both build the stack from the selected tracks.

use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::{AutoParam, MAX_RACK_MEMBERS, MacroMapping, ParamRange, RACK_MACROS, Rack, RackMember, Session, Track, Zone};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "rack.new",
            "Group into Rack",
            ["Track"],
            None,
            "{tracks?, name?} — a rack (a group that also plays every member from its MIDI input) of the tracks, placed where the first was",
            has_selection,
            new_rack
        ),
        cmd!("rack.member_add", "Add to Rack", [], None, "{track: rack, member}", always, member_add),
        cmd!(
            "rack.member_remove",
            "Remove from Rack",
            [],
            None,
            "{track: rack, member} — the member leaves the rack's folder too",
            always,
            member_remove
        ),
        cmd!(
            "rack.zone",
            "Set Rack Zone",
            [],
            None,
            "{track: rack, member, key?: {low?, high?, fade_low?, fade_high?}, velocity?: {low?, high?, fade_low?, fade_high?}} (0..127)",
            always,
            zone
        ),
        cmd!("rack.macro", "Set Macro", [], None, "{track: rack, macro: 1..8, value: 0..1} — also writes every mapped parameter", always, set_macro),
        cmd!(
            "rack.macro_map",
            "Map Macro",
            [],
            None,
            "{track: rack, macro: 1..8, member, slot: a..j|instrument, param, min?: 0..1, max?: 0..1, curve?: -1..1}",
            always,
            macro_map
        ),
        cmd!("rack.macro_unmap", "Unmap Macro", [], None, "{track: rack, macro: 1..8, member, slot, param}", always, macro_unmap),
        cmd!("rack.macro_name", "Rename Macro", [], None, "{track: rack, macro: 1..8, name}", always, macro_name),
        cmd!(query "rack.info", "Rack", [], None, "{track: rack} — members, zones and macros", always, info),
    ]
}

/// Adds the MIDI and instrument tracks of `tracks` that are directly in `folder` to its rack (when
/// it is one), with full zones, up to [`MAX_RACK_MEMBERS`]. Called when tracks move into a folder.
pub(crate) fn join(s: &mut Session, folder: TrackId, tracks: &[TrackId]) {
    let joining: Vec<TrackId> =
        tracks.iter().copied().filter(|t| s.track(*t).is_some_and(|x| x.folder == Some(folder) && x.kind.is_midi())).collect();
    let Some(rack) = s.track_mut(folder).filter(|f| f.is_rack()).and_then(|f| f.rack.as_mut()) else { return };
    for t in joining {
        if rack.member(t).is_none() && rack.members.len() < MAX_RACK_MEMBERS {
            rack.members.push(RackMember::new(t));
        }
    }
}

fn rack_param(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = track_param(e, id, p, "track")?.ok_or_else(|| bad(id, "`track` (the rack) required"))?;
    if !e.session().track(t).is_some_and(Track::is_rack) {
        return Err(bad(id, "`track` is not a rack"));
    }
    Ok(t)
}

fn member_param(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    track_param(e, id, p, "member")?.ok_or_else(|| bad(id, "`member` required"))
}

fn macro_param(id: &str, p: &Value) -> Result<usize> {
    p.get("macro")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| (1..=RACK_MACROS).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| bad(id, format!("`macro` must be 1..{RACK_MACROS}")))
}

fn rack_mut<'a>(s: &'a mut Session, id: &str, t: TrackId) -> Result<&'a mut Rack> {
    s.track_mut(t).and_then(|x| x.rack.as_mut()).ok_or_else(|| bad(id, "`track` is not a rack"))
}

fn new_rack(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.new";
    let tracks = tracks_required(e, id, p)?;
    let s = e.session();
    if tracks.iter().any(|t| s.track(*t).is_some_and(|x| x.kind == transit_model::TrackKind::Master)) {
        return Err(bad(id, "a master track cannot join a rack"));
    }
    if tracks.iter().any(|t| s.track(*t).is_some_and(Track::is_folder)) {
        return Err(bad(id, "racks are one level deep: a group cannot join a rack"));
    }
    if tracks.iter().filter(|t| s.track(**t).is_some_and(|x| x.kind.is_midi())).count() > MAX_RACK_MEMBERS {
        return Err(bad(id, format!("a rack plays at most {MAX_RACK_MEMBERS} instruments")));
    }
    let name = match str_param(p, "name").filter(|n| !n.trim().is_empty()) {
        Some(n) => n.to_string(),
        None => (1..).map(|i| format!("Rack {i}")).find(|n| s.track_by_name(n).is_none()).unwrap_or_else(|| "Rack".into()),
    };
    let list: Vec<u64> = tracks.iter().map(|t| t.0).collect();
    // Tracks inside a group leave it first, as Group Tracks does: their output goes back to where
    // the group's went, so the rack's bus takes it, and a rack they leave drops them as members.
    let nested: Vec<u64> = tracks.iter().filter(|t| e.session().track(**t).is_some_and(|x| x.folder.is_some())).map(|t| t.0).collect();
    if !nested.is_empty() {
        super::track::remove_from_folder(e, &json!({"tracks": nested}))?;
    }
    let r = super::track::move_to_folder(e, &json!({"tracks": list, "name": name}))?;
    let folder = TrackId(r.get("folder").and_then(Value::as_u64).ok_or_else(|| bad(id, "no folder made"))?);
    let s = e.session_mut();
    if let Some(f) = s.track_mut(folder) {
        f.rack = Some(Rack::default());
    }
    join(s, folder, &tracks);
    let members = s.track(folder).and_then(|f| f.rack.as_ref()).map_or(0, |r| r.members.len());
    Ok(json!({"rack": folder, "folder": folder, "members": members, "bus": r.get("bus")}))
}

fn member_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.member_add";
    let rack = rack_param(e, id, p)?;
    let member = member_param(e, id, p)?;
    let s = e.session();
    let t = s.track(member).ok_or_else(|| bad(id, "no such member"))?;
    if !t.kind.is_midi() {
        return Err(bad(id, "only MIDI and instrument tracks play in a rack"));
    }
    if s.track(rack).and_then(|r| r.rack.as_ref()).is_some_and(|r| r.member(member).is_none() && r.members.len() >= MAX_RACK_MEMBERS) {
        return Err(bad(id, format!("a rack plays at most {MAX_RACK_MEMBERS} instruments")));
    }
    if t.folder != Some(rack) {
        super::track::move_into_folder(e, &json!({"tracks": [member.0], "folder": rack.0}))?;
    }
    join(e.session_mut(), rack, &[member]);
    Ok(json!({"rack": rack, "member": member}))
}

fn member_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.member_remove";
    let rack = rack_param(e, id, p)?;
    let member = member_param(e, id, p)?;
    let listed = e.session().track(rack).and_then(|r| r.rack.as_ref()).is_some_and(|r| r.member(member).is_some());
    let inside = e.session().track(member).is_some_and(|t| t.folder == Some(rack));
    if !listed && !inside {
        return Err(bad(id, "that track is not in the rack"));
    }
    if let Some(r) = e.session_mut().track_mut(rack).and_then(|r| r.rack.as_mut()) {
        r.members.retain(|m| m.track != member);
        // The macro mappings to the member go with it (undo brings them back).
        for m in &mut r.macros {
            m.mappings.retain(|x| x.track != member);
        }
    }
    if inside {
        super::track::remove_from_folder(e, &json!({"tracks": [member.0]}))?;
    }
    Ok(json!({"rack": rack, "member": member}))
}

/// `base` with the fields `v` gives (`low`, `high`, `fade_low`, `fade_high`, each 0..127), normalized.
fn zone_from(id: &str, base: Zone, v: &Value) -> Result<Zone> {
    if !v.is_object() {
        return Err(bad(id, "a zone is an object {low?, high?, fade_low?, fade_high?}"));
    }
    let field = |k: &str, d: u8| -> Result<u8> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(x) => x.as_u64().filter(|n| *n <= 127).map(|n| n as u8).ok_or_else(|| bad(id, format!("zone `{k}` must be 0..127"))),
        }
    };
    let mut z = Zone { low: field("low", base.low)?, high: field("high", base.high)?, fade_low: base.fade_low, fade_high: base.fade_high };
    // Moving an edge past a fade handle takes the handle along (Ableton's zone editor).
    z.fade_low = field("fade_low", z.fade_low.max(z.low))?;
    z.fade_high = field("fade_high", z.fade_high.min(z.high))?;
    Ok(z.normalized())
}

fn zone(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.zone";
    let rack = rack_param(e, id, p)?;
    let member = member_param(e, id, p)?;
    let s = e.session();
    let m = s.track(rack).and_then(|r| r.rack.as_ref()).and_then(|r| r.member(member)).ok_or_else(|| bad(id, "that track is not a member"))?;
    let key = p.get("key").map(|v| zone_from(id, m.key_zone, v)).transpose()?.unwrap_or(m.key_zone);
    let vel = p.get("velocity").map(|v| zone_from(id, m.vel_zone, v)).transpose()?.unwrap_or(m.vel_zone);
    if key == m.key_zone && vel == m.vel_zone {
        return Ok(json!({"changed": false}));
    }
    let m = rack_mut(e.session_mut(), id, rack)?.member_mut(member).ok_or_else(|| bad(id, "that track is not a member"))?;
    m.key_zone = key;
    m.vel_zone = vel;
    Ok(json!({"changed": true, "key": key, "velocity": vel}))
}

/// The slot `p[key]` names: an insert letter `a`..`j` (or 0..9) or `instrument`, as automation lanes do.
fn slot_param(id: &str, p: &Value) -> Result<u8> {
    let v = p.get("slot").ok_or_else(|| bad(id, "`slot` required"))?;
    if v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("instrument")) {
        return Ok(AutoParam::INSTRUMENT_SLOT);
    }
    let i = match (v.as_u64(), v.as_str()) {
        (Some(n), _) => usize::try_from(n).ok(),
        (_, Some(s)) => match s.trim().to_ascii_lowercase().as_bytes() {
            [c] if c.is_ascii_lowercase() => Some(usize::from(c - b'a')),
            _ => None,
        },
        _ => None,
    };
    i.filter(|i| *i < transit_model::INSERT_SLOTS).and_then(|i| u8::try_from(i).ok()).ok_or_else(|| bad(id, "`slot` must be a..j or instrument"))
}

fn device(t: &Track, slot: u8) -> Option<&transit_model::Insert> {
    if slot == AutoParam::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(usize::from(slot)).and_then(Option::as_ref) }
}

fn set_macro(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.macro";
    let rack = rack_param(e, id, p)?;
    let i = macro_param(id, p)?;
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`value` must be a number"))? as f32;
    let value = value.clamp(0.0, 1.0);
    let s = e.session_mut();
    let mappings = {
        let r = rack_mut(s, id, rack)?;
        let m = r.macros.get_mut(i).ok_or_else(|| bad(id, "no such macro"))?;
        m.value = value;
        m.mappings.clone()
    };
    // The mapped parameters move through the same stored values `mix.insert_param` and
    // `mix.instrument_param` write: the mix hands a changed value to the plug-in.
    let mut written = 0;
    for m in &mappings {
        let Some(ins) = s.track_mut(m.track).filter(|t| t.folder == Some(rack)).and_then(|t| {
            if m.slot == AutoParam::INSTRUMENT_SLOT {
                t.instrument.as_mut()
            } else {
                t.mixer.inserts.get_mut(usize::from(m.slot)).and_then(Option::as_mut)
            }
        }) else {
            continue;
        };
        ins.params.insert(m.param.clone(), m.plain(value));
        written += 1;
    }
    Ok(json!({"macro": i + 1, "value": value, "written": written}))
}

fn unit_steps(u: transit_dsp::Unit) -> bool {
    matches!(u, transit_dsp::Unit::Toggle | transit_dsp::Unit::Choice | transit_dsp::Unit::Semitones | transit_dsp::Unit::Cents)
}

fn macro_map(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.macro_map";
    let rack = rack_param(e, id, p)?;
    let i = macro_param(id, p)?;
    let member = member_param(e, id, p)?;
    let slot = slot_param(id, p)?;
    let param = str_param(p, "param").filter(|x| !x.is_empty() && x.len() <= 512).ok_or_else(|| bad(id, "`param` required"))?.to_string();
    let num = |k: &str, d: f32, lo: f32, hi: f32| -> Result<f32> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v
                .as_f64()
                .map(|x| x as f32)
                .filter(|x| x.is_finite() && (lo..=hi).contains(x))
                .ok_or_else(|| bad(id, format!("`{k}` must be {lo}..{hi}"))),
        }
    };
    let (min, max, curve) = (num("min", 0.0, 0.0, 1.0)?, num("max", 1.0, 0.0, 1.0)?, num("curve", 0.0, -1.0, 1.0)?);
    let s = e.session();
    let t = s.track(member).filter(|t| t.folder == Some(rack)).ok_or_else(|| bad(id, "`member` is not in the rack"))?;
    let ins = device(t, slot).ok_or_else(|| bad(id, "that slot has no plug-in"))?;
    let info = super::clap::require_info(id, &ins.plugin)?;
    let pi = info.param(&param).ok_or_else(|| bad(id, format!("`{}` has no parameter `{param}`", info.id)))?;
    let range = ParamRange { lo: pi.min, hi: pi.max, log: matches!(pi.taper, transit_dsp::Taper::Log) && pi.min > 0.0, step: unit_steps(pi.unit) };
    let mapping = MacroMapping { track: member, slot, param: param.clone(), min, max, curve, range };
    let s = e.session_mut();
    let value = s.track(rack).and_then(|r| r.rack.as_ref()).and_then(|r| r.macros.get(i)).map_or(0.0, |m| m.value);
    // The parameter takes the macro's value at once, and is a stored value the mix applies from
    // now on (a hosted plug-in's map holds only parameters that were set).
    if let Some(ins) = s.track_mut(member).and_then(|t| {
        if slot == AutoParam::INSTRUMENT_SLOT { t.instrument.as_mut() } else { t.mixer.inserts.get_mut(usize::from(slot)).and_then(Option::as_mut) }
    }) {
        ins.params.insert(param.clone(), pi.clamp(mapping.plain(value)));
    }
    let r = rack_mut(s, id, rack)?;
    // A parameter follows one macro (Ableton's rule): mapping it again moves it.
    for m in &mut r.macros {
        m.mappings.retain(|x| !(x.track == member && x.slot == slot && x.param == param));
    }
    r.macros.get_mut(i).ok_or_else(|| bad(id, "no such macro"))?.mappings.push(mapping);
    Ok(json!({"macro": i + 1, "member": member, "param": param}))
}

fn macro_unmap(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.macro_unmap";
    let rack = rack_param(e, id, p)?;
    let i = macro_param(id, p)?;
    let member = member_param(e, id, p)?;
    let slot = slot_param(id, p)?;
    let param = str_param(p, "param").ok_or_else(|| bad(id, "`param` required"))?.to_string();
    let had = e
        .session()
        .track(rack)
        .and_then(|r| r.rack.as_ref())
        .and_then(|r| r.macros.get(i))
        .is_some_and(|m| m.mappings.iter().any(|x| x.track == member && x.slot == slot && x.param == param));
    if !had {
        return Err(bad(id, "that parameter is not mapped to this macro"));
    }
    let m = rack_mut(e.session_mut(), id, rack)?.macros.get_mut(i).ok_or_else(|| bad(id, "no such macro"))?;
    m.mappings.retain(|x| !(x.track == member && x.slot == slot && x.param == param));
    Ok(json!({"macro": i + 1, "removed": true}))
}

fn macro_name(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.macro_name";
    let rack = rack_param(e, id, p)?;
    let i = macro_param(id, p)?;
    let mut name = str_param(p, "name").ok_or_else(|| bad(id, "`name` required"))?.trim().to_string();
    transit_model::truncate_chars(&mut name, 64);
    let m = rack_mut(e.session_mut(), id, rack)?.macros.get_mut(i).ok_or_else(|| bad(id, "no such macro"))?;
    m.name = name;
    Ok(json!({"macro": i + 1, "name": m.name}))
}

fn info(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "rack.info";
    let rack = rack_param(e, id, p)?;
    let s = e.session();
    let t = s.track(rack).ok_or_else(|| bad(id, "no rack"))?;
    let r = t.rack.as_ref().ok_or_else(|| bad(id, "`track` is not a rack"))?;
    let playing: Vec<TrackId> = s.rack_members(t).map(|(m, _)| m.id).collect();
    let members: Vec<Value> =
        r.members.iter().map(|m| json!({"track": m.track, "plays": playing.contains(&m.track), "key": m.key_zone, "velocity": m.vel_zone})).collect();
    let macros: Vec<Value> = (0..RACK_MACROS)
        .filter_map(|i| r.macros.get(i).map(|m| (i, m)))
        .map(|(i, m)| json!({"macro": i + 1, "name": r.macro_name(i), "value": m.value, "mappings": m.mappings, "midi": m.midi}))
        .collect();
    Ok(json!({"rack": rack, "name": t.name, "members": members, "macros": macros}))
}

//! The Sampler's Instrument view commands (ADR 0041, its amendment 2026-11-08, items 1, 2, 3 and
//! 11, and amendment 2026-11-09, refusal (c); #270): a key-map group's round-robin mode, the
//! batched key-map edit, and the set-time rules every zone edit makes.
//!
//! - **Checked when set, clamped when used.** A zone edit refuses an over-limit value it sets: a
//!   crossfade over half the effective loop (named in milliseconds at the zone source's native
//!   rate and in frames), a crossfade on a ping-pong loop, fade widths wider than the range or on a
//!   pad's zone, a loop on a one-shot (and One-shot on a looping zone) unless the same edit sets
//!   Gate (or the loop Off), a loop on a one-frame slice. A later edit of something else is never
//!   refused because of a stored value: playback clamps it and `mix.sampler_state` reports both.
//! - **Slice-bound zones** loop over their whole slice: an edit that sets the loop mode writes the
//!   slice's derived source, region and loop into the stored zone in the same step, and an edit of
//!   their region or loop points is refused (loop points inside a slice come later).
//! - **An unbound zone's loop** must stay inside its region (the body's rule, which stands): a trim
//!   that would leave it outside is refused, never moved (the engine does not guess where the user
//!   wants the loop); the edit can move the loop in the same step.
//! - **Groups.** Whole-map admission before anything is stored: the group shape
//!   ([`PatchV1::check_groups`]) and, in [`super::sampler_patch::store`], the eight voices per note
//!   ([`PatchV1::layer_overload`]). A pad's zone joins a group only when the same edit unbinds the
//!   pad (`pad: null`), one undo step; changing views never unbinds.
//! - **Texts** name zones 1 to 256, groups, layers and alternatives from 0, slices from 1 in their
//!   set, and notes through the shared namer in the engine's message Middle C (ADR 0051); every
//!   parameter and result is a number.

use super::sampler::instrument_track;
use super::sampler_patch::{patch_of, store};
use super::*;
use crate::cmd;
use serde_json::{Map, json};
use transit_model::Session;
use transit_model::sampler_fades::{FadeRefusal, Fades, check_fades, fades_used};
use transit_model::sampler_loops::{crossfade_max, crossfade_ms_text, crossfade_used, derived_region, effective_region, loop_used, write_region};
use transit_model::sampler_patch::{GroupEntry, LoopMode, Membership, PadEntry, PatchV1, PlayMode, RoundRobin, SliceSet, ZONES, ZoneEntry};

/// The refusal of a loop on a one-shot, and of One-shot on a looping zone (item 1).
pub(crate) const ONE_SHOT_LOOP: &str = "A one-shot cannot loop: choose Gate, or turn the loop off.";
/// The refusal of a nonzero crossfade on a ping-pong loop (body and item 2).
pub(crate) const PING_PONG_CROSSFADE: &str = "A ping-pong loop has no crossfade: set the crossfade to 0 in the same edit.";
/// The refusal of a seed out of range (item 11).
pub(crate) const SEED_RANGE: &str = "seed must be an integer from 0 to 4294967295 (0 is stored as 1)";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.sampler_group_set",
            "Set Sampler Group",
            [],
            None,
            "{track?, group: 0-255, round_robin: sequential|random}: the key-map group's round-robin mode (sequential is the default and is not stored) → {track, group, round_robin}",
            has_selection,
            group_set
        ),
        cmd!(
            "mix.sampler_zone_map",
            "Map Sampler Zones",
            [],
            None,
            "{track?, zones: [{zone: 1-256, keys: [lo, hi], velocities: [lo, hi], root, membership: {group: 0-255, layer: 0-7, alternative: 0-7} | null, fades?: {key_lo, key_hi, vel_lo, vel_hi}, pad?: null (unbind the zone's pad)}]}: the key map's range edits as one step, refused whole (group shape, at most eight voices per note) → {track, zones: [the zone states]}",
            has_selection,
            zone_map
        ),
    ]
}

/// A seed parameter as stored: an integer 0 to 4294967295, 0 stored as 1 (item 11).
pub(crate) fn seed_param(id: &str, v: &Value) -> Result<u32> {
    v.as_u64().and_then(|x| u32::try_from(x).ok()).map(|x| x.max(1)).ok_or_else(|| bad(id, SEED_RANGE))
}

/// What a zone edit does to the zone's pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Binding {
    /// The pad binding stays as it is.
    Keep,
    /// The same edit unbinds the zone's pad (`pad: null`).
    Unbind,
    /// The same edit binds a pad to the zone (`mix.sampler_zone_source` with `pad`).
    Bind,
}

/// The keys of a zone edit's `region` and `playback` objects: the values the edit sets.
pub(crate) fn given_keys(p: &Value) -> Vec<String> {
    ["region", "playback"].iter().filter_map(|g| p.get(*g).and_then(Value::as_object)).flat_map(|o| o.keys().cloned()).collect()
}

/// `fades` as given: exactly the four widths, integers 0 to 127.
fn fades_value(id: &str, v: &Value) -> Result<Fades> {
    let o = v.as_object().ok_or_else(|| bad(id, "`fades` must be {key_lo, key_hi, vel_lo, vel_hi}"))?;
    if let Some(k) = o.keys().find(|k| !["key_lo", "key_hi", "vel_lo", "vel_hi"].contains(&k.as_str())) {
        return Err(bad(id, format!("`fades` has no `{k}` (it takes key_lo, key_hi, vel_lo, vel_hi)")));
    }
    let width = |k: &str| -> Result<u8> {
        o.get(k)
            .and_then(Value::as_u64)
            .and_then(|x| u8::try_from(x).ok())
            .filter(|x| *x <= 127)
            .ok_or_else(|| bad(id, FadeRefusal::Range.to_string()))
    };
    Ok(Fades::of([width("key_lo")?, width("key_hi")?, width("vel_lo")?, width("vel_hi")?]))
}

/// `membership` as given: null, or exactly a group, a layer and an alternative, integers 0 to 255
/// (the group shape names a layer or alternative past 8).
fn membership_value(id: &str, v: &Value) -> Result<Option<Membership>> {
    if v.is_null() {
        return Ok(None);
    }
    let shape = "`membership` must be null or {group: 0-255, layer: 0-7, alternative: 0-7}";
    let o = v.as_object().ok_or_else(|| bad(id, shape))?;
    if o.keys().any(|k| !["group", "layer", "alternative"].contains(&k.as_str())) {
        return Err(bad(id, shape));
    }
    let n = |k: &str| o.get(k).and_then(Value::as_u64).and_then(|x| u8::try_from(x).ok()).ok_or_else(|| bad(id, shape));
    Ok(Some(Membership { group: n("group")?, layer: n("layer")?, alternative: n("alternative")?, ..Membership::default() }))
}

/// Checks the shape of the new zone keys (`membership`, `fades`, `crossfade`) in a zone edit's
/// objects, before they are merged into the zone.
pub(crate) fn check_new_keys(id: &str, given: &Map<String, Value>) -> Result<()> {
    if let Some(v) = given.get("membership") {
        membership_value(id, v)?;
    }
    if let Some(v) = given.get("fades") {
        fades_value(id, v)?;
    }
    if given.get("crossfade").is_some_and(|v| v.as_u64().is_none()) {
        return Err(bad(id, "`crossfade` must be a whole number of source frames, 0 or more"));
    }
    Ok(())
}

/// The admitted rate of the source, using stored metadata when its audio is missing.
fn source_rate(s: &Session, p: &PatchV1, z: &ZoneEntry) -> u32 {
    let r = effective_region(z, p);
    p.sources
        .get(usize::from(r.source))
        .and_then(|x| s.pool.get(x.source).map(|a| a.buffer.sample_rate).or_else(|| s.source(x.source).map(|a| a.sample_rate)))
        .unwrap_or(s.sample_rate.hz())
}

/// The 1-based number of slice-bound zone `z`'s slice in its set.
fn slice_number(p: &PatchV1, z: &ZoneEntry) -> usize {
    let (Some(b), Some(l)) = (&z.slice, &p.slicing) else { return 0 };
    let set = match b.set {
        SliceSet::Auto => l.auto.as_ref(),
        SliceSet::Manual => l.manual.as_ref(),
    };
    set.and_then(|m| m.markers.iter().position(|x| x.id == b.id)).map_or(0, |i| i + 1)
}

/// Puts `zone` into `p` in place of the zone with its id, keeping the zones in id order.
fn put_zone(p: &mut PatchV1, zone: ZoneEntry) {
    p.zones.retain(|z| z.id != zone.id);
    p.zones.push(zone);
    p.zones.sort_by_key(|z| z.id);
}

/// Admits one zone edit into `p` (the patch being built): `before` is the zone as stored (`None`:
/// a new zone), `next` the zone with the edit's values, `given` the keys the edit sets and
/// `binding` what it does to the pad. Makes the set-time checks, writes a slice-bound zone's
/// derived values through when the edit sets its loop, applies `pad: null` and puts the zone into
/// `p`. The group shape and the voices per note are checked by the caller over the whole map.
pub(crate) fn admit_zone(
    s: &Session,
    id: &str,
    p: &mut PatchV1,
    before: Option<&ZoneEntry>,
    mut next: ZoneEntry,
    given: &[String],
    binding: Binding,
) -> Result<()> {
    let sets = |k: &str| given.iter().any(|g| g == k);
    let n = next.id + 1;
    // Unknown fields stay with an unchanged membership or fades.
    if let Some(b) = before {
        let triple = |m: &Option<Membership>| m.as_ref().map(|m| (m.group, m.layer, m.alternative));
        if triple(&b.membership) == triple(&next.membership) {
            next.membership.clone_from(&b.membership);
        }
        if b.fades.widths() == next.fades.widths() {
            next.fades.clone_from(&b.fades);
        }
    }
    let pad = p.pad_of(next.id);
    let pad_bound = match binding {
        Binding::Keep => pad.is_some(),
        Binding::Unbind => false,
        Binding::Bind => true,
    };
    if sets("membership")
        && next.membership.is_some()
        && binding == Binding::Keep
        && let Some(pad) = pad
    {
        let label = transit_model::pad_notes::pad_label(pad);
        return Err(bad(id, format!("Zone {n} is pad {label}'s zone: unbind the pad in the same edit (pad: null) to make it a group member.")));
    }
    if next.slice.is_some() {
        if let Some(k) = ["start", "end", "loop_start", "loop_end"].iter().find(|k| sets(k)) {
            return Err(bad(
                id,
                format!("Zone {n} plays slice {}: its region and loop follow the slice (`{k}` cannot be set).", slice_number(p, &next)),
            ));
        }
        if sets("loop_mode")
            && let Some(d) = derived_region(&next, p)
        {
            if next.loop_mode != LoopMode::Off && d.loop_len() < 2 {
                return Err(bad(id, format!("Slice {} is one frame long, too short to loop.", slice_number(p, &next))));
            }
            // Write-through (item 1): the stored zone describes what plays.
            write_region(&mut next, d);
        }
    }
    if (sets("mode") || sets("loop_mode")) && next.mode == PlayMode::OneShot && next.loop_mode != LoopMode::Off {
        return Err(bad(id, ONE_SHOT_LOOP));
    }
    if (sets("loop_mode") || sets("crossfade")) && next.loop_mode == LoopMode::PingPong && next.crossfade > 0 {
        return Err(bad(id, PING_PONG_CROSSFADE));
    }
    if sets("crossfade") {
        let max = crossfade_max(&next, p);
        if next.crossfade > max {
            let ms = crossfade_ms_text(max, source_rate(s, p, &next));
            return Err(bad(id, format!("Crossfade is at most {ms} ms ({max} frames) for this loop.")));
        }
    }
    let inside =
        next.start <= next.loop_start && next.loop_start < next.loop_end && next.loop_end <= next.end && next.loop_end - next.loop_start >= 2;
    if next.slice.is_none() && next.loop_mode != LoopMode::Off && !inside {
        return Err(bad(
            id,
            format!(
                "Zone {n}'s loop (frames {} to {}) must lie inside its region (frames {} to {}) and be at least two frames long: move the loop in the same edit, or turn it off.",
                next.loop_start, next.loop_end, next.start, next.end
            ),
        ));
    }
    if sets("fades") {
        check_fades(next.keys, next.velocities, &next.fades, pad_bound).map_err(|r| bad(id, r.to_string()))?;
    }
    if binding == Binding::Unbind
        && let Some(pad) = pad
    {
        p.set_pad(PadEntry { zone: None, ..p.pad(pad) });
    }
    put_zone(p, next);
    Ok(())
}

/// Zone `z` of `p` as `mix.sampler_state` and the zone edits report it: its pad and membership, the
/// stored loop mode, crossfade and fades beside what playback uses, its slice and (when
/// slice-bound) the region the slice derives, and its source's native rate.
pub(crate) fn zone_json(s: &Session, p: &PatchV1, z: &ZoneEntry) -> Value {
    let w = |f: [u8; 4]| json!({"key_lo": f[0], "key_hi": f[1], "vel_lo": f[2], "vel_hi": f[3]});
    let bound = p.pad_of(z.id);
    let region =
        derived_region(z, p).map(|d| json!({"source": d.source, "start": d.start, "end": d.end, "loop_start": d.loop_start, "loop_end": d.loop_end}));
    json!({
        "zone": z.id + 1, "pad": bound.map(|x| x + 1),
        "keys": z.keys, "velocities": z.velocities, "root": z.root, "mode": z.mode,
        "membership": z.membership.as_ref().map(|m| json!({"group": m.group, "layer": m.layer, "alternative": m.alternative})),
        "loop_mode": z.loop_mode, "loop_used": loop_used(z, p),
        "crossfade": z.crossfade, "crossfade_used": crossfade_used(z, p),
        "fades": w(z.fades.widths()), "fades_used": w(fades_used(z, bound.is_some())),
        "slice": z.slice.as_ref().map(|b| json!({"set": b.set, "id": b.id, "slice": slice_number(p, z)})),
        "region": region, "rate": source_rate(s, p, z),
    })
}

/// Every group that has a member or a stored mode: its id, mode and member zones.
pub(crate) fn groups_json(p: &PatchV1) -> Value {
    let mut ids: Vec<u8> = p.zones.iter().filter_map(|z| z.membership.as_ref().map(|m| m.group)).chain(p.groups.iter().map(|g| g.id)).collect();
    ids.sort_unstable();
    ids.dedup();
    let groups = ids.into_iter().map(|g| {
        let mode = p.groups.iter().find(|e| e.id == g).map_or(RoundRobin::Sequential, |e| e.round_robin);
        let zones: Vec<u16> = p.zones.iter().filter(|z| z.membership.as_ref().is_some_and(|m| m.group == g)).map(|z| z.id + 1).collect();
        json!({"group": g, "round_robin": mode, "zones": zones})
    });
    Value::Array(groups.collect())
}

fn group_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_group_set";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let group = p
        .get("group")
        .and_then(Value::as_u64)
        .and_then(|g| u8::try_from(g).ok())
        .ok_or_else(|| bad(id, "`group` must be an integer from 0 to 255"))?;
    let mode = match str_param(p, "round_robin") {
        Some("sequential") => RoundRobin::Sequential,
        Some("random") => RoundRobin::Random,
        _ => return Err(bad(id, "`round_robin` must be sequential or random")),
    };
    let mut entry = patch.groups.iter().find(|g| g.id == group).cloned().unwrap_or(GroupEntry { id: group, ..GroupEntry::default() });
    let changed = entry.round_robin != mode;
    entry.round_robin = mode;
    patch.groups.retain(|g| g.id != group);
    // Sequential is the default: an entry that says nothing else is not stored.
    if mode != RoundRobin::Sequential || !entry.extra.is_empty() {
        patch.groups.push(entry);
        patch.groups.sort_by_key(|g| g.id);
    }
    if changed {
        store(e, id, t, patch)?;
    }
    Ok(json!({"track": t, "group": group, "round_robin": mode}))
}

/// A `[lo, hi]` pair of integers in `lo_min..=127`, `lo <= hi`.
fn range_param(id: &str, v: Option<&Value>, what: &str, lo_min: u8) -> Result<[u8; 2]> {
    let text = || bad(id, format!("`{what}` must be [lo, hi] with {lo_min} <= lo <= hi <= 127"));
    let a = v.and_then(Value::as_array).filter(|a| a.len() == 2).ok_or_else(text)?;
    let n = |x: &Value| x.as_u64().and_then(|x| u8::try_from(x).ok()).filter(|x| (lo_min..=127).contains(x));
    match (a.first().and_then(n), a.get(1).and_then(n)) {
        (Some(lo), Some(hi)) if lo <= hi => Ok([lo, hi]),
        _ => Err(text()),
    }
}

/// An English ordinal (`257th`).
fn ordinal(n: u64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

const MAP_KEYS: [&str; 7] = ["zone", "keys", "velocities", "root", "membership", "fades", "pad"];

fn zone_map(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_zone_map";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let list = p.get("zones").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or_else(|| bad(id, "`zones` must be a list of zone edits"))?;
    if list.len() > ZONES {
        return Err(bad(id, format!("`zones` lists {} edits (a Sampler holds at most {ZONES} zones)", list.len())));
    }
    let mut seen: Vec<u16> = Vec::with_capacity(list.len());
    for item in list {
        let o = item.as_object().ok_or_else(|| bad(id, "each zone edit must be an object"))?;
        if let Some(k) = o.keys().find(|k| !MAP_KEYS.contains(&k.as_str())) {
            return Err(bad(id, format!("a zone edit has no `{k}` (it takes {})", MAP_KEYS.join(", "))));
        }
        let n = o.get("zone").and_then(Value::as_u64).filter(|n| *n >= 1).ok_or_else(|| bad(id, "`zone` must be 1-256"))?;
        if n > ZONES as u64 {
            return Err(bad(id, format!("zone {n} would be the {} zone (at most {ZONES})", ordinal(n))));
        }
        let zone = (n - 1) as u16;
        if seen.contains(&zone) {
            return Err(bad(id, format!("zone {n} appears twice in the map")));
        }
        seen.push(zone);
        let before = patch.zone(zone).cloned().ok_or_else(|| bad(id, format!("zone {n} is empty")))?;
        let mut next = before.clone();
        next.keys = range_param(id, o.get("keys"), "keys", 0)?;
        next.velocities = range_param(id, o.get("velocities"), "velocities", 1)?;
        next.root = o
            .get("root")
            .and_then(Value::as_u64)
            .and_then(|r| u8::try_from(r).ok())
            .filter(|r| *r <= 127)
            .ok_or_else(|| bad(id, "`root` must be 0-127"))?;
        next.membership = membership_value(id, o.get("membership").ok_or_else(|| bad(id, "`membership` is required (null: no group)"))?)?;
        let mut given: Vec<String> = ["keys", "velocities", "root", "membership"].map(String::from).to_vec();
        if let Some(f) = o.get("fades") {
            next.fades = fades_value(id, f)?;
            given.push("fades".into());
        }
        let binding = match o.get("pad") {
            None => Binding::Keep,
            Some(Value::Null) => Binding::Unbind,
            Some(_) => return Err(bad(id, "`pad` takes only null (unbind the zone's pad in the same edit)")),
        };
        admit_zone(e.session(), id, &mut patch, Some(&before), next, &given, binding)?;
    }
    patch.check_groups().map_err(|r| bad(id, r.to_string()))?;
    store(e, id, t, patch)?;
    let stored = patch_of(e.session(), id, t)?;
    let s = e.session();
    let zones: Vec<Value> = seen.iter().filter_map(|z| stored.zone(*z)).map(|z| zone_json(s, &stored, z)).collect();
    Ok(json!({"track": t, "zones": zones}))
}

#[cfg(test)]
#[path = "tests_sampler_groups.rs"]
mod tests;

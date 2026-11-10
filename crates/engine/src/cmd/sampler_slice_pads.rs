//! Slices to pads and Map slices to keys (ADR 0041 "Slice sources and Slice to MIDI", its
//! amendment 2026-11-08 item 6 and its amendment 2026-11-09 item 7, #269): the marker set showing
//! becomes playable pads, slice i on trigger note root+i, and a mapping that went out of order is
//! repaired. The ascending-run check ([`mapping_status`]) is shared with `mix.sampler_slice_state`
//! and Slice to MIDI.
//!
//! - **The showing set only.** Both commands read the set of the mode showing, never the hidden
//!   set and never an undelivered detection candidate (the candidate is not in the patch).
//! - **Pad assignment** (item 6, pads in bank order A1 to A16, then B1): pads bound to the showing
//!   set are free for the run; the pad holding note root+i takes slice i when it is empty or
//!   slice-bound (either set) and refuses the run when it is a loose occupied pad; the remaining
//!   slices take the lowest free pads, whose notes become root+i; showing-set pads left over are
//!   unbound (their zones empty, their notes stay). A taken empty pad gets a new zone with the
//!   new-pad defaults over its slice; a taken occupied pad keeps its zone and settings.
//! - **Refused whole.** Every refusal comes before any change and nothing is truncated; `preview`
//!   returns the same plan and the same refusal as data. An accepted run is one undo step.
//! - **Write-through.** A run calls [`write_through_slice_zones`] for the showing set inside its
//!   step, so a rebound zone's stored region follows its new slice once that hook is filled.

use super::sampler::instrument_track;
use super::sampler_patch::{patch_of, store};
use super::sampler_slices::{SliceView, loop_of, set_of, slices, write_through_slice_zones};
use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::sampler_patch::{PADS, PadEntry, PatchV1, SliceBinding, SliceLoop, SliceMode, SliceSet, ZONES, ZoneEntry};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.sampler_slices_to_pads",
            "Slices to Pads",
            [],
            None,
            "{track?, root?: 0-(128 minus the slice count) (default: the loop's root), preview?: false}: the showing set's slice i plays on trigger note root+i, one pad-bound zone per slice (ADR 0041 amendment 2026-11-08 item 6); the loop stores root; preview returns the plan and edits nothing → {track, set, root, count, cells: [{pad, note, slice, id, replaces: null|{set, slice, id}}], loose_collisions: [{pad, note}], unbound: [{pad, note, slice, id, zone}], pads_after, ok, reason?}",
            has_selection,
            slices_to_pads
        ),
        cmd!(
            "mix.sampler_map_slices",
            "Map Slices to Keys",
            [],
            None,
            "{track?, root: 0-(128 minus the slice count)}: re-notes only the showing set's slice pads so slice i plays root+i; an empty pad on one of the run's notes takes a note the run vacates, a sounding one refuses; every other pad is untouched; the loop stores root → {track, set, root, count, pads: [{pad, note, slice, id}], moved: [{pad, from, to}]}",
            has_selection,
            map_slices
        ),
    ]
}

/// A pad's name as the panel numbers it: bank letter and slot (A1 is pad 0, B1 pad 16).
pub(crate) fn pad_name(index: u8) -> String {
    format!("{}{}", char::from(b'A' + (index / 16).min(25)), index % 16 + 1)
}

/// A note named for message text, in the engine's message convention (ADR 0051 decision 5: the
/// machine's Middle C setting, `Engine::message_middle_c`). Parameters and result fields stay MIDI
/// numbers; only the words a person reads follow the setting.
fn note_text(note: u8, convention: transit_midi::MiddleC) -> String {
    transit_midi::note_name(note, convention)
}

/// Pad `index`'s effective trigger note: its stored note, else its default. The pad start note
/// (ADR 0041 amendment 2026-11-09) changes the default inside `PadEntry::trigger`.
fn pad_note(p: &PatchV1, index: u8) -> u8 {
    p.pad(index).trigger()
}

/// The root a run uses when the caller gives none: the loop's root. The loop stores one today
/// (a new loop takes the pad start note); when a loop may hold no root (ADR 0041 amendment
/// 2026-11-09 item 7, `root_stored`), an absent root reads the patch's pad start note here.
fn default_run_root(l: &SliceLoop) -> u8 {
    l.root
}

/// The set the mode showing reads.
pub(crate) fn showing(l: &SliceLoop) -> SliceSet {
    match l.mode {
        SliceMode::Auto => SliceSet::Auto,
        SliceMode::Manual => SliceSet::Manual,
    }
}

fn set_name(s: SliceSet) -> &'static str {
    match s {
        SliceSet::Auto => "auto",
        SliceSet::Manual => "manual",
    }
}

/// One slice of a stale mapping: its 1-based position, id, 0-based pad and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleSlice {
    pub slice: usize,
    pub id: Option<u32>,
    pub pad: Option<u8>,
    pub key: Option<u8>,
}

/// The pad mapping of a marker set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MappingStatus {
    /// Every slice has a pad and slice i plays `root + i`.
    Ascending { root: u8 },
    /// Some slice has a pad, and the run breaks between these two neighboring slices (the first
    /// pair out of order).
    Stale { first_two: [StaleSlice; 2] },
    /// No slice has a pad.
    None,
}

/// The ascending-run check on slices in order (each with its pad and key, as
/// `mix.sampler_slice_state` reports them).
pub(super) fn mapping_of(v: &[SliceView]) -> MappingStatus {
    if v.iter().all(|s| s.key.is_none()) {
        return MappingStatus::None;
    }
    let ascending = |a: Option<u8>, b: Option<u8>| matches!((a, b), (Some(a), Some(b)) if u16::from(b) == u16::from(a) + 1);
    let view = |n: usize, s: &SliceView| StaleSlice { slice: n + 1, id: s.id, pad: s.pad, key: s.key };
    match (1..v.len()).find(|i| !ascending(v[i - 1].key, v[*i].key)) {
        Some(i) => MappingStatus::Stale { first_two: [view(i - 1, &v[i - 1]), view(i, &v[i])] },
        // Every neighbor pair ascends and some key exists, so the first slice has one.
        None => v.first().and_then(|s| s.key).map_or(MappingStatus::None, |root| MappingStatus::Ascending { root }),
    }
}

/// The pad mapping of `set` in patch `p`.
pub(crate) fn mapping_status(p: &PatchV1, set: SliceSet) -> MappingStatus {
    mapping_of(&slices(p, set))
}

/// What a pad holds before a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Empty,
    /// A zone bound to slice `id` of a set.
    Slice(SliceSet, u32),
    /// A zone without a slice binding.
    Loose,
}

fn held(p: &PatchV1, index: u8) -> Held {
    match p.pad(index).zone.and_then(|z| p.zone(z)) {
        None => Held::Empty,
        Some(z) => z.slice.as_ref().map_or(Held::Loose, |b| Held::Slice(b.set, b.id)),
    }
}

/// One slice's place in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    pad: u8,
    note: u8,
    /// 0-based position in the set.
    slice: usize,
    id: u32,
    /// The pad already held the note (step 2); otherwise its note becomes `note` (step 3).
    on_note: bool,
    was: Held,
}

/// A run's plan: the cells, what refuses it, and what it unbinds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    set: SliceSet,
    root: u8,
    count: usize,
    cells: Vec<Cell>,
    loose: Vec<(u8, u8)>,
    /// Showing-set pads the run does not take: (pad, its slice id).
    unbound: Vec<(u8, u32)>,
    pads_after: usize,
    /// A refusal other than a loose collision (whose text [`Plan::reason`] names in the message
    /// convention, so the plan itself holds no message state).
    refusal: Option<String>,
}

impl Plan {
    /// Why the run is refused, in message text: the first loose collision, else the other refusal.
    fn reason(&self, names: transit_midi::MiddleC) -> Option<String> {
        match self.loose.first() {
            Some(&(q, note)) => Some(format!("Pad {} already plays {}", pad_name(q), note_text(note, names))),
            None => self.refusal.clone(),
        }
    }
}

fn too_high(root: u8, n: usize) -> Option<String> {
    (usize::from(root) + n > 128).then(|| format!("Root {root} is too high for {n} slices: the highest root is {}.", 128usize.saturating_sub(n)))
}

/// The pads of `p` in bank order with what each holds and its effective note.
fn pads(p: &PatchV1) -> Vec<(u8, Held, u8)> {
    (0..PADS as u8).map(|q| (q, held(p, q), pad_note(p, q))).collect()
}

/// Plans a run of `ids` (the showing set's slice ids in order) at `root` (ADR 0041 amendment
/// 2026-11-08, item 6). Pure: the patch is only read.
fn plan(p: &PatchV1, set: SliceSet, ids: &[u32], root: u8) -> Plan {
    let n = ids.len();
    let all = pads(p);
    let mut out = Plan { set, root, count: n, cells: Vec::new(), loose: Vec::new(), unbound: Vec::new(), pads_after: 0, refusal: None };
    if let Some(text) = too_high(root, n) {
        out.pads_after = all.iter().filter(|(_, h, _)| *h != Held::Empty).count();
        out.refusal = Some(text);
        return out;
    }
    let is_free = |h: &Held| matches!(h, Held::Empty) || matches!(h, Held::Slice(s, _) if *s == set);
    let mut taken = [false; PADS];
    let mut pending = Vec::new();
    // Step 2: the pad already on note root+i takes slice i, unless it is a loose occupied pad.
    for (i, id) in ids.iter().enumerate() {
        let note = root + i as u8;
        match all.iter().find(|(_, _, k)| *k == note) {
            Some(&(q, Held::Loose, _)) => out.loose.push((q, note)),
            Some(&(q, was, _)) => {
                taken[usize::from(q)] = true;
                out.cells.push(Cell { pad: q, note, slice: i, id: *id, on_note: true, was });
            }
            None => pending.push((i, *id, note)),
        }
    }
    let free_total = all.iter().filter(|(_, h, _)| is_free(h)).count();
    let hidden_on_note = out.cells.iter().filter(|c| !is_free(&c.was)).count();
    let available = free_total + hidden_on_note;
    // Step 3: the rest, ascending, take the lowest free pads not taken in step 2.
    let free: Vec<(u8, Held)> = all.iter().filter(|(q, h, _)| is_free(h) && !taken[usize::from(*q)]).map(|(q, h, _)| (*q, *h)).collect();
    for ((i, id, note), &(q, was)) in pending.into_iter().zip(&free) {
        taken[usize::from(q)] = true;
        out.cells.push(Cell { pad: q, note, slice: i, id, on_note: false, was });
    }
    out.cells.sort_by_key(|c| c.slice);
    // Step 4: showing-set pads the run does not take are unbound.
    out.unbound = all
        .iter()
        .filter_map(|(q, h, _)| match h {
            Held::Slice(s, id) if *s == set && !taken[usize::from(*q)] => Some((*q, *id)),
            _ => None,
        })
        .collect();
    // Occupied after the run: its n pads, the loose pads, and the hidden-set pads it does not take
    // (a hidden-set pad step 2 takes is one of the n).
    let kept =
        all.iter().filter(|(q, h, _)| matches!(h, Held::Loose) || (matches!(h, Held::Slice(s, _) if *s != set) && !taken[usize::from(*q)])).count();
    out.pads_after = n + kept;
    if !out.loose.is_empty() {
        // Refused by the first loose collision; `Plan::reason` words it.
        return out;
    }
    if available < n {
        out.refusal = Some(format!("Slices to pads needs {n} pads; {available} are free."));
    } else {
        let new_zones = out.cells.iter().filter(|c| c.was == Held::Empty).count();
        let empty_zones = ZONES.saturating_sub(p.zones.len()) + out.unbound.len();
        if empty_zones < new_zones {
            out.refusal = Some(format!("Slices to pads needs {new_zones} empty zones; {empty_zones} are empty."));
        }
    }
    out
}

/// The 1-based position of slice `id` in `set` of `p`'s loop.
fn position(p: &PatchV1, set: SliceSet, id: u32) -> Option<usize> {
    p.slicing.as_ref().and_then(|l| set_of(l, set)).and_then(|m| m.markers.iter().position(|x| x.id == id)).map(|i| i + 1)
}

fn plan_json(p: &PatchV1, plan: &Plan, names: transit_midi::MiddleC) -> Value {
    let cells: Vec<Value> = plan
        .cells
        .iter()
        .map(|c| {
            let replaces = match c.was {
                Held::Slice(s, id) if (s, id) != (plan.set, c.id) => json!({"set": set_name(s), "slice": position(p, s, id), "id": id}),
                _ => Value::Null,
            };
            json!({"pad": c.pad + 1, "note": c.note, "slice": c.slice + 1, "id": c.id, "replaces": replaces})
        })
        .collect();
    let loose: Vec<Value> = plan.loose.iter().map(|(q, note)| json!({"pad": q + 1, "note": note})).collect();
    let unbound: Vec<Value> = plan
        .unbound
        .iter()
        .map(|(q, id)| {
            let zone = p.pad(*q).zone.map(|z| z + 1);
            json!({"pad": q + 1, "note": pad_note(p, *q), "slice": position(p, plan.set, *id), "id": id, "zone": zone})
        })
        .collect();
    let mut out = json!({
        "set": set_name(plan.set), "root": plan.root, "count": plan.count, "cells": cells,
        "loose_collisions": loose, "unbound": unbound, "pads_after": plan.pads_after, "ok": plan.reason(names).is_none(),
    });
    if let Some(r) = plan.reason(names) {
        out["reason"] = json!(r);
    }
    out
}

/// Applies an accepted plan to `p` (the loop's source and slice bounds from `views`).
fn apply(p: &mut PatchV1, plan: &Plan, views: &[SliceView]) {
    // Step 4 first, so their zone slots are free for the new zones.
    for (q, _) in &plan.unbound {
        let pad = p.pad(*q);
        if let Some(z) = pad.zone {
            p.zones.retain(|x| x.id != z);
        }
        p.set_pad(PadEntry { zone: None, ..pad });
    }
    let source = p.slicing.as_ref().map_or(0, |l| l.source);
    for c in &plan.cells {
        let binding = SliceBinding { set: plan.set, id: c.id, ..SliceBinding::default() };
        let mut pad = p.pad(c.pad);
        let zone = match pad.zone.filter(|_| c.was != Held::Empty) {
            Some(z) => {
                // A taken occupied pad keeps its zone and settings; only the binding changes.
                if let Some(entry) = p.zone_mut(z) {
                    entry.slice = Some(binding);
                }
                z
            }
            None => {
                let z = (0..ZONES as u16).find(|z| p.zone(*z).is_none()).unwrap_or(0);
                let v = &views[c.slice];
                let mut entry = ZoneEntry {
                    id: z,
                    source,
                    start: v.frame,
                    end: v.end,
                    loop_start: v.frame,
                    loop_end: v.end,
                    slice: Some(binding),
                    ..ZoneEntry::default()
                };
                entry.pad_defaults(c.note);
                p.zones.push(entry);
                p.zones.sort_by_key(|x| x.id);
                z
            }
        };
        pad.zone = Some(zone);
        if !c.on_note {
            pad.note = Some(c.note);
        }
        p.set_pad(pad);
    }
    p.drop_unused_sources();
}

/// The showing set's slices, refused when the set has no markers yet (its one slice has no id).
fn showing_slices(id: &str, p: &PatchV1) -> Result<(SliceSet, Vec<SliceView>, Vec<u32>)> {
    let l = loop_of(id, p)?;
    let set = showing(l);
    let views = slices(p, set);
    let ids: Option<Vec<u32>> = views.iter().map(|v| v.id).collect();
    match ids {
        Some(ids) if !ids.is_empty() => Ok((set, views, ids)),
        _ => Err(bad(
            id,
            format!("The {} set has no slice markers yet: detect or add slices first.", if set == SliceSet::Auto { "Auto" } else { "Manual" }),
        )),
    }
}

fn root_param(id: &str, v: &Value) -> Result<u8> {
    v.as_u64().and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 127).ok_or_else(|| bad(id, "`root` must be an integer from 0 to 127"))
}

fn slices_to_pads(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slices_to_pads";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let preview = match p.get("preview") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(id, "`preview` must be true or false"))?,
    };
    let (set, views, ids) = showing_slices(id, &patch)?;
    let root = match p.get("root") {
        None => default_run_root(loop_of(id, &patch)?),
        Some(v) => root_param(id, v)?,
    };
    let plan = plan(&patch, set, &ids, root);
    let names = e.message_middle_c();
    let mut out = plan_json(&patch, &plan, names);
    out["track"] = json!(t);
    if preview {
        return Ok(out);
    }
    if let Some(r) = plan.reason(names) {
        return Err(bad(id, r));
    }
    let before = patch.clone();
    apply(&mut patch, &plan, &views);
    if let Some(l) = patch.slicing.as_mut() {
        l.root = root;
    }
    write_through_slice_zones(&mut patch, set);
    if patch != before {
        store(e, id, t, patch)?;
    }
    Ok(out)
}

fn map_slices(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_map_slices";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let (set, views, ids) = showing_slices(id, &patch)?;
    let root = root_param(id, p.get("root").ok_or_else(|| bad(id, "`root` required (0 to 127)"))?)?;
    let n = ids.len();
    if let Some(text) = too_high(root, n) {
        return Err(bad(id, text));
    }
    let mut run = Vec::with_capacity(n);
    for (i, v) in views.iter().enumerate() {
        let pad = v.pad.ok_or_else(|| bad(id, format!("Slice {} has no pad: use Slices to pads.", i + 1)))?;
        run.push(pad);
    }
    // A pad outside the run on one of the run's notes: a sounding one (loose or bound to the hidden
    // set) refuses; an empty one, which sounds nothing, takes a note the run vacates.
    let collision = |q: u8, note: u8| bad(id, format!("Pad {} already plays {}", pad_name(q), note_text(note, e.message_middle_c())));
    let mut empty = Vec::new();
    for i in 0..n {
        let note = root + i as u8;
        if let Some(q) = (0..PADS as u8).find(|q| !run.contains(q) && pad_note(&patch, *q) == note) {
            if held(&patch, q) != Held::Empty {
                return Err(collision(q, note));
            }
            empty.push((q, note));
        }
    }
    // Empty pads in ascending pad order take the vacated notes (the run's old notes outside the new
    // run) in ascending order. Notes are unique, so there are always enough; refused otherwise.
    empty.sort_unstable();
    let run_notes = usize::from(root)..usize::from(root) + n;
    let mut vacated: Vec<u8> = run.iter().map(|q| pad_note(&patch, *q)).filter(|k| !run_notes.contains(&usize::from(*k))).collect();
    vacated.sort_unstable();
    if let Some(&(q, note)) = empty.get(vacated.len()) {
        return Err(collision(q, note));
    }
    let before = patch.clone();
    let mut moved = Vec::with_capacity(empty.len());
    for (&(q, from), &to) in empty.iter().zip(&vacated) {
        let entry = patch.pad(q);
        patch.set_pad(PadEntry { note: Some(to), ..entry });
        moved.push(json!({"pad": q + 1, "from": from, "to": to}));
    }
    let mut pads = Vec::with_capacity(n);
    for (i, q) in run.iter().enumerate() {
        let note = root + i as u8;
        let entry = patch.pad(*q);
        if entry.trigger() != note {
            patch.set_pad(PadEntry { note: Some(note), ..entry });
        }
        pads.push(json!({"pad": q + 1, "note": note, "slice": i + 1, "id": ids[i]}));
    }
    if let Some(l) = patch.slicing.as_mut() {
        l.root = root;
    }
    if patch != before {
        store(e, id, t, patch)?;
    }
    Ok(json!({"track": t, "set": set_name(set), "root": root, "count": n, "pads": pads, "moved": moved}))
}

#[cfg(test)]
#[path = "tests_sampler_slice_pads.rs"]
mod tests;

//! The Sampler's slicing commands (ADR 0041 "Slice sources and Slice to MIDI" and its amendment
//! 2026-11-08, #269): make the patch's one slice loop, switch Auto and Manual, add, move, remove
//! and label markers, start Manual from Auto's markers, clear Manual, set the shared settings, and
//! a read-only state query. The marker logic is the model's ([`transit_model::slicing`]); these
//! commands pick the set, read the source for the snap, free bindings and build the reports.
//!
//! - **One step each.** Every command clones the patch, edits the clone and stores it whole
//!   (`store` runs the patch checks), so a refused command changes nothing and an accepted one is
//!   one undo step, including any copy from Auto and the switch to Manual.
//! - **Auto showing.** An accepted marker gesture in Auto first copies Auto into Manual (new
//!   ids, labels kept, the previous Manual set replaced and its bindings freed), switches to
//!   Manual and applies the gesture to the copy: an Auto marker id names the Manual marker at the
//!   same frame. A gesture that would be refused is refused first: no copy, no switch. A move or
//!   remove aimed at the loop start S in Auto copies and switches and leaves S where it is
//!   (`fixed: true`); in Manual it is refused.
//! - **Freed bindings.** A marker removed from a set (by a remove, a Clear or a replacement of
//!   Manual) frees the zones bound to its slice in that set only: each such zone is emptied and
//!   its pad unbound (the pad's note stays), reported as `freed_pads` and `freed_zones`.
//! - **Write-through.** Every edit that changes a set's boundaries calls
//!   [`write_through_slice_zones`] inside its step: each zone bound to the set stores its slice's
//!   derived source, region and loop, so the saved file describes what plays.
//! - **The engine never guesses UI state.** The grid's and the beat length's session tempo is
//!   read at the placement `at` the caller passes; without it they are not reported.

use super::sampler::instrument_track;
use super::sampler_groups::zone_json;
use super::sampler_patch::{NewSource, add_audio, patch_of, source_param, store};
use super::sampler_slice_pads::{MappingStatus, StaleSlice, mapping_status};
use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::Session;
use transit_model::sampler_loops::{DerivedRegion, derived_region, write_region};
use transit_model::sampler_patch::{MAX_SLICES, Markers, PadEntry, PatchV1, SliceLoop, SliceMode, SliceSet};
use transit_model::slicing::{self, ClampedAt, FULL_TEXT, MarkerError, SnapError};

/// The longest marker label, in characters (after trimming).
const MAX_MARKER_LABEL: usize = 64;
/// The refusal when the grid or the beat length needs the session tempo and no placement is given.
pub(crate) const NEEDS_AT: &str = "Detect needs a placement (at) to read the session tempo.";
/// The detector settings: they travel with Detect and are stored by Replace markers.
const DETECTOR_KEYS: [&str; 5] = ["source", "sensitivity", "division", "bars", "grid_bpm"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.sampler_slice_loop",
            "Make Slice Loop",
            [],
            None,
            "{track?, source: id | path | import: id, copy?: false, start?: 0, end?: the source's length, root?: 0-127 (stored, absolute) | null (default: none stored, it follows the pad start note)}: the patch's one slice loop over the native-source-frame region, mode Auto, both sets at the loop start only → {track, source, start, end, mode, root (in use), root_stored}",
            has_selection,
            slice_loop
        ),
        cmd!(
            "mix.sampler_slice_mode",
            "Set Slice Mode",
            [],
            None,
            "{track?, mode: auto|manual}: shows that mode's own marker set; neither set changes → {track, mode}",
            has_selection,
            slice_mode
        ),
        cmd!(
            "mix.sampler_slice_marker",
            "Edit Slice Marker",
            [],
            None,
            "{track?, op: add|move|remove|label, frame? (add, move: source frames), id? (move, remove, label), label?: text|null (label; trimmed, 64 characters at most), snap?: the loop's snap (add, move)}: edits the showing set; in Auto an accepted edit first copies Auto into Manual and switches to Manual, one undo step → {track, op, id, frame, requested, snapped, clamped: null|{at_marker: id, text}|{at_end: true, text}, set, mode, copied_auto, fixed, merged_into?, label?, freed_pads, freed_zones}",
            has_selection,
            marker
        ),
        cmd!(
            "mix.sampler_slice_manual_from_auto",
            "Start Slices from Auto's Markers",
            [],
            None,
            "{track?}: Manual becomes a copy of Auto's accepted markers (labels kept, new ids) and Manual shows; the previous Manual set's bindings are freed → {track, mode, markers, freed_pads, freed_zones}",
            has_selection,
            manual_from_auto
        ),
        cmd!(
            "mix.sampler_slice_manual_clear",
            "Clear Manual Slices",
            [],
            None,
            "{track?}: Manual keeps only the loop start (one slice over the loop); Auto is unchanged → {track, markers, freed_pads, freed_zones}",
            has_selection,
            manual_clear
        ),
        cmd!(
            "mix.sampler_slice_set",
            "Set Slice Settings",
            [],
            None,
            "{track?, snap?: bool, root?: 0-127 (stored, absolute) | null (follow the pad start note), beat_length?: quarter notes > 0 | null}: the shared slicing settings; no marker changes (the detector settings source, sensitivity, division, bars and grid_bpm travel with Detect) → {track, snap, root (in use), root_stored, beat_length}",
            has_selection,
            slice_set
        ),
        cmd!(query
            "mix.sampler_slice_state",
            "Slice State",
            [],
            None,
            "{track?, at?: tick (the placement; the session tempo and meter are read there)}: the loop, both sets (per slice: id, frame, end, label, pad, key, zones), mode, snap, root (in use) and root_stored, beat length and its default, detector, division, bars, grid_bpm, the grid tempo and its origin, and mapping: ascending|stale|none (stale: the first two out-of-order slices), and every slice-bound zone as mix.sampler_state reports it (loop_mode, loop_used, crossfade, crossfade_used, fades, fades_used, slice, region, ...) → {track, loop: null | {.., zones}}",
            has_selection,
            state
        ),
    ]
}

/// A `root` parameter: 0-127 stores it (absolute), `null` clears it (the loop then follows the pad
/// start note, ADR 0041 amendment 2026-11-09 item 7). The root a run reads is
/// [`PatchV1::slice_root`].
fn root_param(id: &str, v: &Value) -> Result<Option<u8>> {
    match v {
        Value::Null => Ok(None),
        v => v.as_u64().and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 127).map(Some).ok_or_else(|| bad(id, "`root` must be 0-127, or null")),
    }
}

/// Rewrites the stored `source`, `start`, `end`, `loop_start` and `loop_end` of every zone bound
/// to a slice of `set` with the slice's derived region ([`derived_region`]; ADR 0041 amendment
/// 2026-11-08, item 1), inside the calling command's one undo step. Called by every marker add,
/// move and remove, Clear and Start from Auto's markers (and a gesture in Auto, which replaces
/// Manual with the copy), Replace markers and Slices to pads, after the set and the bindings are
/// edited and before the patch is stored. A zone whose slice was removed has already been freed,
/// so every zone it meets derives a region. A source reference that no zone uses any more after
/// the rewrite is dropped, as every zone edit does.
pub(super) fn write_through_slice_zones(patch: &mut PatchV1, set: SliceSet) {
    let derived: Vec<(u16, DerivedRegion)> = patch
        .zones
        .iter()
        .filter(|z| z.slice.as_ref().is_some_and(|b| b.set == set))
        .filter_map(|z| derived_region(z, patch).map(|d| (z.id, d)))
        .collect();
    let mut source_changed = false;
    for (id, d) in derived {
        if let Some(z) = patch.zone_mut(id) {
            let old = z.source;
            write_region(z, d);
            source_changed |= old != d.source;
        }
    }
    if source_changed {
        patch.drop_unused_sources();
    }
}

pub(super) fn loop_of<'a>(id: &str, p: &'a PatchV1) -> Result<&'a SliceLoop> {
    p.slicing.as_ref().ok_or_else(|| bad(id, "the patch has no slice loop: make one with mix.sampler_slice_loop"))
}

fn loop_mut<'a>(id: &str, p: &'a mut PatchV1) -> Result<&'a mut SliceLoop> {
    p.slicing.as_mut().ok_or_else(|| bad(id, "the patch has no slice loop"))
}

fn mode_name(m: SliceMode) -> &'static str {
    match m {
        SliceMode::Auto => "auto",
        SliceMode::Manual => "manual",
    }
}

pub(super) fn set_of(l: &SliceLoop, set: SliceSet) -> Option<&Markers> {
    match set {
        SliceSet::Auto => l.auto.as_ref(),
        SliceSet::Manual => l.manual.as_ref(),
    }
}

/// Empties every zone bound to one of `ids` in `set` and unbinds its pad (the note stays):
/// the (1-based) pads and zones freed. Bindings to the other set are never touched.
pub(super) fn free_bindings(p: &mut PatchV1, set: SliceSet, ids: &[u32]) -> (Vec<u8>, Vec<u16>) {
    let gone: Vec<u16> = p.zones.iter().filter(|z| z.slice.as_ref().is_some_and(|b| b.set == set && ids.contains(&b.id))).map(|z| z.id).collect();
    if gone.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut pads = Vec::new();
    for z in &gone {
        if let Some(pad) = p.pad_of(*z) {
            p.set_pad(PadEntry { zone: None, ..p.pad(pad) });
            pads.push(pad + 1);
        }
    }
    p.zones.retain(|z| !gone.contains(&z.id));
    for z in &gone {
        // An emptied zone's controls go with it (#478), as Clear pad's do.
        p.reset_controls(*z);
    }
    p.drop_unused_sources();
    pads.sort_unstable();
    (pads, gone.iter().map(|z| z + 1).collect())
}

/// The 1-based position of the marker at `frame` in `set` (S is 1; an empty set is S only).
fn position_at(set: &Markers, frame: u64) -> usize {
    set.markers.iter().position(|m| m.frame == frame).map_or(1, |i| i + 1)
}

/// A refused marker edit's status text.
fn marker_text(err: MarkerError, set: &Markers, op: &str, frame: u64) -> String {
    match err {
        MarkerError::OnMarker => format!("Cannot add slice: too close to marker {}", position_at(set, frame)),
        MarkerError::Fixed if op == "remove" => "The loop start cannot be removed.".into(),
        e => e.to_string(),
    }
}

/// A source frame param: an integer, or a fractional pointer position rounded to the nearest
/// frame (ties upward). A negative position is outside the loop.
fn frame_param(id: &str, p: &Value) -> Result<u64> {
    let v = p.get("frame").ok_or_else(|| bad(id, "`frame` required (source frames)"))?;
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    match v.as_f64().filter(|x| x.is_finite()) {
        Some(x) if x < 0.0 => Err(bad(id, MarkerError::OutsideLoop.to_string())),
        Some(x) => Ok(slicing::round_request(x)),
        None => Err(bad(id, "`frame` must be a number of source frames")),
    }
}

fn marker_id_param(id: &str, p: &Value) -> Result<u32> {
    p.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(id, "`id` required (a slice id)"))
}

/// The zero crossing for a snapped edit, read from the loop source's decoded channels in the
/// session's pool (never the audio thread's copy). Missing audio refuses.
fn snap_frame(s: &Session, p: &PatchV1, l: &SliceLoop, requested: u64, lo: u64, hi: u64) -> std::result::Result<u64, SnapError> {
    let audio = p.sources.get(usize::from(l.source)).and_then(|r| s.pool.get(r.source)).filter(|a| !a.is_disk()).ok_or(SnapError::NoSource)?;
    let channels: Vec<&[f32]> = audio.buffer.channels.iter().map(Vec::as_slice).collect();
    slicing::snap_to_zero_crossing(&channels, requested, audio.buffer.sample_rate, lo, hi)
}

fn slice_loop(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_loop";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    if patch.slicing.is_some() {
        return Err(bad(id, "the patch already has a slice loop (one loop per patch)"));
    }
    let (sid, ownership) = match source_param(e, id, p, &patch)? {
        NewSource::Session(sid, ownership) => (sid, ownership),
        NewSource::Audio(audio, ownership) => (add_audio(e, audio, ownership), ownership),
    };
    let frames = e.session().pool.get(sid).map_or(0, |a| a.frames() as u64);
    let start = match p.get("start") {
        None => 0,
        Some(v) => v.as_u64().ok_or_else(|| bad(id, "`start` must be a source frame"))?,
    };
    let end = match p.get("end") {
        None => frames,
        Some(v) => v.as_u64().ok_or_else(|| bad(id, "`end` must be a source frame"))?,
    };
    if end <= start {
        return Err(bad(id, "the loop is empty: `end` must be after `start`"));
    }
    if end > frames {
        return Err(bad(id, format!("`end` is past the source's {frames} frames")));
    }
    let root = p.get("root").map(|v| root_param(id, v)).transpose()?.flatten();
    let index = u16::try_from(patch.source_index(sid, ownership)).map_err(|_| bad(id, "too many sources"))?;
    patch.slicing = Some(SliceLoop { source: index, start, end, root, ..SliceLoop::default() });
    let used = patch.slice_root();
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "source": sid, "start": start, "end": end, "mode": "auto", "root": used, "root_stored": root.is_some()}))
}

fn slice_mode(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_mode";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let mode = match str_param(p, "mode") {
        Some("auto") => SliceMode::Auto,
        Some("manual") => SliceMode::Manual,
        _ => return Err(bad(id, "`mode` must be auto or manual")),
    };
    // A Manual set never selected before is S only: absent, so nothing is written for it.
    let l = loop_mut(id, &mut patch)?;
    if l.mode != mode {
        l.mode = mode;
        store(e, id, t, patch)?;
    }
    Ok(json!({"track": t, "mode": mode_name(mode)}))
}

fn marker(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_marker";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let l = loop_of(id, &patch)?.clone();
    let op = str_param(p, "op").ok_or_else(|| bad(id, "`op` required (add, move, remove or label)"))?;
    if !["add", "move", "remove", "label"].contains(&op) {
        return Err(bad(id, "`op` must be add, move, remove or label"));
    }
    let snap = match p.get("snap") {
        None => l.snap,
        Some(v) => v.as_bool().ok_or_else(|| bad(id, "`snap` must be true or false"))?,
    };
    let (start, end) = (l.start, l.end);
    let in_auto = l.mode == SliceMode::Auto;
    let shown = if in_auto { l.auto.clone() } else { l.manual.clone() }.unwrap_or_default();
    // The set the gesture edits: Manual, which in Auto first becomes a copy of Auto (new ids).
    let mut m = l.manual.clone().unwrap_or_default();
    let replaced = if in_auto { m.copy_from(start, end, &shown).map_err(|er| bad(id, er.to_string()))? } else { Vec::new() };
    // An id aimed at the showing set names, in Auto, its copy: the Manual marker at that frame.
    let target = |given: u32| -> Result<u32> {
        let frame = shown.markers.iter().find(|x| x.id == given).map(|x| x.frame).ok_or_else(|| bad(id, MarkerError::NoMarker(given).to_string()))?;
        m.markers.iter().find(|x| x.frame == frame).map(|x| x.id).ok_or_else(|| bad(id, MarkerError::NoMarker(given).to_string()))
    };
    let mut out =
        json!({"track": t, "op": op, "set": "manual", "mode": "manual", "copied_auto": in_auto, "fixed": false, "clamped": null, "snapped": false});
    let mut removed = Vec::new();
    match op {
        "add" => {
            let requested = frame_param(id, p)?;
            let (lo, hi) = m.legal_add(start, end, requested).map_err(|er| bad(id, marker_text(er, &m, op, requested)))?;
            if m.markers.len().max(1) >= MAX_SLICES {
                return Err(bad(id, FULL_TEXT));
            }
            let frame = if snap { snap_frame(e.session(), &patch, &l, requested, lo, hi).map_err(|er| bad(id, er.to_string()))? } else { requested };
            let new = m.add(start, end, frame).map_err(|er| bad(id, marker_text(er, &m, op, frame)))?;
            out["id"] = json!(new);
            out["frame"] = json!(frame);
            out["requested"] = json!(requested);
            out["snapped"] = json!(snap);
        }
        "move" => {
            let given = marker_id_param(id, p)?;
            let requested = frame_param(id, p)?;
            let mid = target(given)?;
            out["id"] = json!(mid);
            out["requested"] = json!(requested);
            match m.legal_move(start, end, mid) {
                Err(MarkerError::Fixed) if in_auto => {
                    out["fixed"] = json!(true);
                    out["frame"] = json!(start);
                }
                Err(er) => return Err(bad(id, marker_text(er, &m, op, requested))),
                Ok((lo, hi)) => {
                    // The clamp to the legal interval comes first; the snap searches inside it.
                    let mut clamped = m.clone();
                    let moved = clamped.move_to(start, end, mid, requested).map_err(|er| bad(id, er.to_string()))?;
                    let frame = if snap {
                        let f = snap_frame(e.session(), &patch, &l, moved.frame, lo, hi).map_err(|er| bad(id, er.to_string()))?;
                        m.move_to(start, end, mid, f).map_err(|er| bad(id, er.to_string()))?.frame
                    } else {
                        m = clamped;
                        moved.frame
                    };
                    out["frame"] = json!(frame);
                    out["snapped"] = json!(snap);
                    out["clamped"] = match moved.clamped {
                        None => Value::Null,
                        Some(c @ ClampedAt::Marker { id: at, .. }) => json!({"at_marker": at, "text": c.to_string()}),
                        Some(c @ ClampedAt::End) => json!({"at_end": true, "text": c.to_string()}),
                    };
                }
            }
        }
        "remove" => {
            let mid = target(marker_id_param(id, p)?)?;
            out["id"] = json!(mid);
            match m.remove(start, end, mid) {
                Err(MarkerError::Fixed) if in_auto => {
                    out["fixed"] = json!(true);
                    out["frame"] = json!(start);
                }
                Err(er) => return Err(bad(id, marker_text(er, &m, op, start))),
                Ok(r) => {
                    out["merged_into"] = json!(r.merged_into);
                    removed.push(r.id);
                }
            }
        }
        _ => {
            let mid = target(marker_id_param(id, p)?)?;
            let label = match p.get("label") {
                None => return Err(bad(id, "`label` required (text, or null to clear)")),
                Some(Value::Null) => None,
                Some(Value::String(x)) => Some(x.trim().to_string()).filter(|x| !x.is_empty()),
                Some(_) => return Err(bad(id, "`label` must be text or null")),
            };
            if label.as_ref().is_some_and(|x| x.chars().count() > MAX_MARKER_LABEL) {
                return Err(bad(id, format!("The label is at most {MAX_MARKER_LABEL} characters.")));
            }
            m.relabel(mid, label.clone()).map_err(|er| bad(id, er.to_string()))?;
            out["id"] = json!(mid);
            out["label"] = json!(label);
        }
    }
    // Accepted: Manual takes the edited set (the copy in Auto) and shows.
    removed.extend(replaced);
    let (pads, zones) = free_bindings(&mut patch, SliceSet::Manual, &removed);
    let lm = loop_mut(id, &mut patch)?;
    lm.manual = Some(m);
    lm.mode = SliceMode::Manual;
    if in_auto || op != "label" {
        write_through_slice_zones(&mut patch, SliceSet::Manual);
    }
    store(e, id, t, patch)?;
    out["freed_pads"] = json!(pads);
    out["freed_zones"] = json!(zones);
    Ok(out)
}

fn manual_from_auto(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_manual_from_auto";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let l = loop_of(id, &patch)?;
    let auto = l.auto.clone().unwrap_or_default();
    let mut m = l.manual.clone().unwrap_or_default();
    let replaced = m.copy_from(l.start, l.end, &auto).map_err(|er| bad(id, er.to_string()))?;
    let (pads, zones) = free_bindings(&mut patch, SliceSet::Manual, &replaced);
    let lm = loop_mut(id, &mut patch)?;
    lm.manual = Some(m);
    lm.mode = SliceMode::Manual;
    write_through_slice_zones(&mut patch, SliceSet::Manual);
    let markers = markers_json(&patch, SliceSet::Manual);
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "mode": "manual", "markers": markers, "freed_pads": pads, "freed_zones": zones}))
}

fn manual_clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_manual_clear";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let l = loop_of(id, &patch)?;
    let (start, end) = (l.start, l.end);
    let Some(mut m) = l.manual.clone().filter(|m| m.markers.len() > 1) else {
        // Already S only: nothing changes and no step is recorded.
        return Ok(json!({"track": t, "markers": markers_json(&patch, SliceSet::Manual), "freed_pads": [], "freed_zones": []}));
    };
    let removed = m.clear_to_start(start, end).map_err(|er| bad(id, er.to_string()))?;
    let (pads, zones) = free_bindings(&mut patch, SliceSet::Manual, &removed);
    loop_mut(id, &mut patch)?.manual = Some(m);
    write_through_slice_zones(&mut patch, SliceSet::Manual);
    let markers = markers_json(&patch, SliceSet::Manual);
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "markers": markers, "freed_pads": pads, "freed_zones": zones}))
}

fn slice_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_set";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    if let Some(k) = DETECTOR_KEYS.iter().find(|k| p.get(**k).is_some()) {
        return Err(bad(id, format!("`{k}` travels with Detect and is stored by Replace markers")));
    }
    let l = loop_mut(id, &mut patch)?;
    if let Some(v) = p.get("snap") {
        l.snap = v.as_bool().ok_or_else(|| bad(id, "`snap` must be true or false"))?;
    }
    if let Some(v) = p.get("root") {
        l.root = root_param(id, v)?;
    }
    if let Some(v) = p.get("beat_length") {
        l.beat_length = match v {
            Value::Null => None,
            v => Some(
                v.as_f64()
                    .filter(|b| b.is_finite() && *b > 0.0)
                    .ok_or_else(|| bad(id, "`beat_length` must be a number of quarter notes above 0, or null"))?,
            ),
        };
    }
    let (snap, stored, beat_length) = (l.snap, l.root.is_some(), l.beat_length);
    let out = json!({"track": t, "snap": snap, "root": patch.slice_root(), "root_stored": stored, "beat_length": beat_length});
    if patch_of(e.session(), id, t)? != patch {
        store(e, id, t, patch)?;
    }
    Ok(out)
}

/// Where the grid's tempo comes from (ADR 0041 amendment 2026-11-08, item 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TempoOrigin {
    Bars,
    GridBpm,
    Acid,
    Session,
}

impl TempoOrigin {
    pub(crate) fn name(self) -> &'static str {
        match self {
            TempoOrigin::Bars => "bars",
            TempoOrigin::GridBpm => "grid_bpm",
            TempoOrigin::Acid => "acid",
            TempoOrigin::Session => "session",
        }
    }
}

/// The session's tempo and meter at the placement `at`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AtTempo {
    pub bpm: f64,
    pub meter: (u32, u32),
}

impl AtTempo {
    pub(crate) fn read(s: &Session, at: i64) -> AtTempo {
        let m = s.tempo.meter_at_tick(at);
        AtTempo { bpm: s.tempo.tempo_at_tick(at), meter: (m.numerator, m.denominator) }
    }
}

/// A file's `acid` tempo when it is usable: finite and inside `grid_bpm`'s 5 to 1000 BPM.
pub(crate) fn usable_acid(acid: Option<f64>) -> Option<f64> {
    acid.filter(|b| b.is_finite() && (5.0..=1000.0).contains(b))
}

/// The grid's tempo and its origin, in priority: an explicit Bars count (the derived BPM, which
/// reads the meter at the placement), else `grid_bpm`, else the file's usable `acid` tempo, else
/// the session tempo at the placement. `rate` is the source's. Without a placement where one is
/// read, [`NEEDS_AT`].
pub(crate) fn grid_tempo(l: &SliceLoop, rate: u32, acid: Option<f64>, at: Option<AtTempo>) -> std::result::Result<(f64, TempoOrigin), &'static str> {
    if let Some(n) = l.bars {
        let (u, v) = at.ok_or(NEEDS_AT)?.meter;
        // The body's derived BPM, as `slicing::grid_boundaries` computes it for a bar count:
        // B = n * u * 4/v quarter notes span the loop.
        let quarters = f64::from(n) * f64::from(u) * 4.0 / f64::from(v.max(1));
        return Ok((60.0 * quarters * f64::from(rate) / (l.end.saturating_sub(l.start)).max(1) as f64, TempoOrigin::Bars));
    }
    if let Some(b) = l.grid_bpm {
        return Ok((b, TempoOrigin::GridBpm));
    }
    if let Some(b) = usable_acid(acid) {
        return Ok((b, TempoOrigin::Acid));
    }
    at.map(|a| (a.bpm, TempoOrigin::Session)).ok_or(NEEDS_AT)
}

/// Slice to MIDI's default beat length in quarter notes: the loop's duration at the file's usable
/// `acid` tempo, else at the session tempo at the placement (`grid_bpm` and Bars never change it).
pub(crate) fn default_beat_length(l: &SliceLoop, rate: u32, acid: Option<f64>, at: Option<AtTempo>) -> Option<f64> {
    let bpm = usable_acid(acid).or(at.map(|a| a.bpm))?;
    (rate > 0).then(|| (l.end.saturating_sub(l.start)) as f64 / f64::from(rate) * bpm / 60.0)
}

/// One slice of a set as the state reports it.
pub(super) struct SliceView {
    pub(super) id: Option<u32>,
    pub(super) frame: u64,
    pub(super) end: u64,
    pub(super) label: Option<String>,
    pub(super) pad: Option<u8>,
    pub(super) key: Option<u8>,
    pub(super) zones: Vec<u16>,
}

/// The slices of `set` in order (an absent or empty set is S alone, without an id).
pub(super) fn slices(p: &PatchV1, set: SliceSet) -> Vec<SliceView> {
    let Some(l) = &p.slicing else { return Vec::new() };
    let starts: Vec<(Option<u32>, u64, Option<String>)> = match set_of(l, set).filter(|m| !m.markers.is_empty()) {
        Some(m) => m.markers.iter().map(|x| (Some(x.id), x.frame, x.label.clone())).collect(),
        None => vec![(None, l.start, None)],
    };
    let ends: Vec<u64> = starts.iter().skip(1).map(|s| s.1).chain([l.end]).collect();
    starts
        .into_iter()
        .zip(ends)
        .map(|((id, frame, label), end)| {
            let zones: Vec<u16> =
                p.zones.iter().filter(|z| id.is_some() && z.slice.as_ref().is_some_and(|b| b.set == set && Some(b.id) == id)).map(|z| z.id).collect();
            let pad = zones.iter().find_map(|z| p.pad_of(*z));
            SliceView { id, frame, end, label, pad, key: pad.map(|x| p.pad_note(x)), zones }
        })
        .collect()
}

fn slice_json(s: &SliceView) -> Value {
    json!({
        "id": s.id, "frame": s.frame, "end": s.end, "label": s.label,
        "pad": s.pad.map(|x| x + 1), "key": s.key, "zones": s.zones.iter().map(|z| z + 1).collect::<Vec<_>>(),
    })
}

fn markers_json(p: &PatchV1, set: SliceSet) -> Value {
    Value::Array(slices(p, set).iter().map(slice_json).collect())
}

/// The showing set's pad mapping as the state reports it ([`mapping_status`]): `ascending` with
/// `mapping_root`, `stale` with the first two `out_of_order` slices, or `none`.
fn mapping(p: &PatchV1, set: SliceSet) -> Value {
    match mapping_status(p, set) {
        MappingStatus::None => json!({"mapping": "none"}),
        MappingStatus::Ascending { root } => json!({"mapping": "ascending", "mapping_root": root}),
        MappingStatus::Stale { first_two } => {
            let pair = |s: &StaleSlice| json!({"slice": s.slice, "id": s.id, "pad": s.pad.map(|x| x + 1), "key": s.key});
            json!({"mapping": "stale", "out_of_order": [pair(&first_two[0]), pair(&first_two[1])]})
        }
    }
}

fn state(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_state";
    let t = instrument_track(e, id, p)?;
    let patch = patch_of(e.session(), id, t)?;
    let at = match p.get("at") {
        None => None,
        Some(v) => Some(v.as_i64().filter(|x| *x >= 0).ok_or_else(|| bad(id, "`at` must be a tick at or after 0"))?),
    };
    let Some(l) = &patch.slicing else { return Ok(json!({"track": t, "loop": null})) };
    // The file's `acid` tempo: the grid's tempo before the session's, and the default beat length.
    let acid = super::sampler_acid::loop_acid(e, id, &patch, l);
    let s = e.session();
    let source = patch.sources.get(usize::from(l.source)).map(|r| r.source);
    let rate =
        source.and_then(|x| s.pool.get(x).map(|a| a.buffer.sample_rate).or_else(|| s.source(x).map(|a| a.sample_rate))).unwrap_or(s.sample_rate.hz());
    let at_tempo = at.map(|x| AtTempo::read(s, x));
    let grid = grid_tempo(l, rate, acid, at_tempo).ok().map(|(bpm, origin)| json!({"bpm": bpm, "origin": origin.name()}));
    let showing = match l.mode {
        SliceMode::Auto => SliceSet::Auto,
        SliceMode::Manual => SliceSet::Manual,
    };
    let mut out = json!({
        "source": source, "start": l.start, "end": l.end, "rate": rate, "mode": mode_name(l.mode), "snap": l.snap,
        "root": patch.slice_root(), "root_stored": l.root.is_some(),
        "detector": {"source": l.detector.source, "sensitivity": l.detector.sensitivity},
        "division": l.division, "bars": l.bars, "grid_bpm": l.grid_bpm, "grid_tempo": grid,
        "beat_length": l.beat_length, "default_beat_length": default_beat_length(l, rate, acid, at_tempo),
        "auto": markers_json(&patch, SliceSet::Auto), "manual": markers_json(&patch, SliceSet::Manual),
        // Every slice-bound zone (either set) as `mix.sampler_state` reports it: the stored loop
        // mode, crossfade and fades beside the values playback uses.
        "zones": patch.zones.iter().filter(|z| z.slice.is_some()).map(|z| zone_json(s, &patch, z)).collect::<Vec<_>>(),
    });
    if let (Some(o), Value::Object(m)) = (out.as_object_mut(), mapping(&patch, showing)) {
        o.extend(m);
    }
    Ok(json!({"track": t, "loop": out}))
}

#[cfg(test)]
#[path = "tests_sampler_slices.rs"]
mod tests;

#[cfg(test)]
#[path = "tests_sampler_slice_write_through.rs"]
mod write_through_tests;

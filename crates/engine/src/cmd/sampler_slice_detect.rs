//! Auto mode's detection commands (ADR 0041 "Slice sources and Slice to MIDI" and its amendment
//! 2026-11-08, items 5 and 7; #269): Detect prepares a candidate marker set off the control
//! thread ([`crate::slice_detect`]), the UI polls it as a preview, Replace markers accepts it as
//! one undo step and Keep current drops it.
//!
//! - **Detect and discard are not edits.** Neither changes the document, records an undo step or
//!   is journaled; the candidate lives on the engine and is never saved or autosaved.
//! - **Settings travel with the candidate.** Detect takes the detector and grid settings (each
//!   defaults to the loop's stored one); Replace stores them as the loop's in its own step, so a
//!   settings change never makes its own candidate stale.
//! - **Replace touches Auto only.** Markers at a frame the old Auto set already had keep their ids
//!   (and so their bindings); every other marker gets a new id, and the bindings of removed Auto
//!   markers are freed and reported. Manual and the mode are left as they are.
//! - **The engine never guesses UI state.** The placement `at` is passed by the UI; Detect refuses
//!   without it when the grid reads the session tempo.

use super::sampler::instrument_track;
use super::sampler_patch::{patch_of, resolve_path, store};
use super::sampler_slices::{AtTempo, TempoOrigin, free_bindings, write_through_slice_zones};
use super::*;
use crate::cmd;
use crate::slice_detect::{Candidate, FileRef, Job, PENDING, STALE, Settings, State};
use serde_json::json;
use transit_audio_io::FileFormat;
use transit_model::sampler_patch::{Marker, Markers, PatchV1, SliceLoop, SliceSet};
use transit_model::{Session, TrackId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query
            "mix.sampler_slice_detect",
            "Detect Slices",
            [],
            None,
            "{track?, source?: auto|cues|transients|grid, sensitivity?: 0-1, division?: 1/4|1/8|1/16|1/32, bars?: 1-16|null, grid_bpm?: 5-1000|null (each defaults to the loop's stored setting), at?: tick (the placement; required when the grid reads the session tempo)}: prepares a candidate marker set on a worker thread, replacing the track's previous candidate; not an edit → {track, candidate_id, state: \"pending\"} (poll mix.sampler_slice_candidate)",
            has_selection,
            detect
        ),
        cmd!(query
            "mix.sampler_slice_candidate",
            "Slice Detection",
            [],
            None,
            "{track?}: the track's detection candidate → {track, candidate: null | {candidate_id, state: pending|ready|refused, stale, settings, current_count, error? (refused), source_used, markers: [{frame, label?}], count, note?, bpm?, tempo_origin?: bars|grid_bpm|acid|session, derived_bpm? (ready)}}",
            has_selection,
            candidate
        ),
        cmd!(@spec false, true, false;
            "mix.sampler_slice_replace",
            "Replace Slice Markers",
            [],
            None,
            "{track?, candidate_id}: Auto takes the candidate's markers (ids kept at unchanged frames, new ids elsewhere; removed markers' bindings freed) and the loop stores the candidate's settings, one undo step; Manual and the mode are unchanged. Refused while the candidate is pending or after the patch changed → {track, candidate_id, source_used, markers, kept, new, removed, freed_pads, freed_zones, settings}",
            has_selection,
            replace
        ),
        cmd!(query
            "mix.sampler_slice_discard",
            "Keep Current Slices",
            [],
            None,
            "{track?, candidate_id?}: drops the track's candidate (only that one when an id is given); not an edit → {track, discarded: id | null}",
            has_selection,
            discard
        ),
    ]
}

/// Track `t`'s ready Sampler patch, if it has one.
pub(crate) fn ready_sampler_patch(s: &Session, t: TrackId) -> Option<&PatchV1> {
    s.track(t).and_then(|tr| tr.instrument.as_ref()).filter(|i| i.plugin == "sampler").and_then(|i| i.sampler_patch.as_ref()).and_then(|p| p.ready())
}

fn loop_of<'a>(id: &str, p: &'a PatchV1) -> Result<&'a SliceLoop> {
    p.slicing.as_ref().ok_or_else(|| bad(id, "the patch has no slice loop: make one with mix.sampler_slice_loop"))
}

/// The settings Detect was given, each defaulting to the loop's stored one.
fn settings_param(id: &str, p: &Value, l: &SliceLoop) -> Result<Settings> {
    let mut s = Settings::stored(l);
    if let Some(v) = p.get("source") {
        s.source = serde_json::from_value(v.clone()).map_err(|_| bad(id, "`source` must be auto, cues, transients or grid"))?;
    }
    if let Some(v) = p.get("sensitivity") {
        let x = v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(id, "`sensitivity` must be from 0 to 1"))?;
        s.sensitivity = x as f32;
    }
    if let Some(v) = p.get("division") {
        s.division = serde_json::from_value(v.clone()).map_err(|_| bad(id, "`division` must be 1/4, 1/8, 1/16 or 1/32"))?;
    }
    if let Some(v) = p.get("bars") {
        s.bars = match v {
            Value::Null => None,
            v => Some(
                v.as_u64()
                    .filter(|n| (1..=16).contains(n))
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| bad(id, "`bars` must be 1 to 16, or null for From tempo"))?,
            ),
        };
    }
    if let Some(v) = p.get("grid_bpm") {
        s.grid_bpm = match v {
            Value::Null => None,
            v => Some(
                v.as_f64()
                    .filter(|b| b.is_finite() && (5.0..=1000.0).contains(b))
                    .ok_or_else(|| bad(id, "`grid_bpm` must be from 5 to 1000 BPM, or null"))?,
            ),
        };
    }
    Ok(s)
}

fn settings_json(s: &Settings) -> Value {
    json!({"source": s.source, "sensitivity": s.sensitivity, "division": s.division, "bars": s.bars, "grid_bpm": s.grid_bpm})
}

fn detect(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_detect";
    let t = instrument_track(e, id, p)?;
    let patch = patch_of(e.session(), id, t)?;
    let l = loop_of(id, &patch)?;
    let settings = settings_param(id, p, l)?;
    let at = match p.get("at") {
        None => None,
        Some(v) => Some(v.as_i64().filter(|x| *x >= 0).ok_or_else(|| bad(id, "`at` must be a tick at or after 0"))?),
    };
    let s = e.session();
    let sid = patch.sources.get(usize::from(l.source)).map(|r| r.source).ok_or_else(|| bad(id, "the loop's source is not in the patch"))?;
    let src = s.source(sid).ok_or_else(|| bad(id, format!("no source {sid}")))?;
    let audio = s.pool.get(sid).filter(|a| !a.is_disk()).cloned().ok_or_else(|| bad(id, format!("{}: no decoded audio to detect in", src.name)))?;
    // Cue points and the `acid` tempo come from the source's WAV file; audio only in memory (not
    // written yet) has none.
    let file = if src.format == FileFormat::Wav && !src.unsaved {
        Some(FileRef { path: resolve_path(e, id, &src.path)?, roots: crate::file_access::current() })
    } else {
        None
    };
    let job = Job { start: l.start, end: l.end, audio, file, settings: settings.clone(), at: at.map(|x| AtTempo::read(s, x)) };
    let current = l.auto.as_ref().map_or(1, |m| m.markers.len().max(1));
    let cid = e.slice_candidates.start(t, patch, settings, current, job);
    Ok(json!({"track": t, "candidate_id": cid, "state": "pending"}))
}

fn candidate_json(c: &Candidate) -> Value {
    let mut v = json!({
        "candidate_id": c.id, "stale": c.stale, "current_count": c.current_count, "settings": settings_json(&c.settings),
    });
    match &c.state {
        State::Pending => v["state"] = json!("pending"),
        State::Refused(m) => {
            v["state"] = json!("refused");
            v["error"] = json!(m);
        }
        State::Ready(f) => {
            v["state"] = json!("ready");
            v["source_used"] = json!(f.source_used);
            v["markers"] = f
                .markers
                .iter()
                .map(|(frame, label)| match label {
                    Some(l) => json!({"frame": frame, "label": l}),
                    None => json!({"frame": frame}),
                })
                .collect();
            v["count"] = json!(f.markers.len());
            if !f.notes.is_empty() {
                v["note"] = json!(f.notes.join(" "));
            }
            if let Some((bpm, origin)) = f.tempo {
                v["bpm"] = json!(bpm);
                v["tempo_origin"] = json!(origin.name());
                if origin == TempoOrigin::Bars {
                    v["derived_bpm"] = json!(bpm);
                }
            }
        }
    }
    v
}

fn candidate(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_candidate";
    let t = instrument_track(e, id, p)?;
    let c = e.slice_candidates.get(t, ready_sampler_patch(e.session(), t));
    Ok(json!({"track": t, "candidate": c.as_ref().map(candidate_json)}))
}

/// The track's live candidate `cid`: refused when it was replaced, discarded or detected before
/// the patch last changed.
fn live(e: &Engine, id: &str, t: TrackId, cid: u64, current: &PatchV1) -> Result<Candidate> {
    match e.slice_candidates.get(t, Some(current)) {
        Some(c) if c.id == cid && c.stale => Err(bad(id, STALE)),
        Some(c) if c.id == cid => Ok(c),
        _ if e.slice_candidates.issued(cid) => Err(bad(id, STALE)),
        _ => Err(bad(id, format!("No detection {cid}: detect first."))),
    }
}

fn ids_json(m: &Markers) -> Value {
    m.markers.iter().map(|x| json!({"id": x.id, "frame": x.frame, "label": x.label})).collect()
}

fn replace(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_replace";
    let t = instrument_track(e, id, p)?;
    let cid = p.get("candidate_id").and_then(Value::as_u64).ok_or_else(|| bad(id, "`candidate_id` required (from mix.sampler_slice_detect)"))?;
    let current = patch_of(e.session(), id, t)?;
    let c = live(e, id, t, cid, &current)?;
    let found = match c.state {
        State::Pending => return Err(bad(id, PENDING)),
        State::Refused(m) => return Err(bad(id, format!("This detection was refused: {m}"))),
        State::Ready(f) => f,
    };
    let mut patch = current.clone();
    let l = loop_of(id, &patch)?;
    let (start, end) = (l.start, l.end);
    let old = l.auto.clone();
    let old_set = old.clone().unwrap_or_default();
    let mut m = old_set.clone();
    let mark = |er: transit_model::slicing::MarkerError| bad(id, er.to_string());
    // Removed first, so the set never holds more than the limit on the way.
    let frames: Vec<u64> = found.markers.iter().map(|x| x.0).collect();
    let mut removed = Vec::new();
    for x in old_set.markers.iter().skip(1).filter(|x| !frames.contains(&x.frame)) {
        m.remove(start, end, x.id).map_err(mark)?;
        removed.push(x.id);
    }
    for &f in frames.iter().skip(1) {
        if !m.markers.iter().any(|x| x.frame == f) {
            m.add(start, end, f).map_err(mark)?;
        }
    }
    // A set still S only stores S only to carry a cue label at the loop start.
    if m.markers.is_empty() && found.markers.first().is_some_and(|x| x.1.is_some()) {
        let sid = m.next_id.max(1);
        m.markers.push(Marker { id: sid, frame: start, ..Marker::default() });
        m.next_id = sid.saturating_add(1);
    }
    for (frame, label) in &found.markers {
        if let Some(mid) = m.markers.iter().find(|x| x.frame == *frame).map(|x| x.id) {
            m.relabel(mid, label.clone()).map_err(mark)?;
        }
    }
    let kept: Vec<u32> = old_set.markers.iter().filter(|x| m.markers.iter().any(|y| y.id == x.id)).map(|x| x.id).collect();
    let new: Vec<u32> = m.markers.iter().filter(|x| !old_set.markers.iter().any(|y| y.id == x.id)).map(|x| x.id).collect();
    let (pads, zones) = free_bindings(&mut patch, SliceSet::Auto, &removed);
    let markers = ids_json(&m);
    let lm = patch.slicing.as_mut().ok_or_else(|| bad(id, "the patch has no slice loop"))?;
    lm.auto = if old.is_none() && m == Markers::default() { None } else { Some(m) };
    let s = &c.settings;
    lm.detector.source = s.source;
    lm.detector.sensitivity = s.sensitivity;
    lm.division = s.division;
    lm.bars = s.bars;
    lm.grid_bpm = s.grid_bpm;
    write_through_slice_zones(&mut patch, SliceSet::Auto);
    if patch != current {
        store(e, id, t, patch)?;
    }
    e.slice_candidates.discard(t, Some(cid));
    Ok(json!({
        "track": t, "candidate_id": cid, "source_used": found.source_used, "markers": markers,
        "kept": kept, "new": new, "removed": removed, "freed_pads": pads, "freed_zones": zones, "settings": settings_json(s),
    }))
}

fn discard(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_slice_discard";
    let t = instrument_track(e, id, p)?;
    let cid = match p.get("candidate_id") {
        None => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(id, "`candidate_id` must be a candidate id"))?),
    };
    Ok(json!({"track": t, "discarded": e.slice_candidates.discard(t, cid)}))
}

#[cfg(test)]
#[path = "tests_sampler_slice_detect.rs"]
mod tests;

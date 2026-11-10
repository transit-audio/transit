//! Slice to MIDI (ADR 0041 "Slice sources and Slice to MIDI", its amendment 2026-11-08 item 9 and
//! its amendment 2026-11-09 item 7, #269): one note per slice of the showing set, written as a
//! MIDI clip on the instrument track at a placement the caller gives, in one undo step.
//!
//! - **Composition editing, not recording.** No capture journal is touched; the clip is placed
//!   with the ordinary clip placement, and an overlap becomes a new take by ADR 0039's path, the
//!   one recording uses: the active playlist is duplicated, the covered material numbered
//!   ([`crate::takes::number_covered`]), the clip placed, and the take settled
//!   ([`crate::takes::settle_new_takes`]).
//! - **Musical time.** Every note boundary is a tick (the session's 960 per quarter note). Fit
//!   length computes them in quarter notes from the beat length; Keep duration computes target
//!   samples and converts each through the tempo map once. Notes are stored at ticks relative to
//!   the clip's start tick, never at sample positions converted back.
//! - **The engine never guesses UI state.** `at` is required (item 9); the UI computes it from the
//!   selection, the edit cursor or bar 1 and names which in its placement line.
//! - **The mapping is checked, never repaired.** A stale or missing mapping refuses; Map slices
//!   to keys is a separate edit (slice E) the UI offers.
//! - **Refused whole.** Every refusal comes before any document change; `preview` returns the
//!   same answer and the same refusals and changes nothing.

use super::sampler::instrument_track;
use super::sampler_patch::patch_of;
use super::sampler_slice_pads::{MappingStatus, mapping_status, root_param, showing, too_high};
use super::sampler_slices::{AtTempo, SliceView, default_beat_length, loop_of, slices};
use super::*;
use crate::cmd;
use serde_json::json;
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::{PatchV1, SliceLoop, SliceSet};
use transit_model::{Clip, Session};
use transit_time::TICKS_PER_QUARTER;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.slice_to_midi",
        "Slice to MIDI",
        [],
        None,
        "{track?, timing: fit|keep, at: tick (required: the placement, computed by the UI), beat_length?: quarter notes (fit; default: the loop's stored beat length, else its duration at the session tempo at `at`), root?: must match the slices' ascending mapping, preview?: false}: one note per slice of the showing set (key root+i, velocity 100, channel 1, to the next boundary) as a MIDI clip at `at`; an overlap becomes a new take (ADR 0039); preview answers and edits nothing → {track, clip, notes, first_key, last_key, velocity: 100, channel: 1, at_tick, position, start, end, ticks, timing, beat_length, placement: explicit, takes, overlap?}",
        has_selection,
        slice_to_midi
    )]
}

const ID: &str = "mix.slice_to_midi";
const VELOCITY: u8 = 100;
/// The written channel, 0-based (channel 1).
const CHANNEL: u8 = 0;

/// The refusal without a placement (ADR 0041 amendment 2026-11-08, item 9).
pub(crate) const NEEDS_PLACEMENT: &str = "Slice to MIDI needs a placement.";

/// The refusal for a placement whose sample is past the timeline's end.
const PLACEMENT_PAST_END: &str = "The placement is past the end of the session's timeline.";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Timing {
    /// Fit length: the loop spans `beat_length` quarter notes.
    Fit,
    /// Keep duration: each slice keeps its duration in seconds at the source's rate.
    Keep,
}

/// The parameters, checked before anything else is read.
struct Params {
    at: i64,
    timing: Timing,
    beat_length: Option<f64>,
    root: Option<u8>,
    preview: bool,
}

fn params(p: &Value) -> Result<Params> {
    let at = match p.get("at") {
        None | Some(Value::Null) => return Err(bad(ID, NEEDS_PLACEMENT)),
        Some(v) => v.as_i64().filter(|x| *x >= 0).ok_or_else(|| bad(ID, "`at` must be a tick at or after 0"))?,
    };
    let timing = match p.get("timing").and_then(Value::as_str) {
        Some("fit") => Timing::Fit,
        Some("keep") => Timing::Keep,
        _ => return Err(bad(ID, "`timing` must be fit or keep")),
    };
    let beat_length = match p.get("beat_length") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .filter(|b| b.is_finite() && *b > 0.0 && *b <= 1.0e6)
                .ok_or_else(|| bad(ID, "`beat_length` must be a number of quarter notes above 0 (at most 1000000)"))?,
        ),
    };
    if beat_length.is_some() && timing == Timing::Keep {
        return Err(bad(ID, "`beat_length` is for fit timing; keep timing follows the source's duration"));
    }
    let root = p.get("root").map(|v| root_param(ID, v)).transpose()?;
    let preview = match p.get("preview") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(ID, "`preview` must be true or false"))?,
    };
    Ok(Params { at, timing, beat_length, root, preview })
}

/// `x` rounded to the nearest integer, ties upward.
fn nearest(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

/// The native rate of the loop's source, when its audio is in the session.
fn source_rate(s: &Session, p: &PatchV1, l: &SliceLoop) -> Option<u32> {
    let source = p.sources.get(usize::from(l.source))?.source;
    s.pool.get(source).map(|a| a.buffer.sample_rate).filter(|r| *r > 0)
}

/// The showing set's slices with ids, refused when it has none yet.
fn showing_slices(p: &PatchV1, l: &SliceLoop) -> Result<(SliceSet, Vec<SliceView>)> {
    let set = showing(l);
    let views = slices(p, set);
    if views.is_empty() || views.iter().any(|v| v.id.is_none()) {
        let name = if set == SliceSet::Auto { "Auto" } else { "Manual" };
        return Err(bad(ID, format!("The {name} set has no slice markers yet: detect or add slices first.")));
    }
    Ok((set, views))
}

/// The root the notes start at: the showing set's ascending mapping, which a chosen root must
/// match. A stale or missing mapping refuses and is never repaired here.
fn mapping_root(p: &PatchV1, set: SliceSet, n: usize, chosen: Option<u8>) -> Result<u8> {
    if let Some(r) = chosen
        && let Some(text) = too_high(r, n)
    {
        return Err(bad(ID, text));
    }
    let root = match mapping_status(p, set) {
        MappingStatus::Ascending { root } => root,
        MappingStatus::None => return Err(bad(ID, "The slices have no pads yet: use Slices to pads.")),
        MappingStatus::Stale { first_two: [a, b] } => {
            if let Some(s) = [&a, &b].into_iter().find(|s| s.key.is_none()) {
                return Err(bad(ID, format!("Slice {} has no pad: use Slices to pads.", s.slice)));
            }
            return Err(bad(ID, format!("The slices' keys no longer run in order: slices {} and {} swapped pads.", a.slice, b.slice)));
        }
    };
    if let Some(r) = chosen.filter(|r| *r != root) {
        return Err(bad(ID, format!("Root {r} does not match the slices' keys, which start at {root}: use Map slices to keys.")));
    }
    if let Some(text) = too_high(root, n) {
        return Err(bad(ID, text));
    }
    Ok(root)
}

/// The absolute tick of every boundary: each slice start, then the loop end.
fn boundaries(s: &Session, l: &SliceLoop, views: &[SliceView], at: i64, timing: Timing, beat: f64, rate: u32) -> Vec<i64> {
    let frames: Vec<u64> = views.iter().map(|v| v.frame).chain([l.end]).collect();
    let span = l.end.saturating_sub(l.start).max(1);
    match timing {
        // Q(P) + B * (marker - S) / (E - S) quarter notes, at 960 ticks each.
        Timing::Fit => {
            frames.iter().map(|m| at + nearest(TICKS_PER_QUARTER as f64 * beat * m.saturating_sub(l.start) as f64 / span as f64)).collect()
        }
        // P + round((marker - S) * session_rate / r), each converted to ticks through the map.
        Timing::Keep => {
            let sr = s.sample_rate;
            let p = s.tempo.tick_to_samples(at, sr);
            let (num, den) = (u128::from(sr.hz()), u128::from(rate.max(1)));
            frames
                .iter()
                .map(|m| {
                    let offset = (u128::from(m.saturating_sub(l.start)) * num * 2 + den) / (den * 2);
                    let sample = p.saturating_add(i64::try_from(offset).unwrap_or(i64::MAX));
                    nearest(s.tempo.render_ticks(sample, sr))
                })
                .collect()
        }
    }
}

/// The planned clip: notes relative to `at`, the boundaries, and the clip's sample range.
struct Plan {
    root: u8,
    ticks: Vec<i64>,
    notes: Vec<Note>,
    beat: Option<f64>,
    range: Range,
    overlap: bool,
}

fn plan(e: &Engine, t: TrackId, prm: &Params) -> Result<Plan> {
    let s = e.session();
    // Before any arithmetic on `at`: a placement past the timeline is refused, so the boundary
    // ticks below cannot overflow.
    if s.tempo.tick_to_samples(prm.at, s.sample_rate) > MAX_POSITION {
        return Err(bad(ID, PLACEMENT_PAST_END));
    }
    let tr = s.track(t).ok_or_else(|| bad(ID, "no track"))?;
    let patch = patch_of(s, ID, t)?;
    let l = loop_of(ID, &patch)?;
    if tr.freeze.is_some() {
        return Err(EngineError::TrackFrozen);
    }
    let (set, views) = showing_slices(&patch, l)?;
    let n = views.len();
    let root = mapping_root(&patch, set, n, prm.root)?;
    let rate = source_rate(s, &patch, l);
    let beat = match prm.timing {
        Timing::Keep => None,
        Timing::Fit => {
            // The acid tempo is read with the file (detection); the state query reports this same
            // default, so the panel shows the value used.
            let default = rate.and_then(|r| default_beat_length(l, r, None, Some(AtTempo::read(s, prm.at))));
            Some(
                prm.beat_length
                    .or(l.beat_length)
                    .or(default)
                    .ok_or_else(|| bad(ID, "Fit length needs a beat length: give one, or load the loop's source audio so its duration is known."))?,
            )
        }
    };
    let rate = match prm.timing {
        Timing::Keep => rate.ok_or_else(|| bad(ID, "Keep duration needs the source's sample rate: load the loop's source audio."))?,
        Timing::Fit => rate.unwrap_or(1),
    };
    let ticks = boundaries(s, l, &views, prm.at, prm.timing, beat.unwrap_or(0.0), rate);
    if let Some(i) = (1..n).find(|i| ticks[*i] <= ticks[i - 1]) {
        return Err(bad(ID, format!("Slices {} and {} land on the same tick; zoom or remap.", i, i + 1)));
    }
    let mut notes = Vec::with_capacity(n);
    for (i, w) in ticks.windows(2).enumerate() {
        let key = u8::try_from(usize::from(root) + i)
            .ok()
            .filter(|k| *k <= 127)
            .ok_or_else(|| bad(ID, format!("Slice {} would play key {}, past 127.", i + 1, usize::from(root) + i)))?;
        notes.push(Note {
            pitch: key,
            velocity: VELOCITY,
            release_velocity: 64,
            channel: CHANNEL,
            start: w[0] - prm.at,
            length: (w[1] - w[0]).max(1),
        });
    }
    let end_tick = notes.iter().map(|x| prm.at + x.start + x.length).max().unwrap_or(prm.at + 1);
    let (start, end) = (s.tempo.tick_to_samples(prm.at, s.sample_rate), s.tempo.tick_to_samples(end_tick, s.sample_rate));
    if end > MAX_POSITION || end <= start {
        return Err(bad(ID, "The clip would end past the end of the session's timeline."));
    }
    let range = Range::new(start, end);
    let overlap = tr.clips().iter().any(|c| c.range().overlaps(&range));
    Ok(Plan { root, ticks, notes, beat, range, overlap })
}

fn answer(e: &Engine, t: TrackId, prm: &Params, plan: &Plan) -> Value {
    let bbt = e.session().tempo.bar_beat_at_tick(prm.at);
    let n = plan.notes.len();
    json!({
        "track": t, "clip": null, "notes": n,
        "first_key": plan.root, "last_key": usize::from(plan.root) + n.saturating_sub(1),
        "velocity": VELOCITY, "channel": CHANNEL + 1,
        "at_tick": prm.at, "position": format!("{}|{}|{}", bbt.bar, bbt.beat, bbt.tick),
        "start": plan.range.start, "end": plan.range.end, "ticks": plan.ticks,
        "timing": if prm.timing == Timing::Fit { "fit" } else { "keep" }, "beat_length": plan.beat,
        "placement": "explicit", "takes": plan.overlap,
    })
}

/// Places the planned clip on the track's active playlist; an overlap becomes a new take by
/// ADR 0039's path (the active playlist duplicated, the covered material numbered, the clip
/// placed and settled). The settling makes the original playlist play again (ADR 0041: "keep the
/// original playlist active"), so its clips keep their ids and the old clips stay whole on the
/// duplicate, the alternate playlist.
fn place(s: &mut Session, t: TrackId, plan: &Plan) -> transit_model::ClipId {
    let previous = s.track(t).map_or(0, |tr| tr.active_playlist);
    if plan.overlap {
        // Duplicate Playlist (`track.playlist_duplicate`), inside this command's one step.
        let mut clips = s.track(t).map(|tr| tr.clips().to_vec()).unwrap_or_default();
        for c in &mut clips {
            c.id = s.new_clip_id();
        }
        if let Some(tr) = s.track_mut(t) {
            let name = format!("{}.{:02}", tr.name, tr.playlists.len() + 1);
            tr.playlists.push(transit_model::Playlist { name, clips });
            tr.active_playlist = tr.playlists.len() - 1;
        }
        crate::takes::number_covered(s, t, plan.range, previous);
    }
    let id = s.new_clip_id();
    let name = s.track(t).map(|tr| tr.name.clone()).unwrap_or_default();
    let mut sequence = Sequence { notes: plan.notes.clone(), ..Sequence::default() };
    sequence.sort();
    crate::edit::place_clip(s, t, Clip::midi(id, name, plan.range.start, plan.range.len(), sequence));
    if plan.overlap {
        crate::takes::settle_new_takes(s, t, previous, &[id]);
    }
    id
}

fn slice_to_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    let prm = params(p)?;
    let t = instrument_track(e, ID, p)?;
    let plan = plan(e, t, &prm)?;
    let mut out = answer(e, t, &prm, &plan);
    if prm.preview {
        out["overlap"] = json!(plan.overlap);
        return Ok(out);
    }
    let id = place(e.session_mut(), t, &plan);
    out["clip"] = json!(id);
    Ok(out)
}

#[cfg(test)]
#[path = "tests_slice_to_midi.rs"]
mod tests;

//! `mix.sampler_state` (ADR 0041 amendment 2026-11-08): the read-only query of a Sampler patch as
//! stored, with what playback uses beside it. This build reports the pad start note, each pad's
//! effective note and whether it is stored, and the loop's root in use and whether it is stored
//! (amendment 2026-11-09, item 6); every value is a MIDI number. The per-zone used values of the
//! 2026-11-08 amendment (`loop_used`, `crossfade_used`, `fades_used`) join it with the group
//! commands.

use super::sampler::instrument_track;
use super::sampler_patch::patch_of;
use super::*;
use crate::cmd;
use serde_json::json;
use transit_model::sampler_patch::{PADS, PatchV1};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(query
        "mix.sampler_state",
        "Sampler State",
        [],
        None,
        "{track?}: the Sampler patch as stored (`patch`), the normalized seed, `pad_start_note`, per pad {pad, note (effective: stored, else the start note + index), note_stored, zone, mute, solo, choke, name}, and the loop's root in use with root_stored (null without a loop); MIDI numbers throughout → {track, patch, seed, pad_start_note, pads, loop}",
        has_selection,
        state
    )]
}

/// Every pad of `p`: its effective note and whether that note is stored, with its settings.
pub(crate) fn pads_json(p: &PatchV1) -> Value {
    let pads = (0..PADS as u8).map(|i| {
        let e = p.pad(i);
        json!({
            "pad": i + 1, "note": p.pad_note(i), "note_stored": e.note.is_some(), "zone": e.zone.map(|z| z + 1),
            "mute": e.mute, "solo": e.solo, "choke": e.choke, "name": e.name,
        })
    });
    Value::Array(pads.collect())
}

fn state(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_state";
    let t = instrument_track(e, id, p)?;
    let patch = patch_of(e.session(), id, t)?;
    let stored = serde_json::to_value(&patch).map_err(|err| bad(id, err.to_string()))?;
    let lp = patch.slicing.as_ref().map(|l| json!({"root": patch.slice_root(), "root_stored": l.root.is_some()}));
    Ok(json!({
        "track": t, "patch": stored, "seed": patch.seed.max(1), "pad_start_note": patch.pad_start_note,
        "pads": pads_json(&patch), "loop": lp,
    }))
}

#[cfg(test)]
#[path = "tests_sampler_pad_start.rs"]
mod tests;

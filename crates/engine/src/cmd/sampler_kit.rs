//! Atomic folder-to-pad import. A worker stages the complete candidate using the same source
//! helper as pad drops. Only an admitted candidate reaches the live document, as one undo step.
//!
//! The caller states what happens to loaded pads (#465; the engine never guesses and never asks):
//! without `replace` the files fill the free pads ascending and a folder with more files than free
//! pads is refused. With `replace: true` the folder fills pads from A1 as a load into an empty kit
//! does: each pad it fills is first cleared as Clear pad clears one (its zone, and with it any
//! slice binding, its name, choke group, mute and solo; its trigger note stays) and the slot
//! controls of the zones it removes and fills are reset, so nothing of the old sample carries over;
//! pads past the folder's file count, the slice loop and its markers stay as they are. The replaced
//! pads' referenced sources nothing else names then leave the session, by kit recall's rule.

use super::sampler::instrument_track;
use super::sampler_kit_presets::prune;
use super::sampler_patch::{NewSource, add_audio, admit_patch, admit_source, patch_of, resolve_path, source_param, store};
use super::*;
use crate::cmd;
use crate::io::sampler_kit as files;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use transit_model::sampler_patch::{Ownership, PADS, PadEntry, PatchV1, ZONES, ZoneEntry};
use transit_model::{Session, SourceId};

const ID: &str = "mix.sampler_load_kit";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(@spec false, true, false;
        "mix.sampler_load_kit", "Load Kit Folder", [], None,
        "{track?, folder: path, preview?: false, replace?: false}: immediate WAV/AIFF/FLAC files, case-folded name order, referenced by path, one undo step; without replace they fill the free pads ascending (refused when more files than free pads); replace: true fills pads from A1, each filled pad cleared first (zone, slice binding, name, choke, mute, solo, zone controls; its note stays), later pads, the slice loop and its markers kept, replaced sources nothing names leave the session; preview probes headers only → {track, loaded, pads: [{pad, zone, file, frames, channels}], ignored, first_pad, last_pad, replace, replaced, replaced_slices}; refusal → {loaded: 0, skipped, needed, free, offender?, notice, files}",
        has_selection, load
    )]
}

struct Candidate {
    session: Arc<Session>,
    patch: PatchV1,
    result: Value,
}

struct Plan {
    paths: Vec<PathBuf>,
    pads: Vec<u8>,
    zones: Vec<u16>,
    ignored: Vec<String>,
}

/// What clearing the pads a replace fills took away: the loaded pads, and how many of them were
/// bound to a slice.
#[derive(Default)]
struct Cleared {
    pads: usize,
    slices: usize,
}

impl Plan {
    fn refusal(&self, reason: impl std::fmt::Display, offender: Option<&str>) -> Value {
        let needed = self.paths.len();
        let notice = format!("Load kit: {reason}; nothing was loaded (0 of {needed} files).");
        let files: Vec<Value> =
            self.paths.iter().map(|p| json!({"file": p.file_name().unwrap_or_default().to_string_lossy(), "reason": notice})).collect();
        let mut out = json!({"loaded": 0, "skipped": needed, "needed": needed, "free": self.pads.len(), "notice": notice, "files": files, "ignored": self.ignored});
        if let Some(name) = offender {
            out["offender"] = json!(name);
        }
        out
    }
}

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = instrument_track(e, ID, p)?;
    let given = str_param(p, "folder").ok_or_else(|| bad(ID, "`folder` is required"))?;
    let preview = match p.get("preview") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(ID, "`preview` must be true or false"))?,
    };
    let replace = match p.get("replace") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(ID, "`replace` must be true or false"))?,
    };
    let folder = resolve_path(e, ID, given)?;
    let candidate = stage(e, "Load kit", move |staged| prepare(staged, t, &folder, preview, replace))?;
    match candidate {
        Err(refusal) => Ok(refusal),
        Ok(candidate) => {
            if !preview {
                let old = if replace { referenced(&patch_of(e.session(), ID, t)?) } else { Vec::new() };
                // The staged session still has the old patch, so checked store compares the
                // real before/after layer counts. All source additions belong to this one step.
                *e.session_mut() = (*candidate.session).clone();
                store(e, ID, t, candidate.patch)?;
                // Kit recall's rule, in this same step: a replaced pad's source leaves the session
                // only when nothing else names it.
                prune(e, &old);
            }
            Ok(candidate.result)
        }
    }
}

/// The patch's referenced sources (the ones a replace may leave unnamed).
fn referenced(patch: &PatchV1) -> Vec<SourceId> {
    patch.sources.iter().filter(|r| r.ownership == Ownership::Referenced).map(|r| r.source).collect()
}

/// Clears pads `0..n` of `patch` as Clear pad clears a pad: the bound zone goes (with its slice
/// binding and its slot controls), and the name, choke group, mute and solo return to their
/// defaults; the trigger note stays. Sources no zone or loop uses any more drop out of the patch.
fn clear_pads(patch: &mut PatchV1, n: usize) -> Cleared {
    let mut cleared = Cleared::default();
    for pad in (0..PADS as u8).take(n) {
        let entry = patch.pad(pad);
        if let Some(z) = entry.zone {
            cleared.pads += 1;
            if patch.zone(z).is_some_and(|zone| zone.slice.is_some()) {
                cleared.slices += 1;
            }
            patch.zones.retain(|zone| zone.id != z);
            patch.reset_controls(z);
        }
        patch.set_pad(PadEntry { zone: None, name: None, choke: 0, mute: false, solo: false, ..entry });
    }
    patch.drop_unused_sources();
    cleared
}

/// Runs `work` on a worker thread over a staged engine holding a clone of `e`'s session, inside the
/// caller's checked file access roots, and joins it before returning: the engine waits for the
/// decode (ADR 0041 #450; Load kit and kit recall share this one place). No device, live engine or
/// import staging map crosses threads. `what` names the job in an error.
pub(super) fn stage<T: Send + 'static>(e: &Engine, what: &str, work: impl FnOnce(&mut Engine) -> T + Send + 'static) -> Result<T> {
    let session = e.session_arc();
    let roots = crate::file_access::current();
    let names = e.message_middle_c();
    let worker = std::thread::Builder::new()
        .name("transit-kit-import".into())
        .spawn(move || {
            crate::file_access::confine(roots.as_ref(), || {
                let mut staged = Engine::new((*session).clone());
                staged.set_message_middle_c(names);
                work(&mut staged)
            })
        })
        .map_err(|err| EngineError::Io(format!("{what}: could not start import: {err}")))?;
    worker.join().map_err(|_| EngineError::Io(format!("{what}: import worker failed; nothing was loaded")))
}

fn prepare(e: &mut Engine, t: TrackId, folder: &std::path::Path, preview: bool, replace: bool) -> std::result::Result<Candidate, Value> {
    let mut plan = Plan { paths: Vec::new(), pads: Vec::new(), zones: Vec::new(), ignored: Vec::new() };
    let listing = files::folder(folder).map_err(|err| plan.refusal(err, None))?;
    plan.paths = listing.files;
    plan.ignored = listing.ignored;
    let mut patch = patch_of(e.session(), ID, t).map_err(|err| plan.refusal(err.to_string(), None))?;
    let free = |p: &PatchV1| (0..PADS as u8).filter(|pad| p.pad(*pad).zone.is_none()).collect::<Vec<u8>>();
    plan.pads = free(&patch);
    let needed = plan.paths.len();
    if needed == 0 {
        return Err(plan.refusal(format!("No audio files in {}.", folder.display()), None));
    }
    let mut cleared = Cleared::default();
    if replace {
        if needed > PADS {
            let mut refusal = plan.refusal("not enough pads", None);
            refusal["notice"] = json!(format!("Load kit needs {needed} pads; a Sampler has {PADS}."));
            return Err(refusal);
        }
        // The pads the folder fills are cleared first, so they are the first free pads and the
        // load fills them from A1 as it would an empty kit.
        cleared = clear_pads(&mut patch, needed);
        plan.pads = free(&patch);
    }
    plan.zones = (0..ZONES as u16).filter(|zone| patch.zone(*zone).is_none()).collect();
    if replace {
        for zone in plan.zones.iter().take(needed) {
            patch.reset_controls(*zone);
        }
    }
    if needed > plan.pads.len() {
        let mut refusal = plan.refusal("not enough free pads", None);
        refusal["notice"] = json!(format!("Load kit needs {needed} pads; {} are free.", plan.pads.len()));
        return Err(refusal);
    }
    if needed > plan.zones.len() || patch.sources.len().saturating_add(needed) > ZONES {
        return Err(plan.refusal("a Sampler holds at most 256 zones and 256 sources", None));
    }
    if e.session().track(t).is_some_and(|tr| tr.freeze.is_some()) {
        return Err(plan.refusal(EngineError::TrackFrozen, None));
    }
    let mut rows = Vec::new();
    let mut total = 0u64;
    for (i, path) in plan.paths.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let refused = |reason| plan.refusal(reason, Some(&name));
        let info = files::probe(path).map_err(|err| refused(err.to_string()))?;
        let rate = info.sample_rate;
        let frames = info.frames;
        if frames == 0 {
            return Err(refused(format!("{name}: no audio")));
        }
        let bytes = admit_source(e.session(), &name, frames, u64::from(info.channels), rate).map_err(|reason| {
            if frames > super::sampler_patch::MAX_SOURCE_SECONDS.saturating_mul(u64::from(rate)) {
                refused(format!("{name} is {:.1} s long, more than the 30 s a Sampler holds in memory", frames as f64 / f64::from(rate.max(1))))
            } else {
                refused(reason)
            }
        })?;
        total = total.saturating_add(bytes);
        admit_patch(&patch, e.session(), total).map_err(refused)?;
        rows.push(json!({"pad": plan.pads[i] + 1, "zone": plan.zones[i] + 1, "file": name, "frames": frames, "channels": info.channels}));
    }
    // Build the same bindings for preview, using missing source handles: checked store accepts
    // missing media, and still validates all structure and the eight-voice admission rule.
    for (i, path) in plan.paths.iter().enumerate() {
        let name = rows[i]["file"].as_str().unwrap_or("Sample");
        let refused = |reason: String| plan.refusal(reason, Some(name));
        let sid = if preview {
            SourceId(e.session_mut().alloc())
        } else {
            files::regular(path).map_err(|err| refused(err.to_string()))?;
            match source_param(e, ID, &json!({"path": path, "copy": false}), &patch).map_err(|err| refused(err.to_string()))? {
                NewSource::Audio(audio, ownership) => add_audio(e, audio, ownership),
                NewSource::Session(sid, _) => sid,
            }
        };
        let frames = if preview { rows[i]["frames"].as_u64().unwrap_or(0) } else { e.session().pool.get(sid).map_or(0, |a| a.frames() as u64) };
        rows[i]["frames"] = json!(frames);
        let source = patch.source_index(sid, Ownership::Referenced) as u16;
        let pad = plan.pads[i];
        let mut zone = ZoneEntry::whole(plan.zones[i], source, frames);
        zone.pad_defaults(patch.pad_note(pad));
        zone.label = path.file_stem().and_then(|s| s.to_str()).map(str::to_string);
        patch.zones.push(zone);
        patch.set_pad(PadEntry { zone: Some(plan.zones[i]), ..patch.pad(pad) });
    }
    let session = e.session_arc();
    store(e, ID, t, patch.clone()).map_err(|err| plan.refusal(err, None))?;
    Ok(Candidate {
        session,
        patch,
        result: json!({
            "track": t,
            "loaded": needed,
            "pads": rows,
            "ignored": plan.ignored,
            "first_pad": plan.pads[0] + 1,
            "last_pad": plan.pads[needed - 1] + 1,
            "replace": replace,
            "replaced": cleared.pads,
            "replaced_slices": cleared.slices,
        }),
    })
}

#[cfg(test)]
#[path = "tests_sampler_kit.rs"]
mod tests;

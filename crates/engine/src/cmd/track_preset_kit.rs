//! A track preset's Sampler kit (ADR 0041, amendment 2026-11-11 "kit presets", item 7, #458): a
//! `.tpreset` carries the instrument's Sampler patch in the portable kit form, each sample by its
//! file (relative to the preset file when it is in or below the preset's folder), never by a
//! session source id; a load resolves, admits and decodes the samples as a kit recall does, once
//! per target track, on a staged copy of the session, and any refusal refuses the whole load.
//!
//! - A preset written before the portable form names samples by the saving session's ids: alpha,
//!   no migration. Each such entry becomes a new missing source named `source <id>` with no path,
//!   so it can never play a sample of the loading session, and the status line says so.
//! - A Sampler patch on an insert slot never plays (only the instrument slot runs the core path)
//!   and no command makes one; a preset neither writes one nor takes one in (the record's item 7
//!   covers the instrument; this is the narrowest rule that keeps a session id out of both).

use super::sampler_kit::stage;
use super::sampler_kit_presets::{Resolved, found_line, missing_lines, reason, resolve, session_folder};
use super::sampler_patch::store_with;
use super::*;
use serde_json::json;
use std::path::Path;
use transit_model::sampler_portable::{self as portable, Entry};
use transit_model::{Insert, Mixer, Session, TrackKind};

/// The undo label of the load (and the staging worker's name for it).
const LABEL: &str = "Load Track Preset";

/// The track's instrument as a preset writes it: its Sampler patch in the portable form, samples
/// relative to `preset_folder` (absolute) when in or below it. `Err`: a sample with no file of its
/// own (the session's unsaved audio), as a kit save refuses it.
pub(super) fn instrument_value(e: &Engine, id: &str, ins: Option<&Insert>, preset_folder: &Path) -> Result<Value> {
    let mut v = serde_json::to_value(ins).map_err(|err| EngineError::Io(err.to_string()))?;
    if let Some(patch) = ins.and_then(|i| i.sampler_patch.as_ref()) {
        let saved = portable::to_portable(patch, e.session(), session_folder(e).as_deref(), preset_folder).map_err(|r| bad(id, r.text()))?;
        v["sampler_patch"] = saved.kit;
    }
    Ok(v)
}

/// The mixer as a preset writes it: no insert carries a Sampler patch.
pub(super) fn mixer_value(mixer: &Mixer) -> Result<Value> {
    let mut v = serde_json::to_value(mixer).map_err(|err| EngineError::Io(err.to_string()))?;
    drop_insert_patches(&mut v);
    Ok(v)
}

/// Removes every insert's `sampler_patch` from a mixer's JSON.
pub(super) fn drop_insert_patches(mixer: &mut Value) {
    let inserts = mixer.get_mut("inserts").and_then(Value::as_array_mut).into_iter().flatten();
    for ins in inserts.filter_map(Value::as_object_mut) {
        ins.remove("sampler_patch");
    }
}

/// What a load puts on one target track.
pub(super) struct Target {
    pub track: TrackId,
    pub mixer: Mixer,
    /// Assigned when `Some` (an instrument track and a preset with an instrument).
    pub instrument: Option<Insert>,
    /// The instrument is a Sampler whose patch comes from the preset's portable kit.
    pub kit: bool,
}

/// The preset's routing-independent settings on track `t`: the track keeps its own input, output,
/// VCA, arm state and (when the channel count differs) pan.
pub(super) fn apply(s: &mut Session, t: &Target) {
    let Some(tr) = s.track_mut(t.track) else { return };
    let keep = (tr.mixer.input.clone(), tr.mixer.output.clone(), tr.mixer.vca, tr.mixer.record_arm, tr.mixer.pan.clone());
    tr.mixer = t.mixer.clone();
    tr.mixer.input = keep.0;
    tr.mixer.output = keep.1;
    tr.mixer.vca = keep.2;
    tr.mixer.record_arm = keep.3;
    if tr.mixer.pan.len() != keep.4.len() {
        tr.mixer.pan = keep.4;
    }
    if tr.kind == TrackKind::Instrument
        && let Some(ins) = &t.instrument
    {
        tr.instrument = Some(ins.clone());
    }
}

/// The preset's kit, read: its entries, or why the whole load is refused.
pub(super) fn read_kit(kit: Value) -> std::result::Result<(Value, Vec<Entry>), String> {
    portable::from_portable(kit).map_err(|r| r.text())
}

/// Applies `targets` on a staged copy of the session, resolving the kit (`kit`, `entries`, read
/// from the preset file `file`) for each target that takes it, and puts the staged session in place
/// in one step. Returns the first kit target's patch and missing samples. `Err`: the refusal's
/// reason; nothing changed.
pub(super) fn load_with_kits(
    e: &mut Engine,
    targets: Vec<Target>,
    file: &Path,
    kit: Value,
    entries: Vec<Entry>,
) -> std::result::Result<Option<Resolved>, String> {
    let file = file.to_path_buf();
    let staged = stage(e, LABEL, move |st| -> std::result::Result<(std::sync::Arc<Session>, Option<Resolved>), String> {
        let mut first = None;
        for t in &targets {
            apply(st.session_mut(), t);
            if t.kit {
                let (patch, missing) = resolve(st, &file, kit.clone(), &entries)?;
                store_with(st, "track.load_preset", t.track, patch.clone(), true).map_err(reason)?;
                first.get_or_insert((patch, missing));
            }
        }
        Ok((st.session_arc(), first))
    });
    let (session, first) = match staged {
        Ok(Ok(x)) => x,
        Ok(Err(why)) => return Err(why),
        Err(err) => return Err(reason(err)),
    };
    *e.session_mut() = (*session).clone();
    Ok(first)
}

/// The status line of a load whose kit took samples in the missing state, `None` when every
/// sample was found. `by_id`: the preset named samples by session id (written before the portable
/// form).
pub(super) fn kit_notice(stem: &str, resolved: &Resolved, by_id: bool) -> Option<String> {
    let what = format!("track preset \"{stem}\"");
    if by_id {
        return Some(format!("{what}: its Sampler samples were saved by session id and cannot be found: load them again."));
    }
    let (patch, missing) = resolved;
    if missing.is_empty() {
        return None;
    }
    let mut parts = vec![found_line(&what, patch, missing)];
    parts.extend(missing_lines(missing));
    Some(parts.join(" "))
}

/// The result's extra field for a notice.
pub(super) fn with_notice(mut out: Value, notice: Option<&str>) -> Value {
    if let Some(n) = notice {
        out["notice"] = json!(n);
    }
    out
}

#[cfg(test)]
#[path = "tests_track_preset_kit.rs"]
mod tests;

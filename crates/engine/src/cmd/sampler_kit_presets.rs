//! Sampler kit presets (ADR 0041, amendment 2026-11-11 "kit presets", #467): save the track's whole
//! Sampler kit as a `.tkit` file and recall one, each sample referenced by its file path, no audio
//! copied. The portable form and the path rules are `transit_model::sampler_portable`; the files
//! go through `crate::io::kit_presets`.
//!
//! - **Save** (`mix.sampler_kit_save`, not undoable, not journaled): the whole patch through
//!   `to_portable`, paths relative to the kit file's folder when the sample is in or below it.
//!   Refused for an opaque patch and for a sample with no file of its own; a sample missing at save
//!   time is written with its stored path and listed.
//! - **Recall** (`mix.sampler_kit_load`, one undo step, not journaled): the caller states the
//!   user's choice: a Sampler with any zone or a loop is replaced only with `replace: true`, else
//!   the command answers `replace_needed` and changes nothing (the engine never asks and never
//!   guesses UI state). The file is read and checked before any audio is touched; a worker then
//!   stages the whole candidate over a clone of the session (the staging Load kit uses), resolves
//!   each sample against the kit file, admits it from its header (or its bytes) for the new kit's
//!   128 MiB, decodes it at its native rate and compares it with the kit's record. A sample not
//!   found, changed or unreadable takes the missing state and the rest of the kit loads. The
//!   candidate passes the checked store with the eight-voices rule judged against an empty patch;
//!   any refusal answers data with a notice and changes nothing. After the new patch is in place,
//!   each referenced source of the replaced kit that nothing else names leaves the session.

use super::sampler::instrument_track;
use super::sampler_kit::stage;
use super::sampler_patch::{add_audio, admit_patch, admit_source, patch_of, resolve_path, store_with};
use super::*;
use crate::cmd;
use crate::io::kit_presets as files;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transit_model::pad_notes::StartChange;
use transit_model::sampler_patch::{Ownership, PatchState, PatchV1};
use transit_model::sampler_portable::{self as portable, Entry, FileRef};
use transit_model::{Session, Source, SourceId};

const SAVE: &str = "mix.sampler_kit_save";
const LOAD: &str = "mix.sampler_kit_load";
const SAMPLER: &str = "sampler";
/// The recall's undo label (and the staging worker's name for it).
const LABEL: &str = "Load Sampler Kit";
/// Files a notice names before "and N more".
const NAMED: usize = 16;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(@spec false, false, false;
            "mix.sampler_kit_save", "Save Sampler Kit", [], None,
            "{track?, path}: the track's whole Sampler kit as a kit file ({format: transit-sampler-kit, version: 1, kit}); `.tkit` appended unless the path ends in it (never replacing another extension); each sample by its file: relative with / when in or below the kit file's folder, else absolute; no audio copied; not undoable, not journaled; refused for an opaque patch, a sample with no file yet (save the session first), or an existing file this version does not read → {track, path, pads, zones, sources, relative, absolute, missing: [{zone, pad?, path}], notes}",
            has_selection, save
        ),
        cmd!(@spec false, true, false;
            "mix.sampler_kit_load", "Load Sampler Kit", [], None,
            "{track?, path, replace?: false}: recalls a kit file onto the track's Sampler as one undo step: the whole patch is replaced and the slot's preset label becomes the file's stem; samples resolve against the kit file (relative from its folder); a Sampler with any zone or a loop needs replace: true, else → {loaded: 0, replace_needed: true, pads, zones} and nothing changes; a sample not found, changed since the save or unreadable loads in the missing state and the rest loads; the replaced kit's referenced samples nothing else names leave the session → {track, kit, loaded, pads, zones, sources, missing: [{zone, pad?, path}], notice}; refusal (an opaque or frozen target, a kit this version does not read, the 128 MiB budget, eight voices per note) → {loaded: 0, notice}, nothing changed",
            has_selection, load
        ),
    ]
}

/// An error's message without the command prefix, for a notice.
fn reason(err: EngineError) -> String {
    match err {
        EngineError::BadParams(_, m) | EngineError::Io(m) => m,
        other => other.to_string(),
    }
}

/// The folder the session's relative source paths point into.
fn session_folder(e: &Engine) -> Option<PathBuf> {
    e.session().folder.clone().or_else(|| e.path.as_deref().and_then(|x| Path::new(x).parent().map(Path::to_path_buf)))
}

/// A path param made absolute (a relative one from the session's folder), without file access.
fn absolute(e: &Engine, id: &str, given: &str) -> Result<PathBuf> {
    let p = resolve_path(e, id, given)?;
    Ok(std::path::absolute(&p).unwrap_or(p))
}

fn path_param<'a>(id: &str, p: &'a Value) -> Result<&'a str> {
    str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(id, "`path` required"))
}

/// The display name of a path: its last component.
fn file_label(path: &str) -> String {
    path.rsplit(['/', '\\']).find(|c| !c.is_empty()).unwrap_or(path).to_string()
}

/// The `missing` rows for source indices `missing` of `patch`: one per zone using the source (with
/// its pad), or `{loop: true}` for a source only the slice loop uses.
fn missing_rows(patch: &PatchV1, missing: &[(usize, String)]) -> Vec<Value> {
    let mut rows = Vec::new();
    for (index, path) in missing {
        let mut zones: Vec<u16> = patch.zones.iter().filter(|z| usize::from(z.source) == *index).map(|z| z.id).collect();
        zones.sort_unstable();
        for z in &zones {
            let mut row = json!({"zone": z + 1, "path": path});
            if let Some(pad) = patch.pad_of(*z) {
                row["pad"] = json!(pad + 1);
            }
            rows.push(row);
        }
        if zones.is_empty() {
            rows.push(json!({"loop": true, "path": path}));
        }
    }
    rows
}

fn bound_pads(patch: &PatchV1) -> Vec<u8> {
    patch.pads.iter().filter(|q| q.zone.is_some()).map(|q| q.index + 1).collect()
}

fn save(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = instrument_track(e, SAVE, p)?;
    let file = portable::kit_file_path(&absolute(e, SAVE, path_param(SAVE, p)?)?);
    let s = e.session();
    let ins = s.track(t).and_then(|tr| tr.instrument.as_ref()).filter(|i| i.plugin == SAMPLER);
    let patch = ins
        .and_then(|i| i.sampler_patch.as_ref())
        .ok_or_else(|| bad(SAVE, "the track's Sampler has no patch: make one with mix.sampler_new_patch"))?;
    let ready = patch.ready().ok_or_else(|| bad(SAVE, "this version cannot save the track's Sampler kit (it is kept as saved)"))?;
    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    // Refused before the temporary file is created: nothing is written.
    let saved = portable::to_portable(patch, s, session_folder(e).as_deref(), &folder).map_err(|r| bad(SAVE, r.text()))?;
    let missing: Vec<(usize, String)> = saved.files.iter().filter(|f| f.missing).map(|f| (f.index, f.path.clone())).collect();
    let mut notes = Vec::new();
    for f in &saved.files {
        let name = file_label(&f.path);
        if f.managed {
            notes.push(format!("\"{name}\" is in this session's Audio Files folder: the kit depends on that session folder."));
        }
        if f.other_share {
            notes.push(format!("\"{name}\" is on another network share than the kit file: it will be missing when the kit is recalled."));
        }
    }
    let relative = saved.files.iter().filter(|f| f.relative).count();
    let out = json!({
        "track": t,
        "path": file.to_string_lossy(),
        "pads": bound_pads(ready).len(),
        "zones": ready.zones.len(),
        "sources": saved.files.len(),
        "relative": relative,
        "absolute": saved.files.len() - relative,
        "missing": missing_rows(ready, &missing),
        "notes": notes,
    });
    files::write(&file, &portable::file(saved.kit)).map_err(EngineError::Io)?;
    Ok(out)
}

/// Why a sample takes the missing state.
enum Why {
    NotFound,
    /// Found, but its rate or length is not the kit's record (`expects`).
    Changed {
        rate: u32,
        frames: u64,
        expects: (u32, u64),
    },
    Unreadable(String),
}

/// What recall does with one sample.
enum Plan {
    /// Decode the file; `Some(bytes)`: already admitted from its header.
    Found(PathBuf, Option<u64>),
    Reuse(SourceId),
    Missing(String, Why),
}

/// A staged recall.
struct Recalled {
    session: Arc<Session>,
    patch: PatchV1,
    missing: Vec<(usize, String, Why)>,
}

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = instrument_track(e, LOAD, p)?;
    let given = path_param(LOAD, p)?;
    let replace = match p.get("replace") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(LOAD, "`replace` must be true or false"))?,
    };
    let file = absolute(e, LOAD, given)?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let refuse = |why: String| json!({"track": t, "kit": stem, "loaded": 0, "notice": format!("Kit \"{stem}\": {why}; nothing was loaded.")});
    if e.session().track(t).is_some_and(|tr| tr.freeze.is_some()) {
        return Ok(refuse(EngineError::TrackFrozen.to_string()));
    }
    let current = match patch_of(e.session(), LOAD, t) {
        Ok(p) => p,
        Err(err) => return Ok(refuse(reason(err))),
    };
    if (!current.zones.is_empty() || current.slicing.is_some()) && !replace {
        let mut zones: Vec<u16> = current.zones.iter().map(|z| z.id + 1).collect();
        zones.sort_unstable();
        return Ok(json!({"track": t, "kit": stem, "loaded": 0, "replace_needed": true, "pads": bound_pads(&current), "zones": zones}));
    }
    let v = match files::read(&file) {
        Ok(v) => v,
        Err(m) => return Ok(refuse(m)),
    };
    let (kit, entries) = match portable::from_portable(v.get("kit").cloned().unwrap_or(Value::Null)) {
        Ok(x) => x,
        Err(r) => return Ok(refuse(r.text())),
    };
    let mut refs = Vec::new();
    for (i, entry) in entries.into_iter().enumerate() {
        match entry {
            Entry::File(f) => refs.push(f),
            Entry::Session(_) => return Ok(refuse(format!("not a valid Sampler kit (source {} names a session source, not a file)", i + 1))),
        }
    }
    let staged_file = file.clone();
    // A worker that cannot start or fails is a refusal like any other (item 4): data, nothing
    // changed.
    let candidate = match stage(e, LABEL, move |staged| prepare(staged, t, &staged_file, kit, &refs)) {
        Ok(Ok(c)) => c,
        Ok(Err(why)) => return Ok(refuse(why)),
        Err(err) => {
            let why = reason(err);
            let why = why.strip_prefix(&format!("{LABEL}: ")).unwrap_or(&why);
            return Ok(refuse(why.strip_suffix("; nothing was loaded").unwrap_or(why).to_string()));
        }
    };
    // Admitted: the staged session (with every added source) replaces the live one in this one
    // step, and the checked store writes the kit whole.
    *e.session_mut() = (*candidate.session).clone();
    store_with(e, LOAD, t, candidate.patch.clone(), true)?;
    if let Some(ins) = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()) {
        ins.preset.clone_from(&stem);
    }
    let old: Vec<SourceId> = current.sources.iter().filter(|r| r.ownership == Ownership::Referenced).map(|r| r.source).collect();
    prune(e, &old);
    let new = &candidate.patch;
    let rows: Vec<(usize, String)> = candidate.missing.iter().map(|(i, path, _)| (*i, path.clone())).collect();
    let notice = notice(&stem, &current, new, &candidate.missing, e.message_middle_c());
    Ok(json!({
        "track": t,
        "kit": stem,
        "loaded": new.zones.len(),
        "pads": bound_pads(new).len(),
        "zones": new.zones.len(),
        "sources": new.sources.len(),
        "missing": missing_rows(new, &rows),
        "notice": notice,
    }))
}

/// "a, b, c and N more" of the first [`NAMED`] items.
fn named(items: &[String]) -> String {
    let shown = items.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", ");
    match items.len().saturating_sub(NAMED) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

fn notice(stem: &str, old: &PatchV1, new: &PatchV1, missing: &[(usize, String, Why)], names: transit_midi::MiddleC) -> String {
    let mut parts = Vec::new();
    let not_found: Vec<String> =
        missing.iter().filter(|(_, _, w)| matches!(w, Why::NotFound)).map(|(_, path, _)| format!("{} ({path})", file_label(path))).collect();
    match not_found.len() {
        0 => parts.push(format!("Kit \"{stem}\" loaded: {} pads, {} zones.", bound_pads(new).len(), new.zones.len())),
        1 => parts.push(format!("Kit \"{stem}\": 1 sample was not found and plays silence: {}.", named(&not_found))),
        n => parts.push(format!("Kit \"{stem}\": {n} samples were not found and play silence: {}.", named(&not_found))),
    }
    let mut other = Vec::new();
    for (_, path, why) in missing {
        let name = file_label(path);
        match why {
            Why::Changed { rate, frames, expects: (r, n) } => other.push(format!(
                "{name} changed since the kit was saved: {rate} Hz, {frames} frames; the kit expects {r} Hz, {n} frames: it plays silence."
            )),
            Why::Unreadable(err) => other.push(format!("{name} could not be read ({err}): it plays silence.")),
            Why::NotFound => {}
        }
    }
    parts.extend(other.iter().take(NAMED).cloned());
    if other.len() > NAMED {
        parts.push(format!("And {} more files play silence.", other.len() - NAMED));
    }
    if new.pad_start_note != old.pad_start_note {
        parts.push(StartChange { from: old.pad_start_note, to: new.pad_start_note }.notice(names));
    }
    parts.join(" ")
}

/// The pool bytes of a source's decoded audio.
fn pool_bytes(s: &Session, sid: SourceId) -> u64 {
    s.pool.get(sid).map_or(0, |a| (a.frames() as u64).saturating_mul(a.buffer.num_channels() as u64).saturating_mul(4))
}

/// A referenced Sampler source of the session at the resolved path `path` whose audio is present
/// at its stored rate and matches the kit's record: reused rather than decoded again.
fn reusable(s: &Session, path: &str, f: &FileRef) -> Option<SourceId> {
    let patches = s.tracks.iter().filter_map(|tr| tr.instrument.as_ref()).filter_map(|i| i.sampler_patch.as_ref());
    patches.flat_map(|p| p.referenced()).find(|sid| {
        let (Some(src), Some(a)) = (s.source(*sid), s.pool.get(*sid)) else { return false };
        portable::same_path(&src.path, path)
            && !a.is_disk()
            && a.buffer.sample_rate == src.sample_rate
            && src.sample_rate == f.sample_rate
            && src.frames == f.frames
            && a.frames() as u64 == f.frames
    })
}

/// A session source in the missing state: the path it looked for, the kit's recorded facts, no
/// audio.
fn add_missing(e: &mut Engine, f: &FileRef, path: &str) -> SourceId {
    let s = e.session_mut();
    let id = SourceId(s.alloc());
    s.sources.push(Source {
        id,
        name: f.name.clone(),
        path: path.to_string(),
        channels: f.channels,
        frames: f.frames,
        sample_rate: f.sample_rate,
        format: f.format,
        time_reference: 0,
        unsaved: false,
        disk: false,
    });
    id
}

/// The path a missing sample is recorded with: the resolved one; else the kit's own when it is
/// absolute; else joined to the kit's folder as text (never a bare relative path, which a session
/// would read against its own folder).
fn missing_path(found: Option<&Path>, stored: &str, folder: &str) -> String {
    match found {
        Some(p) => portable::strip_verbatim(&p.to_string_lossy()),
        None if portable::is_absolute(stored) => portable::strip_verbatim(stored),
        None => portable::join(folder, stored).unwrap_or_else(|| stored.to_string()),
    }
}

/// Reads, admits and decodes one found sample. The outer error refuses the whole kit (admission);
/// the inner one sends the sample to the missing state.
fn decode(
    e: &Engine,
    path: &Path,
    f: &FileRef,
    counted: Option<u64>,
    total: &mut u64,
) -> std::result::Result<std::result::Result<crate::io::PreparedAudio, Why>, String> {
    let text = portable::strip_verbatim(&path.to_string_lossy());
    let name = file_label(&text);
    let bytes = match crate::io::read_file(path) {
        Ok(b) => b,
        Err(err) => return Ok(Err(Why::Unreadable(reason(err)))),
    };
    let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let info = match transit_audio_io::probe(&bytes, ext.as_deref()) {
        Ok(info) => info,
        Err(err) => return Ok(Err(Why::Unreadable(err.to_string()))),
    };
    if info.sample_rate != f.sample_rate || info.frames != f.frames {
        return Ok(Err(Why::Changed { rate: info.sample_rate, frames: info.frames, expects: (f.sample_rate, f.frames) }));
    }
    if counted.is_none() {
        // A file its header does not size: admitted from its bytes, still before decoding.
        *total = total.saturating_add(admit_source(e.session(), &name, info.frames, u64::from(info.channels), info.sample_rate)?);
        admit_patch(&PatchV1::default(), e.session(), *total)?;
    }
    let prepared = match crate::io::prepare_audio_bytes(&name, &bytes, Some(&text), crate::io::ImportRate::Native, &mut |_| true) {
        Ok(p) => p,
        Err(err) => return Ok(Err(Why::Unreadable(reason(err)))),
    };
    if prepared.rate() != f.sample_rate || prepared.frames() as u64 != f.frames {
        return Ok(Err(Why::Changed { rate: prepared.rate(), frames: prepared.frames() as u64, expects: (f.sample_rate, f.frames) }));
    }
    Ok(Ok(prepared))
}

/// Stages the recall on `e` (a clone of the session): every sample resolved, admitted and decoded
/// or sent to the missing state, then the checked store. `Err`: the refusal's reason.
fn prepare(e: &mut Engine, t: TrackId, file: &Path, kit: Value, refs: &[FileRef]) -> std::result::Result<Recalled, String> {
    #[cfg(test)]
    evidence::fail_worker_for(file);
    let folder = file.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let empty = PatchV1::default();
    let mut total = 0u64;
    let mut reused: Vec<SourceId> = Vec::new();
    let mut plans = Vec::with_capacity(refs.len());
    // Admission from headers first, so an over-budget kit is refused before anything is decoded.
    for f in refs {
        let plan = match files::sample(&f.path, file) {
            Err((found, _)) => Plan::Missing(missing_path(found.as_deref(), &f.path, &folder), Why::NotFound),
            Ok(path) => {
                let text = portable::strip_verbatim(&path.to_string_lossy());
                if let Some(sid) = reusable(e.session(), &text, f) {
                    if !reused.contains(&sid) {
                        reused.push(sid);
                        total = total.saturating_add(pool_bytes(e.session(), sid));
                        admit_patch(&empty, e.session(), total)?;
                    }
                    Plan::Reuse(sid)
                } else if let Some(info) = crate::io::probe_header(&path) {
                    if info.sample_rate != f.sample_rate || info.frames != f.frames {
                        Plan::Missing(text, Why::Changed { rate: info.sample_rate, frames: info.frames, expects: (f.sample_rate, f.frames) })
                    } else {
                        let bytes = admit_source(e.session(), &file_label(&text), info.frames, u64::from(info.channels), info.sample_rate)?;
                        total = total.saturating_add(bytes);
                        admit_patch(&empty, e.session(), total)?;
                        Plan::Found(path, Some(bytes))
                    }
                } else {
                    Plan::Found(path, None)
                }
            }
        };
        plans.push(plan);
    }
    let mut ids = Vec::with_capacity(refs.len());
    let mut missing = Vec::new();
    for (i, (plan, f)) in plans.into_iter().zip(refs).enumerate() {
        let sid = match plan {
            Plan::Reuse(sid) => sid,
            Plan::Missing(path, why) => {
                let sid = add_missing(e, f, &path);
                missing.push((i, path, why));
                sid
            }
            Plan::Found(path, counted) => match decode(e, &path, f, counted, &mut total)? {
                Ok(prepared) => add_audio(e, prepared, Ownership::Referenced),
                Err(why) => {
                    let text = portable::strip_verbatim(&path.to_string_lossy());
                    let sid = add_missing(e, f, &text);
                    missing.push((i, text, why));
                    sid
                }
            },
        };
        ids.push(sid);
    }
    let patch = match portable::with_sources(kit, &ids).state {
        PatchState::Ready(p) => *p,
        PatchState::Invalid { reason, .. } => return Err(format!("not a valid Sampler kit ({reason})")),
        PatchState::Unknown { schema, .. } => return Err(format!("saved by a newer transit (Sampler kit schema {schema})")),
    };
    // The staged session before the store: the live engine stores the same patch on it.
    let session = e.session_arc();
    store_with(e, LOAD, t, patch.clone(), true).map_err(reason)?;
    Ok(Recalled { session, patch, missing })
}

/// Whether anything in the session or on the clipboard names source `sid`: a clip in any playlist,
/// a freeze, or any Sampler patch (opaque ones by their raw sources).
fn named_elsewhere(e: &Engine, sid: SourceId) -> bool {
    let s = e.session();
    let mut patches =
        s.tracks.iter().flat_map(|tr| tr.instrument.iter().chain(tr.mixer.inserts.iter().flatten())).filter_map(|i| i.sampler_patch.as_ref());
    s.clips_using(sid).next().is_some()
        || s.tracks.iter().any(|tr| tr.freeze.as_ref().is_some_and(|f| f.source == sid))
        || patches.any(|p| p.media().contains(&sid))
        || e.clipboard.tracks.iter().flatten().any(|c| c.source() == Some(sid))
}

/// Removes each of `old` (the replaced kit's referenced sources) that nothing names any more from
/// the session's sources, its pool and the source selection. The file is never touched. Load Kit
/// Folder's replace (#465) prunes the replaced pads' sources through this same rule.
pub(super) fn prune(e: &mut Engine, old: &[SourceId]) {
    let gone: Vec<SourceId> = old.iter().copied().filter(|sid| !named_elsewhere(e, *sid)).collect();
    if gone.is_empty() {
        return;
    }
    let s = e.session_mut();
    s.sources.retain(|x| !gone.contains(&x.id));
    for sid in &gone {
        s.pool.remove(*sid);
    }
    s.edit.selected_sources.retain(|x| !gone.contains(x));
}

#[cfg(test)]
#[path = "tests_sampler_kit_presets_evidence.rs"]
mod evidence;
#[cfg(test)]
#[path = "tests_sampler_kit_presets.rs"]
mod tests;

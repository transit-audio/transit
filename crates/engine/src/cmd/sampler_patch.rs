//! The built-in Sampler's patch commands (ADR 0041, #269 job C2): make a patch, give a zone its
//! source, region and playback, clear it, bind pads and set their notes and choke groups, set the
//! seed and the slot controls. Each command is one atomic, undoable edit, refused whole before the
//! document changes when anything is out of range (a refused command leaves the document as it
//! was). The plan compiler publishes what changed to the playing instance (`transit_mix`).
//!
//! - **Zones and pads** are numbered as the panel shows them: `zone` 1..=256 (the slot of the
//!   `zoneNNN_*` controls), `pad` 1..=64 (A1 is 1, bank B starts at 17).
//! - **Pad notes** (ADR 0041 amendment 2026-11-09): a new patch has pad start note 36; a pad
//!   without a stored note plays the start note plus its index, a stored note is absolute.
//!   `mix.sampler_pad_start` moves the defaulted pads and the roots of their zones in one step.
//!   Refusals and notices that name a note use the engine's message Middle C (ADR 0051); every
//!   parameter and result is a MIDI number.
//! - **Sources.** `source` names a session source; `path` decodes a file (a relative path is taken
//!   from the session's folder; every path goes through the checked file access); `import` takes a
//!   prepared asynchronous import (`file.prepare_import`). A file is referenced where it is by
//!   default (ADR 0041's save model: its path is kept and never rewritten by a save); `copy: true`
//!   makes it the session's own audio, written into `Audio Files/` like any import. Dropped bytes
//!   have no durable path and are always the session's own ([`load_zone_bytes`]). Clearing a zone
//!   never deletes a file.
//! - **Admission** (ADR 0041 "Capacities and admission"): a source holds at most 30 seconds and
//!   32 MiB of decoded audio, and no more than ADR 0035's streaming threshold; a patch holds at most
//!   128 MiB of distinct decoded audio (a sample reused by several zones counts once). Each bound
//!   is checked with checked arithmetic from the file's header (or the source's size) before the
//!   audio is decoded or the patch changes; an edit over a bound is refused with a message and the
//!   old patch keeps playing. Longer samples stream from disk in #271. A one-sample Sampler's
//!   `Insert::sample` is never checked against these bounds.

use super::sampler::instrument_track;
use super::*;
use crate::cmd;
use serde_json::{Map, json};
use std::path::{Path, PathBuf};
use transit_dsp::plugins::sampler as dsp_sampler;
use transit_model::pad_notes::PadRefusal;
use transit_model::sampler_patch::{Ownership, PadEntry, PatchV1, ZoneEntry};
use transit_model::{Insert, SamplerPatch, Session, SourceId, TrackId};

const SAMPLER: &str = "sampler";

/// Most seconds of audio one source holds in memory.
pub const MAX_SOURCE_SECONDS: u64 = 30;
/// Most bytes of decoded audio (32-bit float) one source holds in memory.
pub const MAX_SOURCE_BYTES: u64 = 32 << 20;
/// Most bytes of distinct decoded audio one patch holds in memory.
pub const MAX_PATCH_BYTES: u64 = 128 << 20;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.sampler_new_patch",
            "New Sampler Patch",
            [],
            None,
            "{track?, seed?: 1}: the track's instrument becomes a Sampler with an empty patch (zones, pads, slicing); its one-sample `sample` is left unset → {track}",
            has_selection,
            new_patch
        ),
        cmd!(@spec false, true, false;
            "mix.sampler_zone_source",
            "Set Sampler Zone Source",
            [],
            None,
            "{track?, zone: 1-256, source: id | path | import: id, copy?: false, pad?: 1-64, region?: {start, end, loop_mode: off|forward|ping_pong, loop_start, loop_end}, playback?: {root, transpose, cents, reverse, mode: one_shot|gate, key_tracking, attack_ms, decay_ms, sustain, release_ms, declick_ms, velocity_sensitivity, keys: [lo, hi], velocities: [lo, hi], label}} → {track, zone, source, frames, channels}",
            has_selection,
            zone_source
        ),
        cmd!(
            "mix.sampler_zone_edit",
            "Edit Sampler Zone",
            [],
            None,
            "{track?, zone: 1-256, region?: {..}, playback?: {..}} (as mix.sampler_zone_source) → {track, zone}",
            has_selection,
            zone_edit
        ),
        cmd!(
            "mix.sampler_zone_clear",
            "Clear Sampler Zone",
            [],
            None,
            "{track?, zone: 1-256}: the zone is emptied (its voices fade; its pad is unbound; no file is deleted) → {track, zone}",
            has_selection,
            zone_clear
        ),
        cmd!("mix.sampler_pad_bind", "Bind Sampler Pad", [], None, "{track?, pad: 1-64, zone: 1-256} → {track, pad, zone}", has_selection, pad_bind),
        cmd!(
            "mix.sampler_pad_unbind",
            "Unbind Sampler Pad",
            [],
            None,
            "{track?, pad: 1-64}: the zone stays, unbound → {track, pad}",
            has_selection,
            pad_unbind
        ),
        cmd!(
            "mix.sampler_pad_set",
            "Set Sampler Pad",
            [],
            None,
            "{track?, pad: 1-64, note?: 0-127 (unique among pads; stored and absolute) | null (back to the default, the pad start note + index), choke?: 0 (Off)-16, mute?, solo?, name?} → {track, pad, note (the effective note)}",
            has_selection,
            pad_set
        ),
        cmd!(
            "mix.sampler_pad_start",
            "Set Sampler Pad Start Note",
            [],
            None,
            "{track?, note: 0-64 (MIDI number)}: the pad start note; pads without a stored note play note + index and move with it, stored notes stay; zones bound to moved pads keep their pitch (their roots move too); clips are not changed → {track, note, from, semitones, notice}",
            has_selection,
            pad_start
        ),
        cmd!("mix.sampler_seed", "Set Sampler Seed", [], None, "{track?, seed: 0-4294967295 (0 acts as 1)} → {track, seed}", has_selection, seed),
        cmd!(
            "mix.sampler_control",
            "Set Sampler Control",
            [],
            None,
            "{track?, id: zoneNNN_<field> (zone001_level, zone001_pan, the shaper and mod ids), value} → {track, id, value} (clamped)",
            has_selection,
            control
        ),
    ]
}

/// The ready patch of track `t`'s Sampler, cloned for an edit.
pub(super) fn patch_of(s: &Session, id: &str, t: TrackId) -> Result<PatchV1> {
    let ins = s.track(t).and_then(|tr| tr.instrument.as_ref()).filter(|i| i.plugin == SAMPLER);
    let patch =
        ins.and_then(|i| i.sampler_patch.as_ref()).ok_or_else(|| bad(id, "the track's Sampler has no patch: make one with mix.sampler_new_patch"))?;
    patch.ready().cloned().ok_or_else(|| bad(id, "this version cannot edit the track's Sampler patch (it is kept as saved)"))
}

/// Checks `p` as the instance will, then stores it on track `t`.
pub(super) fn store(e: &mut Engine, id: &str, t: TrackId, p: PatchV1) -> Result<()> {
    transit_mix::check_sampler_patch(&p, e.session()).map_err(|m| bad(id, m))?;
    let ins = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(id, "no track"))?;
    ins.sampler_patch = Some(SamplerPatch { state: transit_model::sampler_patch::PatchState::Ready(Box::new(p)) });
    Ok(())
}

/// A 1-based number param in `1..=max`, as the 0-based index.
fn index_param(id: &str, p: &Value, key: &str, max: usize) -> Result<usize> {
    let n = p.get(key).and_then(Value::as_u64).ok_or_else(|| bad(id, format!("`{key}` required (1-{max})")))?;
    usize::try_from(n).ok().filter(|n| (1..=max).contains(n)).map(|n| n - 1).ok_or_else(|| bad(id, format!("`{key}` must be 1-{max}")))
}

fn zone_param(id: &str, p: &Value) -> Result<u16> {
    index_param(id, p, "zone", transit_model::sampler_patch::ZONES).map(|z| z as u16)
}

fn pad_param(id: &str, p: &Value) -> Result<u8> {
    index_param(id, p, "pad", transit_model::sampler_patch::PADS).map(|z| z as u8)
}

const REGION_KEYS: [&str; 5] = ["start", "end", "loop_mode", "loop_start", "loop_end"];
const PLAYBACK_KEYS: [&str; 15] = [
    "root",
    "transpose",
    "cents",
    "reverse",
    "mode",
    "key_tracking",
    "attack_ms",
    "decay_ms",
    "sustain",
    "release_ms",
    "declick_ms",
    "velocity_sensitivity",
    "keys",
    "velocities",
    "label",
];

/// `zone` with the `region` and `playback` objects of `p` applied (unknown keys refused).
fn apply_settings(id: &str, zone: &ZoneEntry, p: &Value) -> Result<ZoneEntry> {
    let mut v = serde_json::to_value(zone).map_err(|err| bad(id, err.to_string()))?;
    for (group, keys) in [("region", &REGION_KEYS[..]), ("playback", &PLAYBACK_KEYS[..])] {
        let Some(given) = p.get(group) else { continue };
        let given: &Map<String, Value> = given.as_object().ok_or_else(|| bad(id, format!("`{group}` must be an object")))?;
        for (k, x) in given {
            if !keys.contains(&k.as_str()) {
                return Err(bad(id, format!("`{group}` has no `{k}` (it takes {})", keys.join(", "))));
            }
            if let Some(o) = v.as_object_mut() {
                o.insert(k.clone(), x.clone());
            }
        }
    }
    serde_json::from_value(v).map_err(|err| bad(id, format!("a zone setting has the wrong type: {err}")))
}

fn new_patch(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_new_patch";
    let t = instrument_track(e, id, p)?;
    let seed = match p.get("seed") {
        None => 1,
        Some(v) => v.as_u64().and_then(|x| u32::try_from(x).ok()).ok_or_else(|| bad(id, "`seed` must be 0-4294967295"))?,
    };
    let info = transit_dsp::plugin_info(SAMPLER).ok_or_else(|| bad(id, "the sampler is not available"))?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(id, "no track"))?;
    if tr.instrument.as_ref().is_some_and(|i| i.plugin == SAMPLER && i.sampler_patch.is_some()) {
        return Err(bad(id, "the track's Sampler already has a patch"));
    }
    if !tr.instrument.as_ref().is_some_and(|i| i.plugin == SAMPLER) {
        let mut ins = Insert::new(SAMPLER);
        fill_default_params(&mut ins, info);
        tr.instrument = Some(ins);
    }
    if let Some(ins) = tr.instrument.as_mut() {
        // ADR 0041: a new patch leaves the one-sample field unset, so an older build does not play an
        // arbitrary first sample as the whole kit.
        ins.sample = None;
        ins.sampler_patch =
            Some(SamplerPatch { state: transit_model::sampler_patch::PatchState::Ready(Box::new(PatchV1 { seed, ..PatchV1::default() })) });
    }
    Ok(json!({"track": t}))
}

/// The size of decoded audio: frames × channels × 4 bytes, `None` on overflow.
fn pcm_bytes(frames: u64, channels: u64) -> Option<u64> {
    frames.checked_mul(channels)?.checked_mul(4)
}

/// Refuses a source of `frames` frames and `channels` channels at `rate` that a zone cannot hold
/// in memory (ADR 0041's per-source bounds, the minimum with ADR 0035's streaming threshold).
pub fn admit_source(s: &Session, name: &str, frames: u64, channels: u64, rate: u32) -> std::result::Result<u64, String> {
    if !(1..=2).contains(&channels) {
        return Err(format!("{name}: a Sampler zone takes mono or stereo audio ({channels} channels): convert it first"));
    }
    let seconds = MAX_SOURCE_SECONDS.saturating_mul(u64::from(rate));
    let threshold = transit_model::disk::stream_threshold_frames(s, rate);
    let bytes = pcm_bytes(frames, channels).unwrap_or(u64::MAX);
    let length = frames as f64 / f64::from(rate.max(1));
    if frames > seconds {
        return Err(format!(
            "{name}: {length:.1} s is longer than the {MAX_SOURCE_SECONDS} s a Sampler zone holds in memory (longer samples stream from disk in a later version)"
        ));
    }
    if bytes > MAX_SOURCE_BYTES {
        return Err(format!("{name}: {} MiB of audio is more than the 32 MiB a Sampler zone holds in memory", bytes >> 20));
    }
    if frames > threshold {
        return Err(format!(
            "{name}: {length:.1} s is longer than this session's streaming threshold, so it would play from disk, which a Sampler zone cannot yet"
        ));
    }
    Ok(bytes)
}

/// The distinct decoded audio of `p`'s usable sources in `s`, in bytes (a source reused by several
/// zones counts once).
fn patch_bytes(p: &PatchV1, s: &Session) -> u64 {
    let mut seen: Vec<SourceId> = Vec::new();
    let mut total = 0u64;
    for r in &p.sources {
        if seen.contains(&r.source) {
            continue;
        }
        seen.push(r.source);
        if let Some(a) = s.pool.get(r.source).filter(|a| !a.is_disk()) {
            total = total.saturating_add(pcm_bytes(a.frames() as u64, a.buffer.num_channels() as u64).unwrap_or(u64::MAX));
        }
    }
    total
}

/// Refuses a patch whose distinct decoded audio would pass 128 MiB with `adding` more bytes.
pub(super) fn admit_patch(p: &PatchV1, s: &Session, adding: u64) -> std::result::Result<(), String> {
    let total = patch_bytes(p, s).saturating_add(adding);
    if total > MAX_PATCH_BYTES {
        return Err(format!("the patch would hold {} MiB of audio, more than the 128 MiB a Sampler holds in memory", total >> 20));
    }
    Ok(())
}

/// A file param resolved: relative paths are taken from the session's folder (never the process's
/// working directory).
pub(super) fn resolve_path(e: &Engine, id: &str, given: &str) -> Result<PathBuf> {
    let p = Path::new(given);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    let folder = e.session().folder.clone().or_else(|| e.path.as_deref().and_then(|x| Path::new(x).parent().map(Path::to_path_buf)));
    let folder =
        folder.ok_or_else(|| bad(id, format!("`{given}` is relative and the session has no folder yet: save it first or give an absolute path")))?;
    Ok(folder.join(p))
}

/// Frames of a file of `frames` at `from` once converted to `to` (rounded up), `None` on overflow.
pub(crate) fn frames_at(frames: u64, from: u32, to: u32) -> Option<u64> {
    if from == to || from == 0 {
        return Some(frames);
    }
    let n = u128::from(frames).checked_mul(u128::from(to))?.div_ceil(u128::from(from));
    u64::try_from(n).ok()
}

/// Where a zone's new source comes from.
enum NewSource {
    /// An existing session source.
    Session(SourceId),
    /// Decoded audio to add, with the path it keeps (`None`: managed `Audio Files/` audio).
    Audio(crate::io::PreparedAudio, Ownership),
}

fn source_param(e: &mut Engine, id: &str, p: &Value, admitted: &PatchV1) -> Result<NewSource> {
    let copy = bool_or(p, "copy", false);
    let ownership = if copy { Ownership::Managed } else { Ownership::Referenced };
    let rate = e.session().sample_rate.hz();
    let given = ["source", "path", "import"].iter().filter(|k| p.get(**k).is_some()).count();
    if given != 1 {
        return Err(bad(id, "give exactly one of `source`, `path` or `import`"));
    }
    if let Some(n) = p.get("source") {
        let sid = SourceId(n.as_u64().ok_or_else(|| bad(id, "`source` must be a source id"))?);
        let s = e.session();
        let src = s.source(sid).ok_or_else(|| bad(id, format!("no source {sid}")))?;
        let audio = s.pool.get(sid).ok_or_else(|| bad(id, format!("{}: its audio is missing", src.name)))?;
        if audio.is_disk() {
            return Err(bad(id, format!("{}: it plays from disk, which a Sampler zone cannot yet", src.name)));
        }
        let bytes = admit_source(s, &src.name, audio.frames() as u64, audio.buffer.num_channels() as u64, rate).map_err(|m| bad(id, m))?;
        let adding = if admitted.sources.iter().any(|r| r.source == sid) { 0 } else { bytes };
        admit_patch(admitted, s, adding).map_err(|m| bad(id, m))?;
        return Ok(NewSource::Session(sid));
    }
    if let Some(path) = str_param(p, "path") {
        let abs = resolve_path(e, id, path)?;
        let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
        // Admission from the header alone, before the body is read, for a file sized by its header
        // (WAV/RF64); any other file is checked from its bytes below, still before decoding.
        if let Some(info) = crate::io::probe_header(&abs) {
            let frames = frames_at(info.frames, info.sample_rate, rate).unwrap_or(u64::MAX);
            let pcm = admit_source(e.session(), &name, frames, u64::from(info.channels), rate).map_err(|m| bad(id, m))?;
            admit_patch(admitted, e.session(), pcm).map_err(|m| bad(id, m))?;
        }
        let bytes = crate::io::read_file(&abs)?;
        let ext = abs.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        // Admission from the header, before anything is decoded.
        let info = transit_audio_io::probe(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        let frames = frames_at(info.frames, info.sample_rate, rate).unwrap_or(u64::MAX);
        let pcm = admit_source(e.session(), &name, frames, u64::from(info.channels), rate).map_err(|m| bad(id, m))?;
        admit_patch(admitted, e.session(), pcm).map_err(|m| bad(id, m))?;
        let stored = abs.to_string_lossy().into_owned();
        let prepared = crate::io::prepare_audio_bytes(&name, &bytes, Some(&stored), rate, &mut |_| true)?;
        return Ok(NewSource::Audio(prepared, ownership));
    }
    let job = p.get("import").and_then(Value::as_u64).ok_or_else(|| bad(id, "`import` must be an import id (file.prepare_import)"))?;
    e.imports.ready(job)?;
    if let Some(r) = e.imports.prepared_rate(job).filter(|r| *r != rate) {
        return Err(bad(id, format!("prepared at {r} Hz; the session is now {rate} Hz: re-import")));
    }
    let report = e.imports.progress(job).unwrap_or_default();
    let (frames, channels) = (report["frames"].as_u64().unwrap_or(0), report["channels"].as_u64().unwrap_or(0));
    let name = report["name"].as_str().unwrap_or("Sample").to_string();
    // Checked before the import is taken, so a refusal leaves it staged.
    let pcm = admit_source(e.session(), &name, frames, channels, rate).map_err(|m| bad(id, m))?;
    admit_patch(admitted, e.session(), pcm).map_err(|m| bad(id, m))?;
    let prepared = e.imports.take(job)?;
    // A prepared file keeps its path (referenced unless copied); one without a path is managed.
    let ownership = if prepared.path.is_some() { ownership } else { Ownership::Managed };
    Ok(NewSource::Audio(prepared, ownership))
}

/// Adds decoded audio as a session source: a referenced file keeps its path (and is not written
/// on save); managed audio is written into `Audio Files/` on the next save.
fn add_audio(e: &mut Engine, prepared: crate::io::PreparedAudio, ownership: Ownership) -> SourceId {
    let path = if ownership == Ownership::Referenced { prepared.path.clone() } else { None };
    let s = e.session_mut();
    let sid = crate::io::add_source_audio(s, &prepared.name, prepared.audio, path.as_deref(), prepared.format);
    if ownership == Ownership::Referenced
        && let Some(src) = s.sources.iter_mut().find(|x| x.id == sid)
    {
        // The file is on disk where it is: nothing to write.
        src.unsaved = false;
    }
    sid
}

fn zone_source(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_zone_source";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let zone = zone_param(id, p)?;
    let pad = p.get("pad").map(|_| pad_param(id, p)).transpose()?;
    if let Some(pad) = pad {
        let bound = patch.pad(pad).zone;
        if bound.is_some_and(|z| z != zone) {
            return Err(bad(id, format!("pad {} is bound to zone {}: unbind it first", pad + 1, bound.unwrap_or(0) + 1)));
        }
    }
    // Settings parse before any file is read.
    apply_settings(id, &ZoneEntry::default(), p)?;
    let (sid, ownership) = match source_param(e, id, p, &patch)? {
        NewSource::Session(sid) => (sid, Ownership::Managed),
        NewSource::Audio(prepared, ownership) => (add_audio(e, prepared, ownership), ownership),
    };
    let s = e.session();
    let audio = s.pool.get(sid).ok_or_else(|| bad(id, "the source has no audio"))?;
    let (frames, channels) = (audio.frames() as u64, audio.buffer.num_channels());
    let index = u16::try_from(patch.source_index(sid, ownership)).map_err(|_| bad(id, "too many sources"))?;
    let mut entry = patch.zone(zone).cloned().map_or_else(
        || ZoneEntry::whole(zone, index, frames),
        |z| ZoneEntry { source: index, start: 0, end: frames, loop_start: 0, loop_end: frames, slice: None, ..z },
    );
    let fresh = patch.zone(zone).is_none();
    if let (Some(pad), true) = (pad, fresh) {
        entry.pad_defaults(patch.pad_note(pad));
    }
    let entry = apply_settings(id, &entry, p)?;
    patch.zones.retain(|z| z.id != zone);
    patch.zones.push(entry);
    patch.zones.sort_by_key(|z| z.id);
    if let Some(pad) = pad {
        patch.set_pad(PadEntry { zone: Some(zone), ..patch.pad(pad) });
    }
    patch.drop_unused_sources();
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "zone": zone + 1, "source": sid, "frames": frames, "channels": channels}))
}

fn zone_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_zone_edit";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let zone = zone_param(id, p)?;
    let current = patch.zone(zone).cloned().ok_or_else(|| bad(id, format!("zone {} is empty", zone + 1)))?;
    let next = apply_settings(id, &current, p)?;
    if let Some(z) = patch.zone_mut(zone) {
        *z = next;
    }
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "zone": zone + 1}))
}

fn zone_clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_zone_clear";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let zone = zone_param(id, p)?;
    if patch.zone(zone).is_none() {
        return Err(bad(id, format!("zone {} is already empty", zone + 1)));
    }
    patch.zones.retain(|z| z.id != zone);
    if let Some(pad) = patch.pad_of(zone) {
        patch.set_pad(PadEntry { zone: None, ..patch.pad(pad) });
    }
    patch.drop_unused_sources();
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "zone": zone + 1}))
}

fn pad_bind(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_pad_bind";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let (pad, zone) = (pad_param(id, p)?, zone_param(id, p)?);
    if patch.zone(zone).is_none() {
        return Err(bad(id, format!("zone {} is empty", zone + 1)));
    }
    if let Some(other) = patch.pad_of(zone).filter(|o| *o != pad) {
        return Err(bad(id, format!("zone {} is bound to pad {}", zone + 1, other + 1)));
    }
    patch.set_pad(PadEntry { zone: Some(zone), ..patch.pad(pad) });
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "pad": pad + 1, "zone": zone + 1}))
}

fn pad_unbind(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_pad_unbind";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let pad = pad_param(id, p)?;
    patch.set_pad(PadEntry { zone: None, ..patch.pad(pad) });
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "pad": pad + 1}))
}

fn pad_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_pad_set";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let pad = pad_param(id, p)?;
    let mut entry = patch.pad(pad);
    if let Some(v) = p.get("note") {
        // `null` returns the pad to its default note, checked like any note edit.
        let stored = match v {
            Value::Null => None,
            _ => Some(v.as_u64().and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 127).ok_or_else(|| bad(id, "`note` must be 0-127"))?),
        };
        let note = stored.unwrap_or_else(|| PadEntry::default_note(pad, patch.pad_start_note));
        if let Some(other) = (0..transit_model::sampler_patch::PADS as u8).find(|q| *q != pad && patch.pad_note(*q) == note) {
            return Err(bad(id, format!("note {note} is pad {}'s trigger note", other + 1)));
        }
        if entry.zone.is_some() {
            patch.check_pad_layers(pad, note).map_err(|r| bad(id, r.text(e.message_middle_c())))?;
        }
        entry.note = stored;
    }
    if let Some(v) = p.get("choke") {
        entry.choke = v.as_u64().and_then(|n| u8::try_from(n).ok()).filter(|n| *n <= 16).ok_or_else(|| bad(id, "`choke` must be 0 (Off) to 16"))?;
    }
    for (key, slot) in [("mute", &mut entry.mute), ("solo", &mut entry.solo)] {
        if let Some(v) = p.get(key) {
            *slot = v.as_bool().ok_or_else(|| bad(id, format!("`{key}` must be true or false")))?;
        }
    }
    if let Some(v) = p.get("name") {
        entry.name = match v {
            Value::Null => None,
            Value::String(n) if n.is_empty() => None,
            Value::String(n) => Some(n.clone()),
            _ => return Err(bad(id, "`name` must be text")),
        };
    }
    let note = entry.trigger(patch.pad_start_note);
    patch.set_pad(entry);
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "pad": pad + 1, "note": note}))
}

/// `mix.sampler_pad_start`: one step that moves the defaulted pads and their zones' roots, refused
/// whole (ADR 0041 amendment 2026-11-09, item 3); an unchanged start note records no step.
fn pad_start(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_pad_start";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let names = e.message_middle_c();
    let refused = |r: PadRefusal| bad(id, r.text(names));
    let note = p.get("note").and_then(Value::as_u64).and_then(|n| u8::try_from(n).ok()).ok_or_else(|| refused(PadRefusal::StartRange))?;
    let change = patch.set_pad_start(note).map_err(refused)?;
    if change.semitones() != 0 {
        store(e, id, t, patch)?;
    }
    Ok(json!({"track": t, "note": change.to, "from": change.from, "semitones": change.semitones(), "notice": change.notice(names)}))
}

fn seed(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_seed";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    patch.seed = p.get("seed").and_then(Value::as_u64).and_then(|x| u32::try_from(x).ok()).ok_or_else(|| bad(id, "`seed` must be 0-4294967295"))?;
    let seed = patch.seed;
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "seed": seed}))
}

fn control(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_control";
    let t = instrument_track(e, id, p)?;
    let mut patch = patch_of(e.session(), id, t)?;
    let key = str_param(p, "id").ok_or_else(|| bad(id, "`id` required (zone001_level, ...)"))?;
    let (_, field) = dsp_sampler::resolve(key).ok_or_else(|| bad(id, format!("`{key}` is not a Sampler zone control")))?;
    let info = dsp_sampler::field_info(field).ok_or_else(|| bad(id, format!("`{key}` is not a Sampler zone control")))?;
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`value` must be a number"))?;
    let value = info.clamp(value as f32);
    patch.set_control(key, value, info.default);
    store(e, id, t, patch)?;
    Ok(json!({"track": t, "id": key, "value": value}))
}

/// Gives zone `zone` (0-based) of track `t`'s patch dropped bytes as its source, as one undo step
/// ("Set Sampler Zone Source"): bytes have no durable path, so the audio is the session's own and
/// is written into `Audio Files/` on the next save (ADR 0041). The same checks as the command.
pub fn load_zone_bytes(e: &mut Engine, t: TrackId, zone: u16, name: &str, bytes: &[u8]) -> Result<Value> {
    let id = "mix.sampler_zone_source";
    let before = e.session_arc();
    let r = (|| {
        let patch = patch_of(e.session(), id, t)?;
        let rate = e.session().sample_rate.hz();
        let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let info = transit_audio_io::probe(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        let frames = frames_at(info.frames, info.sample_rate, rate).unwrap_or(u64::MAX);
        let pcm = admit_source(e.session(), name, frames, u64::from(info.channels), rate).map_err(|m| bad(id, m))?;
        admit_patch(&patch, e.session(), pcm).map_err(|m| bad(id, m))?;
        let prepared = crate::io::prepare_audio_bytes(name, bytes, None, rate, &mut |_| true)?;
        let sid = add_audio(e, prepared, Ownership::Managed);
        zone_source(e, &json!({"track": t, "zone": zone + 1, "source": sid})).inspect_err(|_| {
            // Refused after the audio was added: it goes again, so the document is as it was.
            let s = e.session_mut();
            s.sources.retain(|x| x.id != sid);
            s.pool.remove(sid);
        })
    })()?;
    e.push_undo("Set Sampler Zone Source", before);
    Ok(r)
}

#[cfg(test)]
#[path = "tests_sampler_patch.rs"]
mod tests;

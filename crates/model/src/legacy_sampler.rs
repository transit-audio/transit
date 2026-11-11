//! The retired one-sample Sampler (issue #455, ADR 0041 amendment 2026-11-11).
//!
//! Builds before it had a Sampler that played one session source across the keyboard: a slot
//! naming `sampler` with no `sampler_patch`, its state in `Insert::sample` and 19 parameters. That
//! instrument is gone. A session that still holds one gets an empty Sampler kit in the slot when it
//! loads: the sample assignment and the 19 parameters are dropped, no conversion, and so are the
//! automation lanes, MIDI mappings, rack macro targets and user pages that addressed them; the
//! source stays in the session. Each track is reported in the session's load notes. Alpha
//! software: no other shim.

use crate::{AutoParam, Control, Insert, SamplerPatch, Session, SourceId, Strip, TrackId};

/// The plug-in id of the built-in Sampler.
pub const SAMPLER_ID: &str = "sampler";

/// The one-sample instrument's 19 parameter ids: what the drop removes and what the lanes,
/// mappings and macro targets it drops named.
pub const RETIRED_PARAMS: [&str; 19] = [
    "root_note",
    "transpose",
    "fine_tune",
    "key_track",
    "key_low",
    "key_high",
    "vel_low",
    "vel_high",
    "velocity",
    "attack",
    "decay",
    "sustain",
    "release",
    "play_mode",
    "start",
    "loop_mode",
    "loop_start",
    "loop_end",
    "level",
];

/// How the one-line summary of a load that dropped the instrument ends: `N track(s) ...`.
pub const ONE_SAMPLE_NOTE: &str = "had the old one-sample Sampler and now have an empty Sampler kit";

/// Whether `ins` is the retired one-sample Sampler: it names `sampler` and has no patch. A patch in
/// any state (ready, of an unknown schema, kept opaque) is never one.
#[must_use]
pub fn is_one_sample(ins: &Insert) -> bool {
    ins.plugin == SAMPLER_ID && ins.sampler_patch.is_none()
}

/// Does any track's instrument slot still hold the one-sample Sampler?
#[must_use]
pub fn uses_one_sample_sampler(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.instrument.as_ref().is_some_and(is_one_sample))
}

/// The slot part of the drop: an empty kit (seed 1, pad start note 36), no `sample`, none of the 19
/// ids, no user pages and the preset label `Insert::new` gives; `bypass` and `active` are kept.
/// Returns the old sample (`None` inside when it had none), or `None` when `ins` is not one-sample
/// (then nothing changed).
pub fn retire_slot(ins: &mut Insert) -> Option<Option<SourceId>> {
    if !is_one_sample(ins) {
        return None;
    }
    let sample = ins.sample.take();
    ins.sampler_patch = Some(SamplerPatch::new());
    ins.params.retain(|k, _| !is_retired(k));
    ins.pages.clear();
    ins.preset = Insert::new(SAMPLER_ID).preset;
    Some(sample)
}

/// The load note for track `name`: `sample` is the file name of the old sample when the session
/// still lists it, `dropped` what addressed the old parameters (`2 automation lane(s)`, ...).
#[must_use]
pub fn track_line(name: &str, sample: Option<&str>, dropped: &[String]) -> String {
    let mut line = format!("{}the old one-sample Sampler was removed and is now an empty kit", line_start(name));
    if let Some(file) = sample {
        line.push_str(&format!("; its sample \"{file}\" stays in the session's files and can be loaded onto a pad again"));
    }
    if !dropped.is_empty() {
        line.push_str(&format!("; dropped {}", dropped.join(", ")));
    }
    line
}

/// How the track line of track `name` begins (an import picks the donor's lines by it).
#[must_use]
pub fn line_start(name: &str) -> String {
    format!("track \"{name}\": ")
}

/// The summary for `n` tracks.
#[must_use]
pub fn summary(n: usize) -> String {
    format!("{n} track(s) {ONE_SAMPLE_NOTE}")
}

/// Whether the track line `line` is the one for track `name`.
#[must_use]
pub fn is_track_line(line: &str, name: &str) -> bool {
    line.strip_prefix(&line_start(name)).is_some_and(|rest| rest.starts_with("the old one-sample Sampler was removed"))
}

fn is_retired(id: &str) -> bool {
    RETIRED_PARAMS.contains(&id)
}

/// The file name a source's stored path ends in.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Give every instrument slot holding the one-sample Sampler an empty kit, drop what addressed its
/// 19 parameters on that track, and push the load notes: one line per track, then the summary.
/// Returns how many tracks changed (0: nothing changed, no note).
pub fn retire_one_sample_sampler(s: &mut Session) -> usize {
    let found: Vec<TrackId> = s.tracks.iter().filter(|t| t.instrument.as_ref().is_some_and(is_one_sample)).map(|t| t.id).collect();
    if found.is_empty() {
        return 0;
    }
    let slot = AutoParam::INSTRUMENT_SLOT;
    let mut notes = Vec::new();
    for &id in &found {
        let (mut sample, mut pages, mut lanes, mut name) = (None, 0, 0, String::new());
        if let Some(t) = s.track_mut(id) {
            if let Some(ins) = t.instrument.as_mut() {
                pages = ins.pages.len();
                sample = retire_slot(ins).flatten();
            }
            let before = t.automation.len();
            t.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { slot: x, param } if *x == slot && is_retired(param)));
            lanes = before - t.automation.len();
            name = t.name.clone();
        }
        let file = sample.and_then(|sid| s.source(sid)).map(|src| file_name(&src.path).to_string());
        let mut dropped = Vec::new();
        if lanes > 0 {
            dropped.push(format!("{lanes} automation lane(s)"));
        }
        let maps = s.midi_mappings.remove_where(
            |m| matches!((&m.strip, &m.control), (Strip::Track(x), Control::Plugin { slot: y, param }) if *x == id && *y == slot && is_retired(param)),
        );
        if maps > 0 {
            dropped.push(format!("{maps} MIDI mapping(s)"));
        }
        let macros: usize = s
            .tracks
            .iter_mut()
            .filter_map(|r| r.rack.as_mut())
            .flat_map(|r| r.macros.iter_mut())
            .map(|m| {
                let n = m.mappings.len();
                m.mappings.retain(|x| !(x.track == id && x.slot == slot && is_retired(&x.param)));
                n - m.mappings.len()
            })
            .sum();
        if macros > 0 {
            dropped.push(format!("{macros} rack macro target(s)"));
        }
        if pages > 0 {
            dropped.push(format!("{pages} user page(s)"));
        }
        notes.push(track_line(&name, file.as_deref(), &dropped));
    }
    notes.push(summary(found.len()));
    s.load_notes.extend(notes);
    found.len()
}

#[cfg(test)]
#[path = "legacy_sampler_tests.rs"]
mod tests;

//! The second phase of a plug-in scan (ADR 0064, decision 2.4): after a format's files are listed
//! and the listing is published, every listed class is created once in a child process of its own
//! (`<program> --probe-vst3 <file> <class id>`, `--probe-clap <file> <plugin id>`), which reads
//! what the class can do and exits without activating, processing or showing anything. A probe
//! that returns an error, crashes, prints no answer or runs past its timeout only marks its own
//! class: it never removes a listing or changes a class's identity or category.
//!
//! The outcome is a field of the class's cached descriptor ([`ProbeOutcome`]), so the scan cache
//! keeps it with the file: a file that has not changed is not probed again, except a class whose
//! probe failed in another app build (as file failures are retried, [`crate::scan_cache`]).

use crate::scan_cache::{Entry, REPLY};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a probe child read from one class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// The class declares at least one note output transit can read: a VST3 event output bus, or
    /// a CLAP output note port whose dialects include CLAP, MIDI or MIDI MPE.
    pub note_output: bool,
    /// The class declares a note input transit can feed: a VST3 event input bus, or a CLAP input
    /// note port whose dialects include CLAP, MIDI or MIDI MPE.
    pub note_input: bool,
    /// Audio output buses (VST3) or ports (CLAP).
    pub audio_outputs: u32,
    /// The index of the first readable note output (VST3 event output bus 0, CLAP the lowest
    /// readable output note port); `None` without one.
    pub first_note_output: Option<u32>,
}

/// The probe fields of a cached class descriptor. A class not probed yet has every field `None`;
/// an answered probe fills the capability fields; a failed one (an error, a crash, no answer, a
/// timeout) fills `probe_error` and `probe_host` and leaves the capability `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeOutcome {
    /// `Some(true | false)` when the probe answered (see [`Capability::note_output`]).
    #[serde(default)]
    pub note_output: Option<bool>,
    /// `Some(..)` when the probe answered (see [`Capability::note_input`]).
    #[serde(default)]
    pub note_input: Option<bool>,
    /// `Some(n)` when the probe answered (see [`Capability::audio_outputs`]).
    #[serde(default)]
    pub audio_outputs: Option<u32>,
    /// The first readable note output's index, when the probe answered and found one.
    #[serde(default)]
    pub note_output_index: Option<u32>,
    /// Why the probe failed.
    #[serde(default)]
    pub probe_error: Option<String>,
    /// The app build that made a failed probe ([`crate::scan_cache::this_host`]).
    #[serde(default)]
    pub probe_host: Option<String>,
}

impl ProbeOutcome {
    /// The outcome of a probe that answered.
    pub fn answered(c: Capability) -> Self {
        ProbeOutcome {
            note_output: Some(c.note_output),
            note_input: Some(c.note_input),
            audio_outputs: Some(c.audio_outputs),
            note_output_index: c.first_note_output,
            probe_error: None,
            probe_host: None,
        }
    }

    /// The outcome of a probe that failed in app build `host`.
    pub fn failed(error: String, host: &str) -> Self {
        ProbeOutcome { probe_error: Some(error), probe_host: Some(host.to_string()), ..ProbeOutcome::default() }
    }

    /// Whether this build probes the class: not probed yet, or its probe failed in another build.
    /// A probe that answered is reused until the file changes.
    pub fn needs_probe(&self, host: &str) -> bool {
        self.note_output.is_none() && (self.probe_error.is_none() || self.probe_host.as_deref() != Some(host))
    }
}

/// A cached class descriptor that carries a [`ProbeOutcome`].
pub trait Probed {
    fn outcome(&self) -> &ProbeOutcome;
    fn outcome_mut(&mut self) -> &mut ProbeOutcome;
}

/// One class whose probe failed, for the diagnostics report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeProblem {
    /// The plug-in file.
    pub path: PathBuf,
    /// The class's id in the app (`vst3:...`, `clap:...`).
    pub id: String,
    pub name: String,
    /// Why the probe failed.
    pub reason: String,
    /// The app build that tried it.
    pub host: String,
}

/// Probes, in file order, every class of `files` that [`ProbeOutcome::needs_probe`] (files that
/// did not load have no classes). `probe(file, class)` runs one child for one class: `None` means
/// the scan was told to stop (the class and every later one stay unprobed, so the next scan probes
/// them). After each outcome is recorded, `publish(file, class)` is told. Returns the number of
/// probes run.
pub fn probe_files<D: Probed>(
    files: &mut [Entry<D>],
    host: &str,
    mut probe: impl FnMut(&Path, &D) -> Option<Result<Capability, String>>,
    mut publish: impl FnMut(&Path, &D),
) -> usize {
    let mut run = 0;
    for entry in files.iter_mut() {
        let path = entry.path.clone();
        for class in entry.plugins.iter_mut() {
            if !class.outcome().needs_probe(host) {
                continue;
            }
            let Some(answer) = probe(&path, class) else { return run };
            run += 1;
            *class.outcome_mut() = match answer {
                Ok(c) => ProbeOutcome::answered(c),
                Err(e) => {
                    log::warn!("{e}");
                    ProbeOutcome::failed(e, host)
                }
            };
            publish(&path, class);
        }
    }
    run
}

/// What a probe child prints: one line with the class's capability, or why it failed.
pub fn reply<E: std::fmt::Display>(probed: Result<Capability, E>) -> String {
    let body = match probed {
        Ok(c) => serde_json::json!({ "probe": c }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    format!("{REPLY}{body}")
}

#[derive(Deserialize)]
struct Reply {
    probe: Option<Capability>,
    error: Option<String>,
}

/// Reads a probe child's answer line (the JSON after the reply prefix).
pub(crate) fn parse_reply(json: &str, fail: impl Fn(String) -> String) -> Result<Capability, String> {
    let r: Reply = serde_json::from_str(json).map_err(|e| fail(format!("unreadable probe reply: {e}")))?;
    match (r.probe, r.error) {
        (Some(c), _) => Ok(c),
        (None, e) => Err(e.unwrap_or_else(|| fail("the probe process answered nothing".into()))),
    }
}

/// The classes of `list` whose probe failed.
pub fn problems<'a, D: Probed + 'a>(list: impl IntoIterator<Item = &'a D>, describe: impl Fn(&D) -> (PathBuf, String, String)) -> Vec<ProbeProblem> {
    list.into_iter()
        .filter_map(|d| {
            let o = d.outcome();
            let reason = o.probe_error.clone()?;
            let (path, id, name) = describe(d);
            Some(ProbeProblem { path, id, name, reason, host: o.probe_host.clone().unwrap_or_default() })
        })
        .collect()
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;

//! The `acid` tempo of a slice loop's WAV file where commands run (ADR 0041 amendment 2026-11-08,
//! item 5; decided by the maintainer 2026-11-10: Slice to MIDI's Fit length defaults to the file's
//! embedded tempo when it carries one). The state query and Slice to MIDI read it through here, so
//! the default beat length the panel shows is the one Fit uses, and the grid tempo the state
//! reports names `acid` as its origin as Detect does. A file is read once and kept per path for
//! the engine's life; a session change clears the cache.

use super::sampler_patch::resolve_path;
use crate::Engine;
use std::cell::RefCell;
use std::path::PathBuf;
use transit_audio_io::FileFormat;
use transit_model::sampler_patch::{PatchV1, SliceLoop};

/// The files already read: each resolved path and its `acid` tempo (`None`: no `acid` chunk).
#[derive(Default)]
pub(crate) struct AcidCache(RefCell<Vec<(PathBuf, Option<f64>)>>);

/// The `acid` tempo the loop source's WAV file states (range-checked later by `usable_acid`);
/// `None` for a source that is not a saved WAV file, or a file that cannot be read now (not kept,
/// so a later read can succeed).
pub(crate) fn loop_acid(e: &Engine, id: &str, p: &PatchV1, l: &SliceLoop) -> Option<f64> {
    let s = e.session();
    let src = s.source(p.sources.get(usize::from(l.source))?.source)?;
    if src.format != FileFormat::Wav || src.unsaved {
        return None;
    }
    let path = resolve_path(e, id, &src.path).ok()?;
    if let Some((_, tempo)) = e.acid_cache.0.borrow().iter().find(|(known, _)| *known == path) {
        return *tempo;
    }
    let read = || crate::io::read_file(&path);
    let bytes = match crate::file_access::current() {
        Some(roots) => crate::file_access::confine(Some(&roots), read),
        None => crate::file_access::unconfined(read),
    }
    .ok()?;
    let tempo = transit_audio_io::wav_tempo(&bytes);
    e.acid_cache.0.borrow_mut().push((path, tempo));
    tempo
}

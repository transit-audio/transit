//! The built-in Sampler's commands (ADR 0041): every Sampler is a kit whose zones take their
//! sources through `mix.sampler_zone_source` (`cmd/sampler_patch.rs` and its siblings). The
//! one-sample commands `mix.sampler_load` and `mix.sampler_clear` are retired with the one-sample
//! instrument (ADR 0041 amendment 2026-11-11, #455).

use super::*;
use transit_model::TrackKind;

pub fn specs() -> Vec<CommandSpec> {
    super::sampler_patch::specs()
}

/// The instrument track a sampler command acts on.
pub(super) fn instrument_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad(id, "only instrument tracks have a sampler"));
    }
    Ok(t)
}

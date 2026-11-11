//! What the Group panel's gestures send ([`super::group_panel`]), each one command and one undo
//! step, with only what it changes; the engine decides and its refusal is the view's note.
//!
//! - **A chip** selects its zone (UI state, nothing sent).
//! - **The add chip** on a layer of a group asks for a sample (the file picker) and loads it into
//!   the lowest empty zone as the layer's next alternative, with the group's keys, the layer's
//!   velocities and the selected zone's root: one `mix.sampler_zone_source`. On a zone in no
//!   group it makes the zone a group of its own (one `mix.sampler_zone_map` with its membership;
//!   a second zone cannot be loaded and joined in the same step, so the next add brings the
//!   sample). On a pad's zone it only asks (the Notice); **Make group** then unbinds the zone and
//!   makes it a group in one `mix.sampler_zone_map`, on the pad's note at full velocity so its
//!   notes still play it ("Its notes still play the group"); **Cancel** sends nothing.
//! - **Round robin**: one `mix.sampler_group_set`.
//! - **Seed**: one `mix.sampler_seed` with the typed value (the engine stores 1 for 0 and refuses
//!   anything outside 0 to 4,294,967,295); the reroll sends a fresh value from the UI's own
//!   entropy (never the audio path), never the one stored.
//! - **Vel. sens.**: one `mix.sampler_zone_edit {playback: {velocity_sensitivity}}` on release.
//!
//! The UI picks the defaults the engine never guesses (ADR 0041 amendment 2026-11-08): the new
//! group's id (the lowest with no member and no stored mode) and the new zone (the lowest empty).

use super::group_panel::Intent;
use super::state::{State, Zone};
use super::{InstrumentView, send};
use crate::StudioApp;
use crate::device_editor::Dev;
use serde_json::json;
use transit_model::sampler_patch::{RoundRobin, ZONES};

/// The lowest group id with no member and no stored mode.
pub fn free_group(state: &State) -> Option<u8> {
    (0..=u8::MAX).find(|g| !state.groups.iter().any(|e| e.group == *g))
}

/// The lowest empty zone number (from 1).
pub fn free_zone(state: &State) -> Option<u16> {
    (1..=ZONES as u16).find(|n| state.zone(*n).is_none())
}

/// A seed from the UI's own entropy: 1 to 4,294,967,295 and never `now`.
pub fn fresh_seed(now: u32) -> u32 {
    use std::hash::BuildHasher;
    let s = std::collections::hash_map::RandomState::new();
    (0u64..).map(|i| s.hash_one(i) as u32).find(|x| *x != 0 && *x != now).unwrap_or(1)
}

fn place(group: u8, layer: u8, alternative: u8) -> serde_json::Value {
    json!({"group": group, "layer": layer, "alternative": alternative})
}

/// Sends what the Group panel asked for, for the selected zone `z`.
pub(crate) fn apply(app: &mut StudioApp, dev: Dev, v: &mut InstrumentView, state: &State, z: &Zone, intent: Intent) {
    match intent {
        Intent::Select(n) => {
            v.selected = Some(n);
            v.ask = None;
        }
        Intent::Add(_) if z.pad.is_some() => v.ask = Some(z.zone),
        Intent::Add(None) => {
            let Some(g) = free_group(state) else { return };
            let zone = json!({"zone": z.zone, "keys": z.keys, "velocities": z.velocities, "root": z.root, "membership": place(g, 0, 0)});
            send(app, dev, v, "mix.sampler_zone_map", json!({ "zones": [zone] }));
        }
        Intent::Add(Some(layer)) => {
            let (Some(m), Some(n)) = (z.membership, free_zone(state)) else { return };
            let alts: Vec<&Zone> = state.zones.iter().filter(|o| o.membership.is_some_and(|p| p.group == m.group && p.layer == layer)).collect();
            let Some(first) = alts.first() else { return };
            let next = alts.iter().filter_map(|o| o.membership.map(|p| p.alternative)).max().map_or(0, |a| a.saturating_add(1));
            let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("mix.sampler_zone_source", None)) else { return };
            let playback = json!({"keys": z.keys, "velocities": first.velocities, "root": z.root, "membership": place(m.group, layer, next)});
            if send(app, dev, v, "mix.sampler_zone_source", json!({"zone": n, "path": path, "playback": playback})) {
                v.selected = Some(n);
            }
        }
        Intent::Cancel => v.ask = None,
        Intent::MakeGroup => {
            v.ask = None;
            let (Some(pad), Some(g)) = (z.pad, free_group(state)) else { return };
            let Some(note) = state.pad_note(pad) else { return };
            let zone =
                json!({"zone": z.zone, "keys": [note, note], "velocities": [1, 127], "root": z.root, "membership": place(g, 0, 0), "pad": null});
            send(app, dev, v, "mix.sampler_zone_map", json!({ "zones": [zone] }));
        }
        Intent::RoundRobin(g, mode) => {
            let mode = if mode == RoundRobin::Random { "random" } else { "sequential" };
            send(app, dev, v, "mix.sampler_group_set", json!({"group": g, "round_robin": mode}));
        }
        Intent::Seed(seed) => {
            send(app, dev, v, "mix.sampler_seed", json!({ "seed": seed }));
        }
        Intent::Reroll => {
            send(app, dev, v, "mix.sampler_seed", json!({"seed": fresh_seed(state.seed)}));
        }
        Intent::Sensitivity(x) => {
            // The fader steps by hundredths; sent as that decimal, not the f32's widening.
            let x = (f64::from(x) * 100.0).round() / 100.0;
            send(app, dev, v, "mix.sampler_zone_edit", json!({"zone": z.zone, "playback": {"velocity_sensitivity": x}}));
        }
    }
}

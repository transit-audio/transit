//! The key map's gestures (README, Sampler views, Instrument view; ADR 0041 and its amendments), as
//! one `mix.sampler_zone_map` each, sent when the drag is let go (never per pointer move; Escape
//! sends nothing):
//!
//! - **Side edges:** the key range. A group has one key range (ADR 0041), so a member's edge moves
//!   every member's range in the same step.
//! - **Top edge:** the velocity split, the zone's highest velocity. A layer's alternatives share
//!   one velocity range, so they move together.
//! - **Root dot:** the zone's root key.
//! - **Pad-bound zones** do not drag: the pad's note is their key (the lock and its tooltip say so).
//!
//! The command takes each listed zone's keys, velocities, root and membership; only the zones the
//! gesture changes are listed, with their membership as it is, and no fades or pad. The conflict
//! preview applies the same values to the patch as stored and asks the model's and the instrument's
//! own admission checks (`PatchV1::check_groups`, `transit_mix::check_sampler_patch` and
//! `PatchV1::layer_overload`, the ones the command's store runs); the engine still decides on
//! release, and its refusal is what the view shows.

use super::state::{Place, State, Zone};
use serde_json::{Value, json};

/// The part of a zone a drag holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The left edge: the lowest key.
    Low,
    /// The right edge: the highest key.
    High,
    /// The top edge: the highest velocity.
    Top,
    /// The root dot.
    Root,
}

/// A drag in progress: the zone (from 1), the part, the key or velocity under the pointer, and
/// whether Escape canceled it.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub zone: u16,
    pub part: Part,
    pub value: u8,
    pub cancelled: bool,
}

/// One zone's values in the map command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneEdit {
    pub zone: u16,
    pub keys: [u8; 2],
    pub velocities: [u8; 2],
    pub root: u8,
    pub membership: Option<Place>,
}

impl ZoneEdit {
    fn of(z: &Zone) -> ZoneEdit {
        ZoneEdit { zone: z.zone, keys: z.keys, velocities: z.velocities, root: z.root, membership: z.membership }
    }
}

/// The zones `drag` changes, with their new values (none: the drag lands where it started).
pub fn edits(state: &State, drag: &Drag) -> Vec<ZoneEdit> {
    let Some(z) = state.zone(drag.zone) else { return Vec::new() };
    let v = drag.value;
    let changed: Vec<ZoneEdit> = match drag.part {
        Part::Low => state.key_peers(z).into_iter().map(|o| ZoneEdit { keys: [v.min(z.keys[1]), z.keys[1]], ..ZoneEdit::of(o) }).collect(),
        Part::High => state.key_peers(z).into_iter().map(|o| ZoneEdit { keys: [z.keys[0], v.max(z.keys[0])], ..ZoneEdit::of(o) }).collect(),
        Part::Top => state
            .layer_peers(z)
            .into_iter()
            .map(|o| ZoneEdit { velocities: [z.velocities[0], v.clamp(z.velocities[0].max(1), 127)], ..ZoneEdit::of(o) })
            .collect(),
        Part::Root => vec![ZoneEdit { root: v.min(127), ..ZoneEdit::of(z) }],
    };
    changed.into_iter().filter(|e| state.zone(e.zone).is_some_and(|o| ZoneEdit::of(o) != *e)).collect()
}

/// The command's parameters for `edits` (the track is added when it is sent).
pub fn params(edits: &[ZoneEdit]) -> Value {
    let zones: Vec<Value> = edits
        .iter()
        .map(|e| {
            let m = e.membership.map(|m| json!({"group": m.group, "layer": m.layer, "alternative": m.alternative}));
            json!({"zone": e.zone, "keys": e.keys, "velocities": e.velocities, "root": e.root, "membership": m})
        })
        .collect();
    json!({ "zones": zones })
}

/// Whether storing `edits` would be refused, by the checks the command's store makes (see the
/// module notes): the reason, for the preview's tooltip.
pub fn conflict(state: &State, edits: &[ZoneEdit], session: &transit_model::Session, names: transit_midi::MiddleC) -> Option<String> {
    let mut p = state.patch.clone();
    for e in edits {
        if let Some(z) = p.zones.iter_mut().find(|z| z.id + 1 == e.zone) {
            z.keys = e.keys;
            z.velocities = e.velocities;
            z.root = e.root;
        }
    }
    refusal(state, &p, session, names)
}

/// Whether storing `p` (the stored patch with a change applied) would be refused by the checks
/// the commands' store runs: the reason.
fn refusal(
    state: &State,
    p: &transit_model::sampler_patch::PatchV1,
    session: &transit_model::Session,
    names: transit_midi::MiddleC,
) -> Option<String> {
    if let Err(r) = p.check_groups() {
        return Some(r.to_string());
    }
    if let Err(m) = transit_mix::check_sampler_patch(p, session) {
        return Some(m);
    }
    p.layer_overload(&state.patch).map(|o| o.text(names))
}

/// Whether removing zone `zone` (from 1) would be refused, by the same checks (for the remove
/// question's words; the command is still sent and the engine decides).
pub fn removal(state: &State, zone: u16, session: &transit_model::Session, names: transit_midi::MiddleC) -> Option<String> {
    let mut p = state.patch.clone();
    p.zones.retain(|z| z.id + 1 != zone);
    if let Some(pad) = p.pad_of(zone - 1) {
        p.set_pad(transit_model::sampler_patch::PadEntry { zone: None, ..p.pad(pad) });
    }
    refusal(state, &p, session, names)
}

//! Zone selection for a note-on (ADR 0041, "Playback, controls and modulation"): which unbound
//! zones the key scan selects for a note's key and velocity.
//!
//! Unbound Instrument zones select by inclusive original key and velocity. A pad-bound zone is
//! selected only through its pad, never a second time through this scan.

use crate::patch::Zone;

/// Whether the unbound-zone scan selects a zone for `key` and `velocity`: `bound` is the zone's
/// entry in the pad binding table (selected only when the zone is not bound to a pad) and `zone`
/// the zone, if set; its inclusive key and velocity ranges must hold the note, and of a round-robin
/// group only alternative 0 plays.
pub(crate) fn unbound_selected(bound: Option<&Option<u8>>, zone: Option<&Zone>, key: u8, velocity: u8) -> bool {
    bound.is_some_and(Option::is_none)
        && zone.is_some_and(|zone| {
            (zone.keys.0..=zone.keys.1).contains(&key)
                && (zone.velocities.0..=zone.velocities.1).contains(&velocity)
                && zone.membership.is_none_or(|m| m.alternative == 0)
        })
}

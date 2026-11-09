//! Zone selection for a note-on (ADR 0041, "Playback, controls and modulation", and its amendment
//! of 2026-11-08, items 4 and 10): which zones the key scan selects for a note's key and velocity,
//! and which round-robin alternative a key-map group plays.
//!
//! Unbound Instrument zones select by inclusive original key and velocity. A pad-bound zone is
//! selected only through its pad, never a second time through this scan.
//!
//! **Groups.** A key-map group has one key range, up to [`LAYERS`] velocity layers, and each layer
//! one to [`ALTERNATIVES`] mutually exclusive round-robin alternatives with identical ranges,
//! numbered contiguously from 0 ([`check_groups`]). When the scan reaches a group's first zone (in
//! zone order) whose key range holds the note, the group's layers whose velocity range holds the
//! note are admitted in ascending layer order, while the note's layer bound
//! ([`crate::LAYERS_PER_NOTE`]) allows; each admitted layer plays exactly one alternative:
//!
//! - **Sequential** (the default, and the mode of a group without a stored entry): each (group,
//!   layer) has a counter starting at 0; the alternative is `counter % count`, and the counter
//!   advances once per admitted layer trigger.
//! - **Random**: with `s` the normalized seed (0 is 1) and `g` the group id, the group's state starts
//!   at `s ^ (g + 1).wrapping_mul(0x9E3779B9)` in 32-bit unsigned arithmetic, and at 1 when that
//!   is 0 (xorshift32's fixed point). Each admitted layer trigger draws once: one xorshift32 step
//!   (13, 17, 5) on the group's state, the alternative being `word % count`. Several layers of one
//!   group admitted by one note draw in ascending layer order.
//!
//! An admitted layer advances its counter or draws even when its zone's start is then refused (a
//! silent missing-media zone), so relinking does not reorder the other alternatives. A layer the
//! note's layer bound leaves out is not admitted and advances nothing. Every counter and every
//! group's random state restart on [`crate::SamplerCore::reset`], on prepare, and at each transport
//! transition between playing and stopped (see [`crate::SamplerCore::process`]); a locate, a cycle
//! wrap and a patch edit do not restart them. This derivation is part of the file format.

use crate::patch::{GroupMode, PatchError, Zone};
use crate::{ALTERNATIVES, GROUPS, LAYERS, ZONES};

/// Whether `zone` holds `key` and `velocity` in its inclusive ranges.
pub(crate) fn holds(zone: &Zone, key: u8, velocity: u8) -> bool {
    (zone.keys.0..=zone.keys.1).contains(&key) && (zone.velocities.0..=zone.velocities.1).contains(&velocity)
}

/// Whether the unbound-zone scan selects a zone without group membership for `key` and
/// `velocity`: `bound` is the zone's entry in the pad binding table (selected only when the zone is
/// not bound to a pad) and `zone` the zone, if set. Group members select through their group.
pub(crate) fn unbound_selected(bound: Option<&Option<u8>>, zone: Option<&Zone>, key: u8, velocity: u8) -> bool {
    bound.is_some_and(Option::is_none) && zone.is_some_and(|zone| zone.membership.is_none() && holds(zone, key, velocity))
}

/// No zone in a member table entry.
pub(crate) const NONE: u16 = u16::MAX;

/// The member table entry of group `group`, layer `layer`.
pub(crate) fn entry(group: u8, layer: u8) -> usize {
    usize::from(group) * LAYERS + usize::from(layer)
}

/// The alternatives present from 0 in a member table entry.
pub(crate) fn count(alternatives: &[u16; ALTERNATIVES]) -> u8 {
    alternatives.iter().take_while(|&&z| z != NONE).count() as u8
}

/// Fills the member table (`GROUPS * LAYERS` entries, each the zone index of every alternative)
/// from the zones. No allocation.
pub(crate) fn rebuild<'a>(members: &mut [[u16; ALTERNATIVES]], zones: impl Iterator<Item = Option<&'a Zone>>) {
    members.iter_mut().for_each(|e| *e = [NONE; ALTERNATIVES]);
    for (i, z) in zones.enumerate() {
        if let Some(m) = z.and_then(|z| z.membership)
            && let Some(slot) = members.get_mut(entry(m.group, m.layer)).and_then(|e| e.get_mut(usize::from(m.alternative)))
        {
            *slot = i as u16;
        }
    }
}

/// What the shape check reads of a member zone: its membership's group, layer and alternative,
/// its key range and its velocity range.
pub(crate) type Shape = ((u8, u8, u8), (u8, u8), (u8, u8));

/// The shape check reads of zone `z`, if it is a member.
pub(crate) fn shape(z: &Zone) -> Option<Shape> {
    z.membership.map(|m| ((m.group, m.layer, m.alternative), z.keys, z.velocities))
}

/// Checks every key-map group's shape, `at(i)` being zone `i`'s [`Shape`] when it is a member:
/// one key range per group, identical velocity ranges among a layer's alternatives, at most
/// [`LAYERS`] layers and [`ALTERNATIVES`] alternatives per layer, alternatives unique and
/// contiguous from 0. No allocation.
pub(crate) fn check_groups(at: impl Fn(usize) -> Option<Shape>) -> Result<(), PatchError> {
    for i in 0..ZONES {
        let Some(((g, l, a), keys, velocities)) = at(i) else { continue };
        if usize::from(l) >= LAYERS || usize::from(a) >= ALTERNATIVES {
            return Err(PatchError::Range);
        }
        let mut first_of_layer = true;
        let mut mask = 1u16 << a;
        for j in (0..ZONES).filter(|&j| j != i) {
            let Some(((h, m, b), k2, v2)) = at(j) else { continue };
            if h != g {
                continue;
            }
            if k2 != keys {
                return Err(PatchError::GroupShape);
            }
            if m != l {
                continue;
            }
            if v2 != velocities || b == a || usize::from(b) >= ALTERNATIVES {
                return Err(PatchError::GroupShape);
            }
            first_of_layer &= j > i;
            mask |= 1 << b;
        }
        if first_of_layer && mask & (mask + 1) != 0 {
            return Err(PatchError::GroupShape);
        }
    }
    Ok(())
}

/// One xorshift32 step (13, 17, 5).
pub(crate) fn xorshift32(mut x: u32) -> u32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

/// Group `group`'s random state at a restart, for the patch seed `seed` (0 normalizes to 1).
pub(crate) fn start_state(seed: u32, group: u8) -> u32 {
    let s = seed.max(1);
    let x = s ^ (u32::from(group) + 1).wrapping_mul(0x9E37_79B9);
    if x == 0 { 1 } else { x }
}

/// The least common multiple of the alternative counts 1 to 8: a counter kept modulo it gives the
/// same `counter % count` as an unbounded one for every count.
const CYCLE: u16 = 840;

/// The round-robin state: each group's mode, each (group, layer)'s sequential counter and each
/// group's random state. Fixed arrays; nothing here allocates.
#[derive(Debug, Clone)]
pub(crate) struct RoundRobin {
    seed: u32,
    modes: [GroupMode; GROUPS],
    counters: [u16; GROUPS * LAYERS],
    random: [u32; GROUPS],
}

impl RoundRobin {
    pub(crate) fn new(seed: u32) -> RoundRobin {
        let mut r = RoundRobin { seed: seed.max(1), modes: [GroupMode::Sequential; GROUPS], counters: [0; GROUPS * LAYERS], random: [0; GROUPS] };
        r.restart();
        r
    }

    /// A new patch seed (normalized: 0 is 1); takes effect at the next [`RoundRobin::restart`].
    pub(crate) fn set_seed(&mut self, seed: u32) {
        self.seed = seed.max(1);
    }

    pub(crate) fn set_modes(&mut self, modes: &[GroupMode; GROUPS]) {
        self.modes = *modes;
    }

    /// Every counter back to 0 and every group's random state to its start state.
    pub(crate) fn restart(&mut self) {
        self.counters = [0; GROUPS * LAYERS];
        for (g, x) in self.random.iter_mut().enumerate() {
            *x = start_state(self.seed, g as u8);
        }
    }

    /// The alternative an admitted trigger of `group`'s `layer` plays among `count` (1 to 8), and
    /// the advance: the layer's counter or one draw of the group's state.
    pub(crate) fn choose(&mut self, group: u8, layer: u8, count: u8) -> u8 {
        let count = count.max(1);
        let g = usize::from(group);
        match self.modes.get(g).copied().unwrap_or_default() {
            GroupMode::Sequential => {
                let Some(c) = self.counters.get_mut(entry(group, layer)) else { return 0 };
                let alt = (*c % u16::from(count)) as u8;
                *c = (*c + 1) % CYCLE;
                alt
            }
            GroupMode::Random => {
                let Some(x) = self.random.get_mut(g) else { return 0 };
                *x = xorshift32(*x);
                (*x % u32::from(count)) as u8
            }
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn fingerprint(&self, h: &mut dyn core::hash::Hasher) {
        h.write_u32(self.seed);
        for m in &self.modes {
            h.write_u8(*m as u8);
        }
        for c in &self.counters {
            h.write_u16(*c);
        }
        for x in &self.random {
            h.write_u32(*x);
        }
    }
}

//! The Sampler's key-map groups and the voices one note starts (ADR 0041 body "Playback, controls
//! and modulation" and "Capacities and admission"; amendment 2026-11-09, refusal (c)).
//!
//! - **Group shape** ([`PatchV1::check_groups`]): a group has one key range; at most
//!   [`LAYERS`] velocity layers; each layer one to [`ALTERNATIVES`] mutually exclusive round-robin
//!   alternatives, numbered up from 0 without a gap, all with the layer's one velocity range; no
//!   two zones in the same place; and no pad's zone among the members. The patch validator makes
//!   this check, so a badly shaped file loads opaque with a note instead of loading ready and
//!   playing silent (the core refuses such a publication); the edit commands make it before they
//!   store, so they refuse with these texts. A member whose media is missing is not a shape fault.
//! - **Voices per note** ([`PatchV1::note_voices`], [`PatchV1::layer_overload`]): the one
//!   definition of "at most eight layer voices start for one note". For a note and a velocity
//!   1 to 127, a pad bound to a zone and playing that note counts one, and each zone no pad binds
//!   counts one when its key range holds the note, its velocity range holds the velocity and it has
//!   no layer membership or is its layer's alternative 0 (round-robin alternatives of one layer are
//!   one voice, as the core selects them). An edit is refused when it leaves some note and velocity
//!   with more than [`LAYERS_PER_NOTE`] voices and more than the patch had before the edit: a map
//!   loaded over the bound plays (the core's runtime cut is the backstop), and an edit of something
//!   else is never refused because of it ("checked when set").
//! - **Texts** name zones as the panel numbers them (zone 1 to 256), groups, layers and
//!   alternatives as the commands take them (from 0), pads by bank and slot, and notes through the
//!   shared namer in the convention the caller passes (ADR 0051).

use crate::pad_notes::{LAYERS_PER_NOTE, pad_label};
use crate::sampler_patch::{ALTERNATIVES, LAYERS, PADS, PatchV1, ZONES};
use std::collections::BTreeMap;
use transit_midi::{MiddleC, note_name};

/// Why a patch's key-map groups are refused (an edit) or kept opaque (a load).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRefusal {
    /// Group `group` would have `layers` velocity layers (counted to its highest layer).
    Layers { group: u8, layers: u16 },
    /// Layer `layer` of group `group` would have `count` alternatives (counted to its highest).
    Alternatives { group: u8, layer: u8, count: u16 },
    /// Zone `zone` has another key range than zone `first`, the group's first member.
    KeyRange { group: u8, zone: u16, first: u16 },
    /// Zone `zone` has another velocity range than zone `first`, the layer's first member.
    VelocityRange { group: u8, layer: u8, zone: u16, first: u16 },
    /// Layer `layer` of group `group` has no alternative `missing`, below one it has.
    Gap { group: u8, layer: u8, missing: u8 },
    /// Zones `first` and `zone` are both this alternative.
    Duplicate { group: u8, layer: u8, alternative: u8, first: u16, zone: u16 },
    /// Zone `zone`, a member of group `group`, is bound to pad `pad`.
    PadBound { group: u8, zone: u16, pad: u8 },
}

impl std::fmt::Display for GroupRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            GroupRefusal::Layers { group, layers } => write!(f, "group {group} would have {layers} layers (at most {LAYERS})"),
            GroupRefusal::Alternatives { group, layer, count: 9 } => {
                write!(f, "group {group} layer {layer} would have a ninth alternative (at most {ALTERNATIVES})")
            }
            GroupRefusal::Alternatives { group, layer, count } => {
                write!(f, "group {group} layer {layer} would have {count} alternatives (at most {ALTERNATIVES})")
            }
            GroupRefusal::KeyRange { group, zone, first } => {
                write!(f, "group {group}: zone {} has another key range than zone {} (a group has one key range)", zone + 1, first + 1)
            }
            GroupRefusal::VelocityRange { group, layer, zone, first } => write!(
                f,
                "group {group} layer {layer}: zone {} has another velocity range than zone {} (a layer's alternatives share one)",
                zone + 1,
                first + 1
            ),
            GroupRefusal::Gap { group, layer, missing } => {
                write!(f, "group {group} layer {layer} has no alternative {missing} (alternatives count up from 0 without a gap)")
            }
            GroupRefusal::Duplicate { group, layer, alternative, first, zone } => {
                write!(f, "zones {} and {} are both group {group} layer {layer} alternative {alternative}", first + 1, zone + 1)
            }
            GroupRefusal::PadBound { group, zone, pad } => {
                write!(f, "zone {} is pad {}'s zone: a pad's zone cannot be a member of group {group}", zone + 1, pad_label(pad))
            }
        }
    }
}

/// One group as the shape check reads it: its first member and key range, and per layer the first
/// member, its velocity range and the zone holding each alternative.
struct Shape {
    keys: ([u8; 2], u16),
    layers: [Option<([u8; 2], u16, [Option<u16>; ALTERNATIVES as usize])>; LAYERS as usize],
}

/// What makes one voice of a note: a pad, a zone outside any group, or one layer of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Pad(u8),
    Zone(u16),
    Layer(u8, u8),
}

/// The voice an edit would add past the bound, as its refusal names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// A pad (0-based index).
    Pad(u8),
    /// A zone (0-based id).
    Zone(u16),
}

/// An edit that would start more than [`LAYERS_PER_NOTE`] voices for one note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerOverload {
    pub note: u8,
    /// The first velocity at which it happens.
    pub velocity: u8,
    /// The voices the note would start at that velocity.
    pub voices: u8,
    /// The pad or zone that the edit added to that note.
    pub by: Voice,
}

impl LayerOverload {
    /// The refusal's text, with the note named in convention `names` (the pad wording is the
    /// amendment's, `Pad A5 would be a ninth layer on E3.`).
    #[must_use]
    pub fn text(&self, names: MiddleC) -> String {
        let note = note_name(self.note, names);
        match self.by {
            Voice::Pad(p) => format!("Pad {} would be a ninth layer on {note}.", pad_label(p)),
            Voice::Zone(z) => format!("Zone {} would be a ninth layer on {note}.", z + 1),
        }
    }
}

impl PatchV1 {
    /// Checks every key-map group's shape (see the module doc); the first fault in zone order.
    pub fn check_groups(&self) -> Result<(), GroupRefusal> {
        let mut groups: BTreeMap<u8, Shape> = BTreeMap::new();
        for z in &self.zones {
            let Some(m) = &z.membership else { continue };
            let (group, layer, alternative) = (m.group, m.layer, m.alternative);
            if let Some(pad) = self.pad_of(z.id) {
                return Err(GroupRefusal::PadBound { group, zone: z.id, pad });
            }
            if layer >= LAYERS {
                return Err(GroupRefusal::Layers { group, layers: u16::from(layer) + 1 });
            }
            if alternative >= ALTERNATIVES {
                return Err(GroupRefusal::Alternatives { group, layer, count: u16::from(alternative) + 1 });
            }
            let shape = groups.entry(group).or_insert(Shape { keys: (z.keys, z.id), layers: [None; LAYERS as usize] });
            if shape.keys.0 != z.keys {
                return Err(GroupRefusal::KeyRange { group, zone: z.id, first: shape.keys.1 });
            }
            let Some(slot) = shape.layers.get_mut(usize::from(layer)) else { continue };
            let (velocities, first, alternatives) = slot.get_or_insert((z.velocities, z.id, [None; ALTERNATIVES as usize]));
            if *velocities != z.velocities {
                return Err(GroupRefusal::VelocityRange { group, layer, zone: z.id, first: *first });
            }
            match alternatives.get_mut(usize::from(alternative)) {
                Some(Some(other)) => return Err(GroupRefusal::Duplicate { group, layer, alternative, first: *other, zone: z.id }),
                Some(held) => *held = Some(z.id),
                None => {}
            }
        }
        for (group, shape) in &groups {
            for (layer, slot) in shape.layers.iter().enumerate() {
                let Some((_, _, alternatives)) = slot else { continue };
                let count = alternatives.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
                if let Some(missing) = alternatives.iter().take(count).position(Option::is_none) {
                    return Err(GroupRefusal::Gap { group: *group, layer: layer as u8, missing: missing as u8 });
                }
            }
        }
        Ok(())
    }

    /// The zones no pad binds, by id.
    fn unbound_mask(&self) -> [bool; ZONES] {
        let mut bound = [false; ZONES];
        for p in &self.pads {
            if let Some(b) = p.zone.and_then(|z| bound.get_mut(usize::from(z))) {
                *b = true;
            }
        }
        bound.map(|b| !b)
    }

    /// The voices note `note` starts at each velocity (index 1 to 127; index 0 is unused), as the
    /// module doc defines them.
    #[must_use]
    pub fn note_voices(&self, note: u8) -> [u8; 128] {
        self.voice_table()[usize::from(note)]
    }

    /// [`Self::note_voices`] of every note, by note then velocity: each counting zone adds one over
    /// its velocity range on every note of its key range (a running sum per note), so the table
    /// costs the zones' key spans, not notes times velocities times zones.
    fn voice_table(&self) -> Box<[[u8; 128]; 128]> {
        let unbound = self.unbound_mask();
        // Per note, +1 at the low velocity and -1 past the high one.
        let mut steps = Box::new([[0i16; 129]; 128]);
        for z in &self.zones {
            let counted = unbound.get(usize::from(z.id)).copied().unwrap_or(false) && z.membership.as_ref().is_none_or(|m| m.alternative == 0);
            let [klo, khi] = z.keys;
            let [vlo, vhi] = z.velocities;
            if !counted || klo > khi || khi > 127 || vlo > vhi || vhi > 127 {
                continue;
            }
            for row in steps.iter_mut().take(usize::from(khi) + 1).skip(usize::from(klo)) {
                row[usize::from(vlo.max(1))] += 1;
                row[usize::from(vhi) + 1] -= 1;
            }
        }
        for i in 0..PADS as u8 {
            if self.pad(i).zone.is_some()
                && let Some(row) = steps.get_mut(usize::from(self.pad_note(i)))
            {
                row[1] += 1;
                row[128] -= 1;
            }
        }
        let mut table = Box::new([[0u8; 128]; 128]);
        for (row, step) in table.iter_mut().zip(steps.iter()) {
            let mut run = 0i16;
            for (c, s) in row.iter_mut().zip(step.iter()).skip(1) {
                run += s;
                *c = u8::try_from(run.max(0)).unwrap_or(u8::MAX);
            }
        }
        table
    }

    /// The voices `note` starts at `velocity`, each as what makes it one, with a zone it names.
    fn units(&self, note: u8, velocity: u8) -> Vec<(Unit, u16)> {
        let unbound = self.unbound_mask();
        let mut out = Vec::new();
        if let Some(i) = (0..PADS as u8).find(|i| self.pad(*i).zone.is_some() && self.pad_note(*i) == note) {
            out.push((Unit::Pad(i), 0));
        }
        for z in &self.zones {
            let counted = unbound.get(usize::from(z.id)).copied().unwrap_or(false)
                && (z.keys[0]..=z.keys[1]).contains(&note)
                && (z.velocities[0]..=z.velocities[1]).contains(&velocity)
                && z.membership.as_ref().is_none_or(|m| m.alternative == 0);
            if counted {
                let unit = z.membership.as_ref().map_or(Unit::Zone(z.id), |m| Unit::Layer(m.group, m.layer));
                out.push((unit, z.id));
            }
        }
        out
    }

    /// Whether this patch (an edit's result) starts more than [`LAYERS_PER_NOTE`] voices for some
    /// note and velocity, and more than `before` (the patch before the edit) does there: the first
    /// such note and velocity, with the pad or zone the edit added to it. `None`: the edit keeps
    /// within the bound (or adds nothing to a note already over it).
    #[must_use]
    pub fn layer_overload(&self, before: &PatchV1) -> Option<LayerOverload> {
        let (now, then) = (self.voice_table(), before.voice_table());
        for (note, (after, prior)) in (0..=127u8).zip(now.iter().zip(then.iter())) {
            let hit = (1..=127u8).find(|&v| {
                let (a, b) = (after[usize::from(v)], prior[usize::from(v)]);
                usize::from(a) > LAYERS_PER_NOTE && a > b
            });
            if let Some(velocity) = hit {
                let by = self.added_voice(before, note, velocity);
                return Some(LayerOverload { note, velocity, voices: after[usize::from(velocity)], by });
            }
        }
        None
    }

    /// The pad or zone this patch has among `note`'s voices at `velocity` that `before` does not:
    /// the pad first, else the first new zone in zone order; for a new group layer, its zone that
    /// the edit changed or added (else the layer's alternative 0).
    fn added_voice(&self, before: &PatchV1, note: u8, velocity: u8) -> Voice {
        let old: Vec<Unit> = before.units(note, velocity).into_iter().map(|(u, _)| u).collect();
        let new = self.units(note, velocity);
        let Some(&(unit, zone)) = new.iter().find(|(u, _)| !old.contains(u)).or_else(|| new.first()) else { return Voice::Zone(0) };
        match unit {
            Unit::Pad(p) => Voice::Pad(p),
            Unit::Zone(z) => Voice::Zone(z),
            Unit::Layer(g, l) => {
                let changed =
                    self.zones.iter().find(|z| z.membership.as_ref().is_some_and(|m| m.group == g && m.layer == l) && before.zone(z.id) != Some(*z));
                Voice::Zone(changed.map_or(zone, |z| z.id))
            }
        }
    }
}

#[cfg(test)]
#[path = "sampler_groups_tests.rs"]
mod tests;

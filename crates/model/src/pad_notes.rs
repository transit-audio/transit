//! The Sampler's pad start note (ADR 0041, "Amendment 2026-11-09: the pad start note").
//!
//! - **Effective notes.** A pad with a stored trigger note plays that note, absolute; a pad
//!   without one plays `pad_start_note + index` ([`PatchV1::pad_note`]). This is the one
//!   accessor for "the note this pad plays now": the link to the core, the commands, the state
//!   query, Slices to pads and the Pads view all read it.
//! - **The slice mapping root.** A loop without a stored root uses the pad start note
//!   ([`PatchV1::slice_root`]), so a default Slices to pads run puts slice 1 on pad A1.
//! - **Changing the start note** ([`PatchV1::set_pad_start`]) moves only the pads without a stored
//!   note, adds the change to the root of every zone bound to such a pad (so no pad changes
//!   pitch), and refuses, leaving the patch as it was, when a moved pad would take a stored
//!   pad's note, start a ninth layer voice on its note, or put a root outside 0 to 127.
//! - **Texts.** A refusal or notice that names a note names it through the shared namer in the
//!   convention its caller passes (the engine's message Middle C, ADR 0051); the values stay MIDI
//!   numbers.

use crate::sampler_patch::{PADS, PatchV1};
use transit_midi::{MiddleC, note_name};

/// The pad start note every patch-creating path stores (the drum convention's first note).
pub const NEW_PAD_START: u8 = 36;
/// The pad start note of a patch saved without the field (what such a patch meant).
pub const SAVED_PAD_START: u8 = 60;
/// The highest pad start note, so that the start note plus 63 stays within 127.
pub const MAX_PAD_START: u8 = 64;
/// Most voices one note starts, a pad triggered by the note counting as one (the core's bound).
pub const LAYERS_PER_NOTE: usize = 8;

/// Pad `index`'s name as the Pads view numbers it: the bank letter and the slot, `A1` to `D16`.
#[must_use]
pub fn pad_label(index: u8) -> String {
    let bank = char::from(b'A'.saturating_add(index / 16));
    format!("{bank}{}", index % 16 + 1)
}

/// Why a pad start note change or a pad note edit is refused (the patch is unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadRefusal {
    /// (a) The start note is not an integer from 0 to 64.
    StartRange,
    /// (b) Defaulted pad `defaulted` would take stored pad `stored`'s note `note` at start `start`.
    Taken { stored: u8, note: u8, start: u8, defaulted: u8 },
    /// (c) Bound pad `pad` on note `note` would start more than eight voices for some velocity.
    NinthLayer { pad: u8, note: u8 },
    /// (d) The root of the zone bound to pad `pad` would leave 0 to 127.
    RootRange { pad: u8 },
}

impl PadRefusal {
    /// The refusal's text, with note names in convention `names`.
    #[must_use]
    pub fn text(&self, names: MiddleC) -> String {
        match *self {
            PadRefusal::StartRange => "note must be an integer from 0 to 64".to_string(),
            PadRefusal::Taken { stored, note, start, defaulted } => format!(
                "Pad {} plays {}: a start note of {} would put pad {} there.",
                pad_label(stored),
                note_name(note, names),
                note_name(start, names),
                pad_label(defaulted)
            ),
            PadRefusal::NinthLayer { pad, note } => format!("Pad {} would be a ninth layer on {}.", pad_label(pad), note_name(note, names)),
            PadRefusal::RootRange { pad } => format!("Pad {}'s root would leave 0 to 127.", pad_label(pad)),
        }
    }
}

/// A pad start note change that was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartChange {
    pub from: u8,
    pub to: u8,
}

impl StartChange {
    /// How far the defaulted pads moved, in semitones (negative: down).
    #[must_use]
    pub fn semitones(&self) -> i16 {
        i16::from(self.to) - i16::from(self.from)
    }

    /// The status notice: the new start note and the distance to transpose pad clips by, with
    /// note names in convention `names`.
    #[must_use]
    pub fn notice(&self, names: MiddleC) -> String {
        let d = self.semitones();
        let name = note_name(self.to, names);
        if d == 0 {
            return format!("Pad start note {name} (unchanged).");
        }
        let way = if d < 0 { "down" } else { "up" };
        let unit = if d.abs() == 1 { "semitone" } else { "semitones" };
        format!("Pad start note {name} ({way} {} {unit}): transpose pad clips by {d:+} to keep them on the same pads.", d.abs())
    }
}

impl PatchV1 {
    /// The note pad `index` plays: its stored note, else the pad start note plus the index.
    #[must_use]
    pub fn pad_note(&self, index: u8) -> u8 {
        match self.pads.iter().find(|p| p.index == index) {
            Some(p) => p.trigger(self.pad_start_note),
            None => crate::sampler_patch::PadEntry::default_note(index, self.pad_start_note),
        }
    }

    /// Whether pad `index` has a stored trigger note (it then never moves with the start note).
    #[must_use]
    pub fn pad_note_stored(&self, index: u8) -> bool {
        self.pads.iter().any(|p| p.index == index && p.note.is_some())
    }

    /// The loop's slice mapping root: the stored one, else the pad start note (also without a
    /// loop, the root a new loop would start from).
    #[must_use]
    pub fn slice_root(&self) -> u8 {
        self.slicing.as_ref().and_then(|l| l.root).unwrap_or(self.pad_start_note)
    }

    /// The voices note `note` at velocity `velocity` starts in the unbound zones, as the core
    /// selects them: each zone that no pad binds, whose key and velocity ranges hold the note and
    /// the velocity, and that has no layer membership or is its layer's alternative 0.
    #[must_use]
    pub fn unbound_layers(&self, note: u8, velocity: u8) -> usize {
        self.zones
            .iter()
            .filter(|z| self.pad_of(z.id).is_none())
            .filter(|z| (z.keys[0]..=z.keys[1]).contains(&note) && (z.velocities[0]..=z.velocities[1]).contains(&velocity))
            .filter(|z| z.membership.as_ref().is_none_or(|m| m.alternative == 0))
            .count()
    }

    /// Refuses pad `pad` (bound to a zone) on note `note` when, for some velocity 1 to 127, the pad
    /// and the unbound zones would start more than [`LAYERS_PER_NOTE`] voices.
    pub fn check_pad_layers(&self, pad: u8, note: u8) -> Result<(), PadRefusal> {
        if (1..=127u8).any(|v| 1 + self.unbound_layers(note, v) > LAYERS_PER_NOTE) {
            return Err(PadRefusal::NinthLayer { pad, note });
        }
        Ok(())
    }

    /// Sets the pad start note to `start`: every pad without a stored note moves with it and the
    /// root of each zone bound to such a pad moves by the same distance; stored notes, the zones
    /// of their pads and unbound zones are unchanged. Refused, with the patch unchanged, in the
    /// amendment's order (a) to (d). An unchanged start note changes nothing.
    pub fn set_pad_start(&mut self, start: u8) -> Result<StartChange, PadRefusal> {
        if start > MAX_PAD_START {
            return Err(PadRefusal::StartRange);
        }
        let change = StartChange { from: self.pad_start_note, to: start };
        let d = change.semitones();
        if d == 0 {
            return Ok(change);
        }
        let defaulted: Vec<u8> = (0..PADS as u8).filter(|i| !self.pad_note_stored(*i)).collect();
        for &i in &defaulted {
            let note = start.saturating_add(i);
            if let Some(q) = self.pads.iter().find(|q| q.note == Some(note)) {
                return Err(PadRefusal::Taken { stored: q.index, note, start, defaulted: i });
            }
        }
        let bound: Vec<(u8, u16)> = defaulted.iter().filter_map(|&i| self.pad(i).zone.map(|z| (i, z))).collect();
        for &(i, _) in &bound {
            self.check_pad_layers(i, start.saturating_add(i))?;
        }
        let mut roots = Vec::with_capacity(bound.len());
        for &(i, z) in &bound {
            let root = self.zone(z).map_or(0, |z| i16::from(z.root)) + d;
            let root = u8::try_from(root).ok().filter(|r| *r <= 127).ok_or(PadRefusal::RootRange { pad: i })?;
            roots.push((z, root));
        }
        for (z, root) in roots {
            if let Some(z) = self.zone_mut(z) {
                z.root = root;
            }
        }
        self.pad_start_note = start;
        Ok(change)
    }
}

#[cfg(test)]
#[path = "pad_notes_tests.rs"]
mod tests;

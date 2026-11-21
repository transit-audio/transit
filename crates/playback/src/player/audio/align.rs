//! The output converter's coordinates for the live input (#709; ADR 0015, Amendment 2026-11-16 on
//! the converter term, rule 3): what [`crate::record::InputRing::pop_aligned`] needs to read a
//! block's input at the positions the term prescribes.

use super::Audio;
use crate::record::Coordinates;

impl Audio {
    /// The coordinates of the block about to render: `last`, the device callback's last output
    /// frame counted from the stream's start, sits `ahead` output frames after the next one, whose
    /// render position from the new block's frame 0 is the converter's phase. Valid while the
    /// converter's alignment is (rule 10: speed 1 since a restart at speed 1). `None` on an
    /// arithmetic overflow, which reads the input as before. Integer arithmetic only.
    pub(super) fn input_coordinates(&self, last: u64, ahead: usize, step: u64) -> Option<Coordinates> {
        let units = self.read.units_after(ahead, step)?;
        Some(Coordinates {
            output: last,
            units,
            den: self.read.denominator(),
            term: self.term,
            valid: self.transport.aligned && self.transport.speed == 1000,
        })
    }
}

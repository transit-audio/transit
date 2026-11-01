//! The texture synth played the way the Texture Synth instrument plays it (#241 slice 3): notes on
//! the SEED input and the PITCH CV, at 48 kHz in the module's 12-frame blocks.
//!
//! The synth sweep ([`crate::texture_sweep::synth_points`]) moves the knobs with the SEED button
//! latched, frozen, or a short pulse on the SEED input; it never patches the PITCH CV, holds the
//! SEED gate for a note's length, retriggers a held gate, or latches the button with the input
//! patched (the clock divider). These points do: a fixed melody ([`NOTES`]) whose notes hold the
//! SEED gate high for their length and set the PITCH CV (`has_cv`, from -60 to +60 semitones, so
//! the oscillator's and the grains' pitch reach the CV's whole range), with two legato steps (the
//! gate drops for one frame and rises again, as the instrument retriggers a held gate). Each point
//! ([`note_points`]) sets DENSITY (the regular train, the taps near noon, the random train), the
//! SEED button (released, held, latched: the divider), freeze over the middle second, the PITCH
//! knob and its randomness, DRY/WET and the monophonic mode, around the synth's center. A point is
//! rendered from a fresh synth seeded with [`crate::texture_sweep::SEED`], 3 s long, at each of
//! the four resolutions.

#![forbid(unsafe_code)]

use crate::texture::{Controls, Media, SeedButton};
use crate::texture_sweep::{SEED, synth_center};
use crate::{Error, Session};

/// The length of a note point, frames: 3 seconds at 48 kHz.
pub const NOTE_POINT_FRAMES: usize = 144_000;

/// The module's block at 48 kHz, frames.
pub const NOTE_BLOCK: usize = 12;

/// The melody: (first frame, end frame, PITCH CV in semitones), every frame on the 12-frame grid.
/// The third, fourth and sixth notes start where the one before ends (legato: a retrigger).
pub const NOTES: [(usize, usize, f32); 8] = [
    (0, 19_200, 21.0),
    (24_000, 38_400, -12.0),
    (38_400, 52_800, 45.0),
    (52_800, 60_000, -36.0),
    (72_000, 96_000, 7.0),
    (96_000, 98_400, 60.0),
    (108_000, 132_000, -60.0),
    (134_400, 136_800, 0.0),
];

/// Whether the SEED gate is high at frame `n`: inside a note, except on the first frame of a note
/// that starts where the one before ends (the gate drops for that frame, so the note rises again).
#[must_use]
pub fn note_gate(n: usize) -> bool {
    NOTES.iter().any(|&(start, end, _)| (start..end).contains(&n) && !(n == start && NOTES.iter().any(|&(_, e, _)| e == start)))
}

/// The PITCH CV at frame `n`: the latest note's that has started (it holds after the note ends),
/// 0 before the first.
#[must_use]
pub fn note_cv(n: usize) -> f32 {
    NOTES.iter().rev().find(|&&(start, _, _)| start <= n).map_or(0.0, |&(_, _, cv)| cv)
}

/// One note point: the synth's center with these changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotePoint {
    /// DENSITY.
    pub density: f32,
    /// The SEED button.
    pub button: SeedButton,
    /// Freeze from 1 s to 2 s.
    pub freeze: bool,
    /// The PITCH knob, semitones.
    pub pitch: f32,
    /// PITCH's attenuverter (its randomness: the CV is patched, but the synth adds it to the knob
    /// and treats the attenuverter as randomness).
    pub pitch_amount: f32,
    /// DRY/WET.
    pub dry_wet: f32,
    /// The monophonic grain mode.
    pub monophonic: bool,
}

impl NotePoint {
    const CENTER: Self =
        Self { density: 0.35, button: SeedButton::Off, freeze: false, pitch: 0.0, pitch_amount: 0.0, dry_wet: 0.5, monophonic: false };

    /// The controls of the block starting at frame `start`.
    #[must_use]
    pub fn controls(&self, start: usize) -> Controls {
        let mut c = synth_center();
        c.density = self.density;
        c.seed_button = self.button;
        c.pitch.base = self.pitch;
        c.pitch.attenuverter = self.pitch_amount;
        c.pitch.cv = note_cv(start);
        c.pitch.has_cv = true;
        c.dry_wet = self.dry_wet;
        c.monophonic = self.monophonic;
        c
    }

    /// Freeze at frame `n`.
    #[must_use]
    pub fn frozen(&self, n: usize) -> bool {
        self.freeze && (48_000..96_000).contains(&n)
    }

    /// The SEED input's gate flags for frames `start..start + out.len()`.
    pub fn seed(start: usize, out: &mut [u8]) {
        for (i, flags) in out.iter_mut().enumerate() {
            let n = start + i;
            let (now, before) = (note_gate(n), n > 0 && note_gate(n - 1));
            *flags = match (now, before) {
                (true, false) => 3,
                (true, true) => 1,
                (false, true) => 4,
                (false, false) => 0,
            };
        }
    }

    /// A stable label.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "notes den{:.2} {:?} freeze{} pitch{} pa{:.2} wet{:.2} mono{}",
            self.density, self.button, self.freeze, self.pitch, self.pitch_amount, self.dry_wet, self.monophonic
        )
    }
}

/// The 18 note points, in a fixed order: DENSITY at five levels (the regular train at 0.2 and
/// 0.35, the taps at noon, the random train at 0.65 and 0.9) with the button released and latched;
/// the button held; freeze over the middle second (released and latched); the PITCH knob at -7
/// with smooth and per-grain randomness; fully dry and fully wet; the monophonic mode.
#[must_use]
pub fn note_points() -> Vec<NotePoint> {
    let c = NotePoint::CENTER;
    let mut points = Vec::new();
    for button in [SeedButton::Off, SeedButton::Locked] {
        for density in [0.2, 0.35, 0.5, 0.65, 0.9] {
            points.push(NotePoint { density, button, ..c });
        }
    }
    points.push(NotePoint { button: SeedButton::On, ..c });
    points.push(NotePoint { density: 0.65, freeze: true, ..c });
    points.push(NotePoint { button: SeedButton::Locked, freeze: true, ..c });
    points.push(NotePoint { pitch: -7.0, pitch_amount: 0.5, ..c });
    points.push(NotePoint { pitch: -7.0, pitch_amount: -0.5, ..c });
    points.push(NotePoint { density: 0.9, dry_wet: 0.0, ..c });
    points.push(NotePoint { density: 0.9, dry_wet: 1.0, ..c });
    points.push(NotePoint { density: 0.9, monophonic: true, ..c });
    points
}

/// Renders one note point from a fresh synth at `resolution` (0 to 3), seeded with [`SEED`], in
/// 12-frame blocks for [`NOTE_POINT_FRAMES`].
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_note_point(session: &Session, resolution: usize, point: &NotePoint) -> Result<Vec<[f32; 2]>, Error> {
    session.seed(SEED);
    let mut synth = session.texture_synth()?;
    debug_assert_eq!(Media::Digital.block_size(), NOTE_BLOCK);
    let mut out = vec![[0.0_f32; 2]; NOTE_POINT_FRAMES];
    let mut freeze = [false; NOTE_BLOCK];
    let mut seed = [0_u8; NOTE_BLOCK];
    for (b, o) in out.chunks_mut(NOTE_BLOCK).enumerate() {
        let start = b * NOTE_BLOCK;
        for (k, f) in freeze.iter_mut().enumerate() {
            *f = point.frozen(start + k);
        }
        NotePoint::seed(start, &mut seed);
        synth.render(&point.controls(start), resolution, &freeze, Some(&seed), o)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_points_have_the_documented_shape() {
        let points = note_points();
        assert_eq!(points.len(), 18);
        let labels: std::collections::HashSet<_> = points.iter().map(NotePoint::label).collect();
        assert_eq!(labels.len(), 18, "every point is rendered once");
        for p in &points {
            for n in (0..NOTE_POINT_FRAMES).step_by(NOTE_BLOCK) {
                assert!(crate::texture::controls_in_range(&p.controls(n)), "{} at {n}", p.label());
            }
        }
        assert!(NOTES.iter().all(|&(s, e, _)| s % NOTE_BLOCK == 0 && e % NOTE_BLOCK == 0 && s < e && e <= NOTE_POINT_FRAMES));
    }

    #[test]
    fn the_gate_holds_for_each_note_and_drops_for_one_frame_at_a_legato_step() {
        let mut flags = [0_u8; NOTE_BLOCK];
        NotePoint::seed(0, &mut flags);
        assert_eq!(flags[0], 3, "the first note rises at frame 0");
        assert!(flags[1..].iter().all(|&f| f == 1));
        NotePoint::seed(19_200, &mut flags);
        assert_eq!(flags[0], 4, "falls at the note's end");
        NotePoint::seed(38_400, &mut flags);
        assert_eq!((flags[0], flags[1], flags[2]), (4, 3, 1), "legato: low for one frame, then rising");
        assert_eq!(note_cv(38_399), -12.0);
        assert_eq!(note_cv(38_400), 45.0);
        assert_eq!(note_cv(140_000), 0.0);
        assert_eq!(note_cv(100_000), 60.0, "the CV holds after the note ends");
    }
}

//! The fixed sweep every resonator model and polyphony is rendered with (ADR 0033 gate, #198).
//!
//! A point is rendered from a fresh part, after seeding the random state with [`SEED`], at 48 kHz in
//! the firmware's 24-frame blocks, for [`POINT_FRAMES`] frames (100 ms). Fresh state per point keeps a
//! residual local: a failing point names its parameters, not the history before it.
//!
//! Points, per model and polyphony (1, 2, 3 and 4; the panel offers 1, 2 and 4, the easter-egg path
//! sets 3, and odd polyphonies take their own voice-rotation pattern):
//!
//! - for each note in [`NOTES`] and each [`Exciter`], structure, brightness, damping and position
//!   are swept one at a time over [`LEVELS`] with the other three at 0.5; the all-0.5 center is
//!   rendered once: 3 × 3 × 17 = 153 points;
//! - for each note, the internally excited center with the FM input at each of [`FMS`]: 6 points.
//!
//! 159 points in all. The chord of the quantized sympathetic strings follows structure as the
//! module's CV scaler rounds it (`structure × 10 + 0.5`, truncated), so the structure sweep walks the
//! chord table. The port (#198) renders the same points through [`Point::inputs`] and
//! [`Point::input_signal`], so both sides see the same values block for block.

#![forbid(unsafe_code)]

use crate::resonator::{PerformanceState, RESONATOR_BLOCK_SIZE, ResonatorModel, ResonatorPatch};
use crate::{Error, Session};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0198_5EED;

/// Notes every point is rendered at (MIDI semitones: tonic plus V/OCT).
pub const NOTES: [f32; 3] = [36.0, 60.0, 84.0];

/// The tonic (the frequency knob's `12 + transpose`); the V/OCT input carries the rest of the note.
pub const TONIC: f32 = 36.0;

/// Values each macro is swept over. The top is 0.9995, the module's structure ceiling, so no macro
/// reads past an `Interpolate` table (`crate::resonator::inputs_in_range`).
pub const LEVELS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 0.9995];

/// FM input values (semitones) rendered at each note's internally excited center.
pub const FMS: [f32; 2] = [-7.5, 12.25];

/// Polyphonies every model is rendered at.
pub const POLYPHONIES: [usize; 4] = [1, 2, 3, 4];

/// Frames per point: 100 ms at 48 kHz, 200 blocks.
pub const POINT_FRAMES: usize = 4800;

/// How a point excites the part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exciter {
    /// Nothing in the audio input: the part's own exciter (a pulse for the modal model, a noise
    /// burst for the strings, the FM voice's envelope), strummed on the first block and halfway.
    Internal,
    /// An impulse in the audio input on the first block and halfway (decaying by 0.99 per sample),
    /// never strummed: the first voice rings alone.
    External,
    /// The same input, strummed with each impulse, so the voices rotate.
    ExternalStrummed,
}

impl Exciter {
    /// Every exciter, in sweep order.
    pub const ALL: [Self; 3] = [Self::Internal, Self::External, Self::ExternalStrummed];

    fn label(self) -> &'static str {
        match self {
            Self::Internal => "int",
            Self::External => "ext",
            Self::ExternalStrummed => "ext-strum",
        }
    }
}

/// One point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Pitch, MIDI semitones.
    pub note: f32,
    /// The four macros.
    pub patch: ResonatorPatch,
    /// How the part is excited.
    pub exciter: Exciter,
    /// The FM input, semitones.
    pub fm: f32,
}

impl Point {
    /// Whether block `block` of a point `blocks` long carries a strum or an impulse.
    fn fires(block: usize, blocks: usize) -> bool {
        block == 0 || block == blocks / 2
    }

    /// The chord the module's CV scaler picks for this structure (without its hysteresis, which a
    /// fixed knob never crosses).
    #[must_use]
    pub fn chord(&self) -> i32 {
        // Truncated to int32, as the firmware's cast.
        let chord = (self.patch.structure * (crate::resonator::CHORD_COUNT - 1) as f32 + 0.5) as i32;
        chord.clamp(0, crate::resonator::CHORD_COUNT - 1)
    }

    /// The performance state and the patch for block `block` of a point `blocks` long.
    #[must_use]
    pub fn inputs(&self, block: usize, blocks: usize) -> (PerformanceState, ResonatorPatch) {
        let strum = Self::fires(block, blocks) && self.exciter != Exciter::External;
        let internal = self.exciter == Exciter::Internal;
        let performance = PerformanceState {
            strum,
            internal_exciter: internal,
            internal_strum: internal,
            internal_note: false,
            tonic: TONIC,
            note: self.note - TONIC,
            fm: self.fm,
            chord: self.chord(),
        };
        (performance, self.patch)
    }

    /// The audio input of the whole point: silence for [`Exciter::Internal`], otherwise an impulse
    /// of 1.0 at the first block and halfway, each decaying by 0.99 per sample (in `f32`, as the
    /// firmware's desktop test builds it).
    #[must_use]
    pub fn input_signal(&self, frames: usize) -> Vec<f32> {
        let mut signal = vec![0.0_f32; frames];
        if self.exciter == Exciter::Internal {
            return signal;
        }
        let blocks = frames / RESONATOR_BLOCK_SIZE;
        let mut impulse = 0.0_f32;
        for (block, chunk) in signal.chunks_mut(RESONATOR_BLOCK_SIZE).enumerate() {
            if Self::fires(block, blocks) {
                impulse = 1.0;
            }
            for s in chunk {
                *s = impulse;
                impulse *= 0.99;
            }
        }
        signal
    }

    /// A stable label for reports, such as `n60 s0.25 b0.50 d0.50 p0.50 int fm0`.
    #[must_use]
    pub fn label(&self) -> String {
        let p = &self.patch;
        format!("n{} s{:.2} b{:.2} d{:.2} p{:.2} {} fm{}", self.note, p.structure, p.brightness, p.damping, p.position, self.exciter.label(), self.fm)
    }
}

/// The 159 points, in a fixed order.
#[must_use]
pub fn points() -> Vec<Point> {
    let mut points = Vec::new();
    let center = ResonatorPatch { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 };
    for note in NOTES {
        for exciter in Exciter::ALL {
            for axis in 0..4 {
                for level in LEVELS {
                    #[allow(clippy::float_cmp)] // Exact constants: the center is rendered on the first axis only.
                    if axis > 0 && level == 0.5 {
                        continue;
                    }
                    let mut macros = [0.5_f32; 4];
                    macros[axis] = level;
                    let patch = ResonatorPatch { structure: macros[0], brightness: macros[1], damping: macros[2], position: macros[3] };
                    points.push(Point { note, patch, exciter, fm: 0.0 });
                }
            }
        }
        for fm in FMS {
            points.push(Point { note, patch: center, exciter: Exciter::Internal, fm });
        }
    }
    points
}

/// A render: float out and aux.
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    /// Main output (odd voices' harmonics, or the odd voices at polyphony above 1).
    pub out: Vec<f32>,
    /// Auxiliary output.
    pub aux: Vec<f32>,
}

/// Renders one point from a fresh part.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_point(session: &Session, model: ResonatorModel, polyphony: usize, point: &Point) -> Result<Render, Error> {
    session.seed(SEED);
    let mut part = session.resonator()?;
    part.set_polyphony(polyphony)?;
    part.set_model(model);
    let blocks = POINT_FRAMES / RESONATOR_BLOCK_SIZE;
    let input = point.input_signal(POINT_FRAMES);
    let mut render = Render { out: vec![0.0; POINT_FRAMES], aux: vec![0.0; POINT_FRAMES] };
    let ins = input.as_chunks::<RESONATOR_BLOCK_SIZE>().0;
    let outs = render.out.as_chunks_mut::<RESONATOR_BLOCK_SIZE>().0;
    let auxes = render.aux.as_chunks_mut::<RESONATOR_BLOCK_SIZE>().0;
    for (block, ((i, o), a)) in ins.iter().zip(outs.iter_mut()).zip(auxes.iter_mut()).enumerate() {
        let (performance, patch) = point.inputs(block, blocks);
        part.process(&performance, &patch, i, o, a)?;
    }
    Ok(render)
}

/// One line per model, polyphony and point, `<model> p<polyphony> <point> <digest>`, for the whole
/// sweep. Two runs with equal lines rendered bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    for model in ResonatorModel::ALL {
        for polyphony in POLYPHONIES {
            for point in points() {
                let r = render_point(session, model, polyphony, &point)?;
                let (out, aux) = (crate::sweep::digest_f32(&r.out), crate::sweep::digest_f32(&r.aux));
                lines.push(format!("{} p{polyphony} {} {out:016x}{aux:016x}", model.label(), point.label()));
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(points().len(), 159);
        assert_eq!(POINT_FRAMES % RESONATOR_BLOCK_SIZE, 0);
        let labels: std::collections::HashSet<_> = points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 159, "every point is rendered once");
    }

    #[test]
    fn strums_and_impulses_fire_on_the_first_block_and_halfway() {
        let p = Point {
            note: 60.0,
            patch: ResonatorPatch { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 },
            exciter: Exciter::ExternalStrummed,
            fm: 0.0,
        };
        assert!(p.inputs(0, 200).0.strum);
        assert!(!p.inputs(1, 200).0.strum);
        assert!(p.inputs(100, 200).0.strum);
        let ext = Point { exciter: Exciter::External, ..p };
        assert!(!ext.inputs(0, 200).0.strum);
        let signal = ext.input_signal(POINT_FRAMES);
        assert_eq!(signal[0].to_bits(), 1.0_f32.to_bits());
        assert_eq!(signal[2400].to_bits(), 1.0_f32.to_bits());
        assert!(signal[2399] < 1e-9);
        assert!(Point { exciter: Exciter::Internal, ..p }.input_signal(48).iter().all(|s| s.to_bits() == 0));
    }

    #[test]
    fn the_chord_follows_structure_over_the_whole_table() {
        let at = |structure| {
            Point {
                note: 60.0,
                patch: ResonatorPatch { structure, brightness: 0.5, damping: 0.5, position: 0.5 },
                exciter: Exciter::Internal,
                fm: 0.0,
            }
            .chord()
        };
        assert_eq!([at(0.0), at(0.25), at(0.5), at(0.75), at(0.9995)], [0, 3, 5, 8, 10]);
    }
}

//! The fixed sweep the meta-modulator is rendered with (ADR 0033 gate, #200).
//!
//! A point is rendered from a fresh modulator, after seeding the random state with [`SEED`], at
//! 96 kHz in the module's 60-frame blocks, with the same two deterministic inputs
//! ([`input_signal`]): a static point for [`POINT_FRAMES`] frames (0.25 s), a moving one for
//! [`MOTION_FRAMES`] (1 s). Fresh state per point keeps a residual local.
//!
//! Points, per [`CarrierShape`] (the external carrier and the internal oscillator's three shapes):
//!
//! - the grid: every [`ALGORITHM_LEVELS`] value (the six algorithms, the morphs halfway between
//!   them, the edges of the crossfade into the vocoder, the vocoder's release up to its freeze)
//!   at timbre 0, 0.5 and 1, the drives at 0.5 and the note at 60: 23 x 3 = 69 points;
//! - the axes: at three algorithms (between the two ring modulators, between XOR and the
//!   comparator, and the vocoder), each drive and the note over its other four levels with the
//!   rest at the grid's center (the center itself is a grid point): 3 x 12 = 36 points;
//! - motion ([`Motion`]): the algorithm and timbre ramping across the whole range one way and the
//!   other with the drives moving, and a fixed pseudo-random sequence of parameter jumps every
//!   seven blocks: 3 points.
//!
//! 108 points per shape, 432 in all. The port renders the same points through
//! [`Point::parameters`] and [`input_signal`], so both sides see the same values block for block.

#![forbid(unsafe_code)]

use crate::modulator::{CarrierShape, MODULATOR_BLOCK_SIZE, Parameters};
use crate::{Error, Session};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0200_5EED;

/// Frames of a static point: 0.25 s at 96 kHz, 400 blocks.
pub const POINT_FRAMES: usize = 24_000;

/// Frames of a moving point: 1 s at 96 kHz, 1600 blocks.
pub const MOTION_FRAMES: usize = 96_000;

/// The algorithm values of the grid: the six algorithms (0, 1/8 ... 5/8), the morphs halfway
/// between neighbors, the comparator's morph toward the dry modulator (11/16), the crossfade into
/// the vocoder (0.68, 0.69, 0.7, 0.71, 0.72), and the vocoder with its release growing (0.75,
/// 0.8125, 0.875, 0.9375, 0.97) to the freeze at 1 (from about 0.982 the band envelopes hold; on a
/// fresh modulator they hold at zero, so a static point there is silent by design, and the
/// [`Motion::Up`] ramp is what freezes learned envelopes).
pub const ALGORITHM_LEVELS: [f32; 23] = [
    0.0, 0.0625, 0.125, 0.1875, 0.25, 0.3125, 0.375, 0.4375, 0.5, 0.5625, 0.625, 0.6875, 0.68, 0.69, 0.7, 0.71, 0.72, 0.75, 0.8125, 0.875, 0.9375,
    0.97, 1.0,
];

/// The timbre values of the grid.
pub const TIMBRE_LEVELS: [f32; 3] = [0.0, 0.5, 1.0];

/// The five levels of each drive axis (0 is the module's closed level knob).
pub const DRIVE_LEVELS: [f32; 5] = [0.0, 0.0625, 0.25, 0.5625, 1.0];

/// The five levels of the note axis, MIDI notes (the module's knob covers 36 to 96).
pub const NOTE_LEVELS: [f32; 5] = [12.0, 36.0, 60.37, 84.0, 108.0];

/// The algorithms the axes are rendered at: between the two ring modulators, between XOR and the
/// comparator, and the vocoder.
pub const AXIS_ALGORITHMS: [f32; 3] = [0.3125, 0.5625, 0.875];

/// How a point's parameters move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    /// The parameters hold.
    Static,
    /// The algorithm and timbre ramp from 0 to 1 across the point, drive 1 from 0.1 to 1 and
    /// drive 2 from 1 to 0.1.
    Up,
    /// The algorithm and timbre ramp from 1 to 0, the drives the other way round from [`Motion::Up`].
    Down,
    /// Every seven blocks every parameter jumps to a value from a fixed linear congruential
    /// sequence (the note within 36 to 96).
    Jumps,
}

impl Motion {
    fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Up => "up",
            Self::Down => "down",
            Self::Jumps => "jumps",
        }
    }
}

/// One point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The parameters (a static point's for every block; a moving point starts from its carrier).
    pub base: Parameters,
    /// How the parameters move.
    pub motion: Motion,
}

/// A value from a 0-to-1 ramp over `blocks` blocks: `block / (blocks - 1)` in single precision.
fn ramp(block: usize, blocks: usize) -> f32 {
    let last = blocks.saturating_sub(1).max(1);
    #[allow(clippy::cast_precision_loss)] // Both are below 2^24.
    let t = block.min(last) as f32 / last as f32;
    t
}

/// The `k`th value of the jumps' sequence, 0 to 1 (exact multiples of 1/65536).
fn jump_value(k: u32) -> f32 {
    let mut state = 0x0200_1A9E_u32 ^ k.wrapping_mul(0x9E37_79B9);
    for _ in 0..3 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    #[allow(clippy::cast_precision_loss)] // A 16-bit value.
    let v = (state >> 16) as f32 / 65_536.0;
    v
}

impl Point {
    /// Whether the point is silent by design: a static point in the vocoder's freeze, where the
    /// band envelopes of a fresh modulator hold at zero (the release `r = 4 (algorithm - 0.75)`
    /// freezes them once `r (2 - r)` exceeds 0.995).
    #[must_use]
    pub fn silent_by_design(&self) -> bool {
        let r = (4.0 * (self.base.algorithm - 0.75)).clamp(0.0, 1.0);
        self.motion == Motion::Static && r * (2.0 - r) > 0.995
    }

    /// The point's length, frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        if self.motion == Motion::Static { POINT_FRAMES } else { MOTION_FRAMES }
    }

    /// The parameters for block `block` of a point `blocks` long.
    #[must_use]
    pub fn parameters(&self, block: usize, blocks: usize) -> Parameters {
        let mut p = self.base;
        match self.motion {
            Motion::Static => {}
            Motion::Up | Motion::Down => {
                let t = ramp(block, blocks);
                let t = if self.motion == Motion::Up { t } else { 1.0 - t };
                p.algorithm = t;
                p.timbre = t;
                p.drive = [0.1 + 0.9 * t, 1.0 - 0.9 * t];
            }
            Motion::Jumps => {
                let k = u32::try_from(block / 7).unwrap_or(0).wrapping_mul(5);
                p.algorithm = jump_value(k);
                p.timbre = jump_value(k + 1);
                p.drive = [jump_value(k + 2), jump_value(k + 3)];
                p.note = 36.0 + 60.0 * jump_value(k + 4);
            }
        }
        p
    }

    /// A stable label for reports, such as `saw alg0.6875 tim0.50 d0.5000/0.5000 n60.37 static`.
    #[must_use]
    pub fn label(&self) -> String {
        let p = &self.base;
        format!(
            "{} alg{:.4} tim{:.2} d{:.4}/{:.4} n{} {}",
            p.carrier.label(),
            p.algorithm,
            p.timbre,
            p.drive[0],
            p.drive[1],
            p.note,
            self.motion.label()
        )
    }
}

/// The center the axes start from: drives 0.5625 (the level knobs at three quarters), timbre 0.5,
/// note 60.37 (a little above middle C, so the pitch table's fine step is used).
#[must_use]
pub fn center(carrier: CarrierShape, algorithm: f32) -> Parameters {
    Parameters { drive: [0.5625, 0.5625], algorithm, timbre: 0.5, note: 60.37, carrier }
}

/// The 108 points of one carrier shape, in a fixed order.
#[must_use]
pub fn points_for(carrier: CarrierShape) -> Vec<Point> {
    let mut points = Vec::new();
    for algorithm in ALGORITHM_LEVELS {
        for timbre in TIMBRE_LEVELS {
            points.push(Point { base: Parameters { timbre, ..center(carrier, algorithm) }, motion: Motion::Static });
        }
    }
    for algorithm in AXIS_ALGORITHMS {
        let c = center(carrier, algorithm);
        // Exact constants: the center's own level is the grid point already rendered.
        #[allow(clippy::float_cmp)]
        for d in DRIVE_LEVELS.into_iter().filter(|d| *d != c.drive[0]) {
            points.push(Point { base: Parameters { drive: [d, c.drive[1]], ..c }, motion: Motion::Static });
        }
        #[allow(clippy::float_cmp)]
        for d in DRIVE_LEVELS.into_iter().filter(|d| *d != c.drive[1]) {
            points.push(Point { base: Parameters { drive: [c.drive[0], d], ..c }, motion: Motion::Static });
        }
        #[allow(clippy::float_cmp)]
        for note in NOTE_LEVELS.into_iter().filter(|n| *n != c.note) {
            points.push(Point { base: Parameters { note, ..c }, motion: Motion::Static });
        }
    }
    for motion in [Motion::Up, Motion::Down, Motion::Jumps] {
        points.push(Point { base: center(carrier, 0.0), motion });
    }
    points
}

/// Every point, shape by shape (432).
#[must_use]
pub fn points() -> Vec<Point> {
    CarrierShape::ALL.into_iter().flat_map(points_for).collect()
}

/// The two inputs every point is rendered with, `frames` stereo int16 frames at 96 kHz, from
/// integer arithmetic only (so every runner builds the same samples).
///
/// - Input 1 (left, the carrier, or the internal oscillator's FM input): a sawtooth of period 437
///   samples (220 Hz) and a triangle of period 2903 (33 Hz), with a little noise.
/// - Input 2 (right, the modulator): a triangle of period 293 (328 Hz) with noise, under a
///   syllable-like envelope of period 7200 frames (13.3 Hz) for the vocoder's followers.
///
/// Over every 24000 frames: both full for 12000 frames, input 2 silent from 12000 to 14400 (the
/// amplifier's gate closes), input 1 silent from 14400 to 16800, both at a quarter from 16800 to
/// 21600, and both overdriven into the int16 limits from 21600 to 24000.
#[must_use]
pub fn input_signal(frames: usize) -> Vec<[i16; 2]> {
    let mut state = 0x0200_1A9E_u32;
    (0..frames)
        .map(|n| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = i32::from((state >> 16) as u16 as i16) >> 4;
            let n32 = u32::try_from(n).unwrap_or(0);
            let saw = i32::try_from((n32 % 437) * 65_536 / 437).unwrap_or(0) - 32_768;
            let tri = |period: u32| {
                let phase = i32::try_from((n32 % period) * 65_536 / period).unwrap_or(0);
                (phase - 32_768).abs() * 2 - 32_768
            };
            let env = {
                let phase = i32::try_from(n32 % 7200).unwrap_or(0);
                // 0 to 4096: a rise over 1200 frames, a fall over 4800, 1200 of silence.
                if phase < 1200 {
                    phase * 4096 / 1200
                } else if phase < 6000 {
                    (6000 - phase) * 4096 / 4800
                } else {
                    0
                }
            };
            let mut l = saw * 3 / 8 + tri(2903) / 5 + noise / 4;
            let mut r = (tri(293) / 2 + noise) * env / 4096;
            match n % 24_000 {
                12_000..14_400 => r = 0,
                14_400..16_800 => l = 0,
                16_800..21_600 => (l, r) = (l / 4, r / 4),
                21_600..24_000 => (l, r) = (l * 3, r * 4),
                _ => {}
            }
            let clip = |x: i32| i16::try_from(x.clamp(-32_768, 32_767)).unwrap_or(0);
            [clip(l), clip(r)]
        })
        .collect()
}

/// Renders one point from a fresh modulator over the first [`Point::frames`] frames of `input`
/// (take it from [`input_signal`] at [`MOTION_FRAMES`]).
///
/// # Errors
///
/// An [`Error`] from the oracle; [`Error::BadBlock`] when `input` is shorter than the point.
pub fn render_point(session: &Session, point: &Point, input: &[[i16; 2]]) -> Result<Vec<[i16; 2]>, Error> {
    session.seed(SEED);
    let mut modulator = session.modulator()?;
    let input = input.get(..point.frames()).ok_or(Error::BadBlock)?;
    let blocks = input.len() / MODULATOR_BLOCK_SIZE;
    let mut out = vec![[0_i16; 2]; input.len()];
    for (block, (i, o)) in input.chunks(MODULATOR_BLOCK_SIZE).zip(out.chunks_mut(MODULATOR_BLOCK_SIZE)).enumerate() {
        modulator.process(&point.parameters(block, blocks), i, o)?;
    }
    Ok(out)
}

/// One line per point, `<point> <digest>`, for the whole sweep. Two runs with equal lines rendered
/// bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let input = input_signal(MOTION_FRAMES);
    let mut lines = Vec::new();
    for point in points() {
        let out = render_point(session, &point, &input)?;
        lines.push(format!("{} {:016x}", point.label(), crate::granular_sweep::digest_frames(&out)));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(points_for(CarrierShape::Saw).len(), 108);
        assert_eq!(points().len(), 432);
        let labels: std::collections::HashSet<_> = points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 432, "every point is rendered once");
        for point in points() {
            let blocks = point.frames() / MODULATOR_BLOCK_SIZE;
            for block in [0, 1, blocks / 2, blocks - 1] {
                assert!(crate::modulator::parameters_in_range(&point.parameters(block, blocks)), "{}", point.label());
            }
        }
        assert_eq!(POINT_FRAMES % MODULATOR_BLOCK_SIZE, 0);
        assert_eq!(MOTION_FRAMES % MODULATOR_BLOCK_SIZE, 0);
    }

    #[test]
    fn ramps_reach_both_ends_and_jumps_hold_for_seven_blocks() {
        let up = Point { base: center(CarrierShape::External, 0.0), motion: Motion::Up };
        assert_eq!(up.parameters(0, 1600).algorithm, 0.0);
        assert_eq!(up.parameters(1599, 1600).algorithm, 1.0);
        let down = Point { motion: Motion::Down, ..up };
        assert_eq!(down.parameters(0, 1600).algorithm, 1.0);
        assert_eq!(down.parameters(1599, 1600).timbre, 0.0);
        let jumps = Point { motion: Motion::Jumps, ..up };
        assert_eq!(jumps.parameters(7, 1600), jumps.parameters(13, 1600));
        assert_ne!(jumps.parameters(13, 1600), jumps.parameters(14, 1600));
    }

    #[test]
    fn the_inputs_are_deterministic_and_cover_their_sections() {
        let a = input_signal(24_000);
        assert_eq!(a, input_signal(24_000));
        assert!(a[12_000..14_400].iter().all(|[_, r]| *r == 0));
        assert!(a[14_400..16_800].iter().all(|[l, _]| *l == 0));
        assert!(a[21_600..].iter().any(|[l, _]| *l == i16::MAX || *l == i16::MIN));
        assert!(a[21_600..].iter().any(|[_, r]| *r == i16::MAX || *r == i16::MIN));
        assert!(a[..12_000].iter().any(|[l, r]| l.unsigned_abs() > 10_000 && r.unsigned_abs() > 5_000));
    }
}

//! The fixed sweep the granular texture processor is rendered with at every quality (ADR 0033
//! gate, #199).
//!
//! A point is rendered from a fresh processor on the module's memory sizes, after seeding the
//! random state with [`SEED`], at 32 kHz in the firmware's 32-frame blocks, for
//! [`point_frames`] frames (the quality's whole recording buffer plus a quarter of a second, so
//! even the oldest position plays recorded audio before the point ends: 1.27 s in 16-bit stereo,
//! 2.11 s in mono, 4.34 s and 7.67 s at the low-fidelity qualities), with the same deterministic
//! input ([`input_signal`]). Fresh state per point keeps a residual local: a failing point names
//! its parameters, not the history before it.
//!
//! Points, per playback mode, quality and [`Gate`] pattern: each of the nine continuous parameters
//! is swept over its five [`levels`] with the other eight at [`center`]; the center itself is
//! rendered once: 1 + 9 × 4 = 37 points, so 111 per mode and quality with the three patterns, 888
//! in all over the granular and the stretch modes. The port (#199) renders the same points through
//! [`Point::parameters`] and [`input_signal`], so both sides see the same values block for block;
//! the stretch points are the reference for the pitch shifter's port (#199 slice 2).

#![forbid(unsafe_code)]

use crate::granular::{FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, GRANULAR_BLOCK_SIZE, MAX_DRY_WET, MAX_SIZE, Parameters, PlaybackMode, Quality};
use crate::{Error, Session};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0199_5EED;

/// Frames of a point at `quality`: the recording buffer's length on the module's memory sizes, in
/// frames at 32 kHz (a low-fidelity buffer sample spans two), plus 8000 frames (0.25 s), rounded up
/// to whole blocks. The buffers hold 32696 samples (16-bit stereo: 65408 bytes per channel less the
/// 8-sample interpolation tail), 59384 (16-bit mono, 118784 bytes), 65400 (8-bit stereo) and
/// 118776 (8-bit mono).
#[must_use]
pub fn point_frames(quality: Quality) -> usize {
    let samples = match quality {
        Quality::Stereo => FIRMWARE_SMALL_BUFFER / 2 - 8,
        Quality::Mono => FIRMWARE_LARGE_BUFFER / 2 - 8,
        Quality::StereoLowFidelity => FIRMWARE_SMALL_BUFFER - 8,
        Quality::MonoLowFidelity => FIRMWARE_LARGE_BUFFER - 8,
    };
    let frames = if quality.low_fidelity() { samples * 2 } else { samples } + 8000;
    frames.div_ceil(GRANULAR_BLOCK_SIZE) * GRANULAR_BLOCK_SIZE
}

/// The longest point, frames ([`point_frames`] at the 8-bit mono quality): the input every point
/// takes its first [`point_frames`] frames from.
#[must_use]
pub fn longest_point_frames() -> usize {
    Quality::ALL.iter().map(|q| point_frames(*q)).max().unwrap_or(0)
}

/// The number of continuous parameters ([`Axis`]).
pub const AXES: usize = 9;

/// One continuous parameter of the sweep, in [`Parameters`] order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// [`Parameters::position`].
    Position,
    /// [`Parameters::size`].
    Size,
    /// [`Parameters::pitch`].
    Pitch,
    /// [`Parameters::density`].
    Density,
    /// [`Parameters::texture`].
    Texture,
    /// [`Parameters::dry_wet`].
    DryWet,
    /// [`Parameters::stereo_spread`].
    Spread,
    /// [`Parameters::feedback`].
    Feedback,
    /// [`Parameters::reverb`].
    Reverb,
}

impl Axis {
    /// Every axis, in sweep order.
    pub const ALL: [Self; AXES] =
        [Self::Position, Self::Size, Self::Pitch, Self::Density, Self::Texture, Self::DryWet, Self::Spread, Self::Feedback, Self::Reverb];

    fn get(self, p: &Parameters) -> f32 {
        match self {
            Self::Position => p.position,
            Self::Size => p.size,
            Self::Pitch => p.pitch,
            Self::Density => p.density,
            Self::Texture => p.texture,
            Self::DryWet => p.dry_wet,
            Self::Spread => p.stereo_spread,
            Self::Feedback => p.feedback,
            Self::Reverb => p.reverb,
        }
    }

    fn set(self, p: &mut Parameters, value: f32) {
        match self {
            Self::Position => p.position = value,
            Self::Size => p.size = value,
            Self::Pitch => p.pitch = value,
            Self::Density => p.density = value,
            Self::Texture => p.texture = value,
            Self::DryWet => p.dry_wet = value,
            Self::Spread => p.stereo_spread = value,
            Self::Feedback => p.feedback = value,
            Self::Reverb => p.reverb = value,
        }
    }
}

/// The five values an axis is swept over: 0, 0.25, 0.5, 0.75 and the top of the range (1, or the
/// largest value below 1 for size and dry/wet, [`MAX_SIZE`] and [`MAX_DRY_WET`]); pitch over -48,
/// -12, 0, 12 and 48 semitones, the module's whole range.
#[must_use]
pub fn levels(axis: Axis) -> [f32; 5] {
    match axis {
        Axis::Pitch => [-48.0, -12.0, 0.0, 12.0, 48.0],
        Axis::Size => [0.0, 0.25, 0.5, 0.75, MAX_SIZE],
        Axis::DryWet => [0.0, 0.25, 0.5, 0.75, MAX_DRY_WET],
        _ => [0.0, 0.25, 0.5, 0.75, 1.0],
    }
}

/// The center every other axis sits at: position, size, texture and spread 0.5, pitch 0, density
/// 0.75 (randomly seeded grains with some overlap: at 0.5 no grain plays), fully wet, feedback and
/// reverb 0.25 (so both paths run at every point). Every center value is one of its axis's levels.
#[must_use]
pub fn center() -> Parameters {
    Parameters {
        position: 0.5,
        size: 0.5,
        pitch: 0.0,
        density: 0.75,
        texture: 0.5,
        dry_wet: MAX_DRY_WET,
        stereo_spread: 0.5,
        feedback: 0.25,
        reverb: 0.25,
        freeze: false,
        trigger: false,
        gate: false,
    }
}

/// How freeze and the trigger move during a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// Neither: the recording runs and grains are seeded by density alone.
    Free,
    /// Freeze from the middle of the point to three quarters, then recording again (the buffer's
    /// crossfade on resume).
    Freeze,
    /// A trigger every [`TRIGGER_EVERY`] blocks.
    Trigger,
}

/// The trigger period of [`Gate::Trigger`], blocks (25 ms).
pub const TRIGGER_EVERY: usize = 25;

impl Gate {
    /// Every pattern, in sweep order.
    pub const ALL: [Self; 3] = [Self::Free, Self::Freeze, Self::Trigger];

    fn label(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Freeze => "freeze",
            Self::Trigger => "trig",
        }
    }
}

/// One point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The continuous parameters (freeze and trigger are set per block from `gate`).
    pub base: Parameters,
    /// How freeze and the trigger move.
    pub gate: Gate,
    /// The playback mode.
    pub mode: PlaybackMode,
}

impl Point {
    /// The parameters for block `block` of a point `blocks` long.
    #[must_use]
    pub fn parameters(&self, block: usize, blocks: usize) -> Parameters {
        let mut p = self.base;
        p.freeze = self.gate == Gate::Freeze && block >= blocks / 2 && block < blocks * 3 / 4;
        p.trigger = self.gate == Gate::Trigger && block.is_multiple_of(TRIGGER_EVERY);
        p
    }

    /// A stable label for reports, such as `pos0.50 size0.50 pit0 den0.75 tex0.50 wet1.00 spr0.50
    /// fb0.25 rev0.25 free` (with ` stretch` after it in the stretch mode).
    #[must_use]
    pub fn label(&self) -> String {
        let p = &self.base;
        format!(
            "pos{:.2} size{:.2} pit{} den{:.2} tex{:.2} wet{:.2} spr{:.2} fb{:.2} rev{:.2} {}{}",
            p.position,
            p.size,
            p.pitch,
            p.density,
            p.texture,
            p.dry_wet,
            p.stereo_spread,
            p.feedback,
            p.reverb,
            self.gate.label(),
            if self.mode == PlaybackMode::Stretch { " stretch" } else { "" }
        )
    }
}

/// The 222 points of one quality, in a fixed order: the granular mode's 111, then the stretch
/// mode's.
#[must_use]
pub fn points() -> Vec<Point> {
    PlaybackMode::ALL.into_iter().flat_map(points_in_mode).collect()
}

/// The 111 points of one quality in `mode`, in a fixed order.
#[must_use]
pub fn points_in_mode(mode: PlaybackMode) -> Vec<Point> {
    let mut points = Vec::new();
    let center = center();
    for gate in Gate::ALL {
        points.push(Point { base: center, gate, mode });
        for axis in Axis::ALL {
            for level in levels(axis) {
                #[allow(clippy::float_cmp)] // Exact constants: the center is rendered once.
                if level == axis.get(&center) {
                    continue;
                }
                let mut base = center;
                axis.set(&mut base, level);
                points.push(Point { base, gate, mode });
            }
        }
    }
    points
}

/// The input every point is rendered with, `frames` stereo int16 frames, from integer arithmetic
/// only (so every runner builds the same samples): on the left a triangle of period 145 samples
/// (221 Hz), on the right a sawtooth of period 97 (330 Hz), both with noise from a linear
/// congruential generator, at full level for 4000 frames and a quarter for the next 4000, and
/// overdriven into the int16 limits for 2000 frames out of every 40000 (from frame 24000).
#[must_use]
pub fn input_signal(frames: usize) -> Vec<[i16; 2]> {
    let mut state = 0x0199_1A9E_u32;
    (0..frames)
        .map(|n| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = i32::from((state >> 16) as u16 as i16) >> 3;
            let n32 = u32::try_from(n).unwrap_or(0);
            let tri_phase = i32::try_from((n32 % 145) * 65_536 / 145).unwrap_or(0);
            let triangle = (tri_phase - 32_768).abs() * 2 - 32_768;
            let saw = i32::try_from((n32 % 97) * 65_536 / 97).unwrap_or(0) - 32_768;
            let quiet = (n / 4000) % 2 == 1;
            let loud = (24_000..26_000).contains(&(n % 40_000));
            let (mut l, mut r) = (triangle * 3 / 8 + noise, saw / 4 - noise / 2);
            if quiet {
                (l, r) = (l / 4, r / 4);
            }
            if loud {
                (l, r) = (l * 5 / 2, r * 4);
            }
            let clip = |x: i32| i16::try_from(x.clamp(-32_768, 32_767)).unwrap_or(0);
            [clip(l), clip(r)]
        })
        .collect()
}

/// Renders one point from a fresh processor on the module's memory sizes, over the first
/// [`point_frames`] frames of `input` (take it from [`input_signal`] at [`longest_point_frames`]).
///
/// # Errors
///
/// An [`Error`] from the oracle; [`Error::BadBlock`] when `input` is shorter than the point.
pub fn render_point(session: &Session, quality: Quality, point: &Point, input: &[[i16; 2]]) -> Result<Vec<[i16; 2]>, Error> {
    session.seed(SEED);
    let mut processor = session.granular_in_mode(FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, point.mode)?;
    let input = input.get(..point_frames(quality)).ok_or(Error::BadBlock)?;
    let blocks = input.len() / GRANULAR_BLOCK_SIZE;
    let mut out = vec![[0_i16; 2]; input.len()];
    for (block, (i, o)) in input.chunks(GRANULAR_BLOCK_SIZE).zip(out.chunks_mut(GRANULAR_BLOCK_SIZE)).enumerate() {
        processor.process(&point.parameters(block, blocks), i, o)?;
    }
    Ok(out)
}

/// FNV-1a 64 over the frames (left then right, little-endian).
#[must_use]
pub fn digest_frames(frames: &[[i16; 2]]) -> u64 {
    crate::sweep::digest(frames.iter().flat_map(|[l, r]| {
        let [a, b] = l.to_le_bytes();
        let [c, d] = r.to_le_bytes();
        [a, b, c, d]
    }))
}

/// One line per quality and point, `<quality> <point> <digest>`, for the whole sweep. Two runs with
/// equal lines rendered bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let input = input_signal(longest_point_frames());
    let mut lines = Vec::new();
    for quality in Quality::ALL {
        for point in points() {
            let out = render_point(session, quality, &point, &input)?;
            lines.push(format!("{} {} {:016x}", quality.label(), point.label(), digest_frames(&out)));
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(points().len(), 222);
        assert_eq!(points_in_mode(PlaybackMode::Granular).len(), 111);
        let frames: Vec<usize> = Quality::ALL.iter().map(|q| point_frames(*q)).collect();
        assert_eq!(frames, [40_704, 67_392, 138_816, 245_568]);
        let labels: std::collections::HashSet<_> = points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 222, "every point is rendered once");
        assert!(points().iter().all(|p| crate::granular::parameters_in_range(&p.base)));
    }

    #[test]
    fn freeze_and_trigger_follow_their_patterns() {
        let p = Point { base: center(), gate: Gate::Freeze, mode: PlaybackMode::Granular };
        assert!(!p.parameters(499, 1000).freeze);
        assert!(p.parameters(500, 1000).freeze);
        assert!(p.parameters(749, 1000).freeze);
        assert!(!p.parameters(750, 1000).freeze);
        assert!(!p.parameters(0, 1000).trigger);
        let t = Point { base: center(), gate: Gate::Trigger, mode: PlaybackMode::Granular };
        assert!(t.parameters(0, 1000).trigger && t.parameters(25, 1000).trigger && !t.parameters(26, 1000).trigger);
        assert!(!t.parameters(600, 1000).freeze);
    }

    #[test]
    fn the_input_is_deterministic_and_reaches_the_limits() {
        let a = input_signal(40_000);
        assert_eq!(a, input_signal(40_000));
        assert!(a.iter().any(|[l, _]| *l == i16::MAX) && a.iter().any(|[_, r]| *r == i16::MIN));
        assert!(a.iter().take(4000).any(|[l, r]| l.unsigned_abs() > 10_000 && r.unsigned_abs() > 5_000));
    }
}

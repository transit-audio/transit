//! The fixed sweep the random sequencer is rendered with (ADR 0033 gate, #202).
//!
//! A point is rendered from fresh generators seeded with [`SEED`], at 32 kHz in the module's
//! 5-frame blocks, for [`Point::frames`] frames (3 s, or 6 s for the deja vu and motion points).
//! Fresh state per point keeps a difference local. Each frame's three gates and four voltages are
//! the output; the port must give the same gates and the same voltage bits.
//!
//! The center: the complementary Bernoulli model at 16 Hz (`t_rate` 36 semitones above 2 Hz, 2000
//! frames per tick), the knobs at 12 o'clock, the gate length at half, the X section quantized to
//! the major scale (steps 0.75) over the full range. Points, in order:
//!
//! - **models**: every t model (the module's six and the Markov model it hides) at five t biases
//!   (35);
//! - **jitter**: four amounts at three models (12);
//! - **gate length**: four means by three randomness amounts (12);
//! - **rate**: three ranges by four rates (12);
//! - **deja vu**: five knob positions (across the dead band) by four loop lengths, on both sections
//!   (20), and the locked state at two lengths (2);
//! - **X distribution**: five spreads by five biases (25), seven steps (7), every scale at two steps
//!   (12), every control mode by every range (9), the input-sampling mode at three inputs (3);
//! - **Y**: six settings of its spread, bias, steps, division and range (6);
//! - **clocks**: the X section on each internal clock (4), on its clock input (2), the t section on
//!   its clock input at four divisions and clock shapes (steady, swung, paused and restarted, at
//!   audio rate) (6), explicit resets every 37 blocks (2);
//! - **scale**: a twelve-degree chromatic scale loaded into slot 5 (1);
//! - **motion** ([`Motion`]): the knobs ramping one way and the other, and a fixed sequence of jumps
//!   every 37 blocks (3).
//!
//! 173 points. The port renders the same points through [`Point::controls`], [`Point::clocks`] and
//! [`Point::scale`], so both sides see the same values block for block.

#![forbid(unsafe_code)]

use crate::sequencer::{Controls, SEQUENCER_BLOCK_SIZE, Scale, gate};
use crate::{Error, Session};

/// The seed of the generators' random source at every point.
pub const SEED: u32 = 0x0202_5EED;

/// Frames of a point: 3 s at 32 kHz (600 blocks).
pub const POINT_FRAMES: usize = 96_000;

/// Frames of a deja vu or moving point: 6 s.
pub const LONG_FRAMES: usize = 192_000;

/// How a point's knobs move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    /// The controls hold.
    Static,
    /// The t bias, jitter, spread, X bias, steps and deja vu ramp from 0 to 1 across the point.
    Up,
    /// The same knobs ramp from 1 to 0.
    Down,
    /// Every 37 blocks every knob, the model and the scale jump to values from a fixed sequence.
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

/// A clock input's signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clock {
    /// A pulse every `period` frames, high for the first `high`.
    #[allow(missing_docs)]
    Steady { period: u32, high: u32 },
    /// Pulses alternating `a` and `b` frames apart, high for `high`.
    #[allow(missing_docs)]
    Swing { a: u32, b: u32, high: u32 },
    /// A steady clock that stops for `pause` frames after `at` frames, then restarts (the clock
    /// recovery takes a long pause for a restart and resets).
    #[allow(missing_docs)]
    Paused { period: u32, high: u32, at: u32, pause: u32 },
}

impl Clock {
    fn high(self, n: u32) -> bool {
        match self {
            Self::Steady { period, high } => n % period < high,
            Self::Swing { a, b, high } => {
                let m = n % (a + b);
                if m < a { m < high } else { m - a < high }
            }
            Self::Paused { period, high, at, pause } => {
                if n < at {
                    n % period < high
                } else if n < at + pause {
                    false
                } else {
                    (n - at - pause) % period < high
                }
            }
        }
    }

    /// The gate flags of the first `frames` frames.
    #[must_use]
    pub fn flags(self, frames: usize) -> Vec<u8> {
        let mut previous = 0_u8;
        (0..frames)
            .map(|n| {
                let f = gate::extract(previous, self.high(u32::try_from(n).unwrap_or(u32::MAX)));
                previous = f;
                f
            })
            .collect()
    }

    fn label(self) -> String {
        match self {
            Self::Steady { period, high } => format!("steady{period}/{high}"),
            Self::Swing { a, b, high } => format!("swing{a}/{b}/{high}"),
            Self::Paused { period, high, at, pause } => format!("paused{period}/{high}@{at}+{pause}"),
        }
    }
}

/// One point.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// What the point varies, for its label.
    pub name: String,
    /// The controls (a static point's for every block; a moving point starts from them).
    pub base: Controls,
    /// How the knobs move.
    pub motion: Motion,
    /// The t clock input, when patched (`base.t_external` is then 1).
    pub t_clock: Option<Clock>,
    /// The X clock input, when used (`base.xy_clock_source` is then 4).
    pub xy_clock: Option<Clock>,
    /// An explicit reset of both sections every this many blocks (the first at block 0 excluded).
    pub reset_every: Option<usize>,
    /// A scale loaded into a slot after the generators are made.
    pub scale: Option<(usize, Scale)>,
    /// Length, frames.
    pub frames: usize,
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
    let mut state = 0x0202_1A9E_u32 ^ k.wrapping_mul(0x9E37_79B9);
    for _ in 0..3 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    #[allow(clippy::cast_precision_loss)] // A 16-bit value.
    let v = (state >> 16) as f32 / 65_536.0;
    v
}

/// The center every point starts from (see the module documentation).
#[must_use]
pub fn center() -> Controls {
    Controls { t_rate: 36.0, x_steps: 0.75, ..Controls::default() }
}

/// A twelve-tone scale of equal weights (all 255): every semitone at every steps level.
#[must_use]
pub fn chromatic() -> Scale {
    #[allow(clippy::cast_precision_loss)] // Small integers.
    let degrees = (0..12).map(|i| (i as f32 / 12.0, 255_u8)).collect();
    Scale { base_interval: 1.0, degrees }
}

impl Point {
    fn new(name: impl Into<String>, base: Controls) -> Self {
        Self { name: name.into(), base, motion: Motion::Static, t_clock: None, xy_clock: None, reset_every: None, scale: None, frames: POINT_FRAMES }
    }

    fn long(mut self) -> Self {
        self.frames = LONG_FRAMES;
        self
    }

    /// The point's length, frames.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The controls for block `block` of a point `blocks` long.
    #[must_use]
    pub fn controls(&self, block: usize, blocks: usize) -> Controls {
        let mut c = self.base;
        match self.motion {
            Motion::Static => {}
            Motion::Up | Motion::Down => {
                let t = ramp(block, blocks);
                let t = if self.motion == Motion::Up { t } else { 1.0 - t };
                c.t_bias = t;
                c.t_jitter = t;
                c.x_spread = t;
                c.x_bias = 1.0 - t;
                c.x_steps = t;
                c.deja_vu = t;
            }
            Motion::Jumps => {
                let k = u32::try_from(block / 37).unwrap_or(0).wrapping_mul(9);
                c.t_bias = jump_value(k);
                c.t_jitter = jump_value(k + 1) * 0.5;
                c.x_spread = jump_value(k + 2);
                c.x_bias = jump_value(k + 3);
                c.x_steps = jump_value(k + 4);
                c.deja_vu = jump_value(k + 5);
                c.deja_vu_length = jump_value(k + 6);
                #[allow(clippy::cast_possible_truncation)] // Below 6.
                let model = (jump_value(k + 7) * 6.0) as i32;
                c.t_model = model;
                #[allow(clippy::cast_possible_truncation)] // Below 6.
                let scale = (jump_value(k + 8) * 6.0) as i32;
                c.x_scale = scale;
            }
        }
        if let Some(every) = self.reset_every
            && block > 0
            && block.is_multiple_of(every)
        {
            c.t_reset = 1;
            c.x_reset = 1;
        }
        c
    }

    /// The clock inputs' gate flags over the point, (t, X).
    #[must_use]
    pub fn clocks(&self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        (self.t_clock.map(|c| c.flags(self.frames)), self.xy_clock.map(|c| c.flags(self.frames)))
    }

    /// A stable label for reports, such as `models m3 b0.25 static`.
    #[must_use]
    pub fn label(&self) -> String {
        let mut s = format!("{} {}", self.name, self.motion.label());
        if let Some(c) = self.t_clock {
            s.push_str(&format!(" t:{}", c.label()));
        }
        if let Some(c) = self.xy_clock {
            s.push_str(&format!(" x:{}", c.label()));
        }
        s
    }
}

/// Every point, in a fixed order (173).
#[must_use]
#[allow(clippy::too_many_lines)] // One list.
pub fn points() -> Vec<Point> {
    let c = center();
    let mut p = Vec::new();
    for model in 0..7 {
        for bias in [0.0, 0.25, 0.5, 0.75, 1.0] {
            p.push(Point::new(format!("models m{model} b{bias:.2}"), Controls { t_model: model, t_bias: bias, ..c }));
        }
    }
    for model in [0, 1, 4] {
        for jitter in [0.25, 0.5, 0.75, 1.0] {
            p.push(Point::new(format!("jitter m{model} j{jitter:.2}"), Controls { t_model: model, t_jitter: jitter, ..c }));
        }
    }
    for mean in [0, 64, 192, 255] {
        for std in [0, 128, 255] {
            p.push(Point::new(format!("length mean{mean} std{std}"), Controls { t_pulse_width_mean: mean, t_pulse_width_std: std, ..c }));
        }
    }
    for range in 0..3 {
        for rate in [-24.0, 0.0, 24.0, 48.0] {
            p.push(Point::new(format!("rate r{range} {rate}"), Controls { t_range: range, t_rate: rate, ..c }));
        }
    }
    for deja_vu in [0.2, 0.49, 0.5, 0.75, 1.0] {
        for length in [0.0, 0.3, 0.6, 1.0] {
            p.push(
                Point::new(
                    format!("deja vu {deja_vu:.2} len{length:.1}"),
                    Controls { deja_vu, deja_vu_length: length, t_deja_vu: 1, x_deja_vu: 1, ..c },
                )
                .long(),
            );
        }
    }
    for length in [0.4, 1.0] {
        p.push(Point::new(format!("deja vu locked len{length:.1}"), Controls { deja_vu_length: length, t_deja_vu: 2, x_deja_vu: 2, ..c }).long());
    }
    for spread in [0.0, 0.25, 0.5, 0.75, 1.0] {
        for bias in [0.0, 0.25, 0.5, 0.75, 1.0] {
            p.push(Point::new(format!("x s{spread:.2} b{bias:.2}"), Controls { x_spread: spread, x_bias: bias, ..c }));
        }
    }
    for steps in [0.0, 0.25, 0.49, 0.5, 0.6, 0.9, 1.0] {
        p.push(Point::new(format!("steps {steps:.2}"), Controls { x_steps: steps, ..c }));
    }
    for scale in 0..6 {
        for steps in [0.7, 1.0] {
            p.push(Point::new(format!("scale {scale} steps{steps:.1}"), Controls { x_scale: scale, x_steps: steps, ..c }));
        }
    }
    for mode in 0..3 {
        for range in 0..3 {
            p.push(Point::new(
                format!("x mode{mode} range{range}"),
                Controls { x_control_mode: mode, x_range: range, x_spread: 0.8, x_bias: 0.3, ..c },
            ));
        }
    }
    for value in [0.1, 0.5, 0.93] {
        p.push(Point::new(format!("register {value:.2}"), Controls { x_register_mode: 1, x_register_value: value, ..c }));
    }
    for (spread, bias, steps, divider, range) in
        [(0, 0, 0, 255, 0), (255, 255, 255, 255, 1), (64, 200, 128, 0, 2), (200, 64, 200, 100, 2), (128, 128, 0, 170, 1), (32, 32, 255, 220, 0)]
    {
        p.push(Point::new(
            format!("y s{spread} b{bias} st{steps} d{divider} r{range}"),
            Controls { y_spread: spread, y_bias: bias, y_steps: steps, y_divider: divider, y_range: range, ..c },
        ));
    }
    for source in 0..4 {
        p.push(Point::new(format!("x clock {source}"), Controls { xy_clock_source: source, t_model: 1, ..c }));
    }
    for clock in [Clock::Steady { period: 1500, high: 300 }, Clock::Swing { a: 2600, b: 1400, high: 700 }] {
        let mut point = Point::new("x clock input", Controls { xy_clock_source: 4, ..c });
        point.xy_clock = Some(clock);
        p.push(point);
    }
    for (rate, range, clock) in [
        (0.0, 1, Clock::Steady { period: 2000, high: 1000 }),
        (30.0, 1, Clock::Swing { a: 2400, b: 1600, high: 400 }),
        (-30.0, 2, Clock::Steady { period: 3000, high: 60 }),
        (12.0, 0, Clock::Paused { period: 1800, high: 900, at: 40_000, pause: 120_000 }),
        (0.0, 1, Clock::Steady { period: 200, high: 100 }),
        (45.0, 1, Clock::Steady { period: 230, high: 23 }),
    ] {
        let mut point = Point::new(format!("t clock input {rate} r{range}"), Controls { t_external: 1, t_rate: rate, t_range: range, ..c });
        point.t_clock = Some(clock);
        if matches!(clock, Clock::Paused { .. }) {
            point = point.long();
        }
        p.push(point);
    }
    for model in [1, 2] {
        let mut point = Point::new(format!("resets m{model}"), Controls { t_model: model, deja_vu: 0.5, t_deja_vu: 1, x_deja_vu: 1, ..c });
        point.reset_every = Some(37);
        p.push(point);
    }
    let mut point = Point::new("scale chromatic", Controls { x_scale: 5, x_steps: 0.6, x_range: 1, ..c });
    point.scale = Some((5, chromatic()));
    p.push(point);
    for motion in [Motion::Up, Motion::Down, Motion::Jumps] {
        let mut point = Point::new("motion", Controls { t_deja_vu: 1, x_deja_vu: 1, ..c }).long();
        point.motion = motion;
        p.push(point);
    }
    p
}

/// One point's output: per frame the three gates (t1, t2, t3) and four voltages (X1, X2, X3, Y).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Render {
    /// t1, t2, t3 per frame (0 or 1).
    pub gates: Vec<[u8; 3]>,
    /// X1, X2, X3, Y per frame, volts.
    pub voltages: Vec<[f32; 4]>,
}

impl Render {
    /// A 64-bit FNV-1a digest of the gates and the voltages' bits.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        let mut eat = |b: u8| h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        for g in &self.gates {
            g.iter().for_each(|b| eat(*b));
        }
        for v in &self.voltages {
            v.iter().flat_map(|x| x.to_bits().to_le_bytes()).for_each(&mut eat);
        }
        h
    }

    /// The first frame where `other` differs, with a description, or `None` when equal bit for bit.
    #[must_use]
    pub fn first_difference(&self, other: &Self) -> Option<String> {
        if self.gates.len() != other.gates.len() {
            return Some(format!("lengths {} and {}", self.gates.len(), other.gates.len()));
        }
        for (n, ((ga, gb), (va, vb))) in self.gates.iter().zip(&other.gates).zip(self.voltages.iter().zip(&other.voltages)).enumerate() {
            if ga != gb || va.iter().zip(vb).any(|(a, b)| a.to_bits() != b.to_bits()) {
                return Some(format!("frame {n}: gates {ga:?} vs {gb:?}, voltages {va:?} vs {vb:?}"));
            }
        }
        None
    }
}

/// Renders one point from fresh generators.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_point(session: &Session, point: &Point) -> Result<Render, Error> {
    render_point_with(session, point, true)
}

/// [`render_point`] with the flush-to-zero scope on or off (the evidence switch).
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_point_with(session: &Session, point: &Point, flush: bool) -> Result<Render, Error> {
    let mut s = session.sequencer(SEED)?;
    s.set_flush(flush);
    if let Some((slot, scale)) = &point.scale {
        s.load_scale(*slot, scale)?;
    }
    let (t_clock, xy_clock) = point.clocks();
    let frames = point.frames();
    let blocks = frames / SEQUENCER_BLOCK_SIZE;
    let mut out = Render { gates: vec![[0; 3]; frames], voltages: vec![[0.0; 4]; frames] };
    for (block, (g, v)) in out.gates.chunks_mut(SEQUENCER_BLOCK_SIZE).zip(out.voltages.chunks_mut(SEQUENCER_BLOCK_SIZE)).enumerate() {
        let at = block * SEQUENCER_BLOCK_SIZE;
        let range = at..at + g.len();
        let t = t_clock.as_ref().and_then(|c| c.get(range.clone()));
        let x = xy_clock.as_ref().and_then(|c| c.get(range));
        s.process(&point.controls(block, blocks), t, x, g, v)?;
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
    points().iter().map(|point| render_point(session, point).map(|r| format!("{} {:016x}", point.label(), r.digest()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::controls_in_range;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        let points = points();
        assert_eq!(points.len(), 173);
        let labels: std::collections::HashSet<_> = points.iter().map(Point::label).collect();
        assert_eq!(labels.len(), points.len(), "every point is rendered once");
        for point in &points {
            assert_eq!(point.frames() % SEQUENCER_BLOCK_SIZE, 0);
            let blocks = point.frames() / SEQUENCER_BLOCK_SIZE;
            for block in [0, 1, 37, blocks / 2, blocks - 1] {
                assert!(controls_in_range(&point.controls(block, blocks)), "{}", point.label());
            }
            assert_eq!(point.base.t_external == 1, point.t_clock.is_some(), "{}", point.label());
            assert_eq!(point.base.xy_clock_source == 4, point.xy_clock.is_some(), "{}", point.label());
        }
    }

    #[test]
    fn ramps_reach_both_ends_and_jumps_hold_for_37_blocks() {
        let mut up = Point::new("m", center());
        up.motion = Motion::Up;
        assert_eq!(up.controls(0, 1200).x_spread, 0.0);
        assert_eq!(up.controls(1199, 1200).x_spread, 1.0);
        let mut jumps = up.clone();
        jumps.motion = Motion::Jumps;
        assert_eq!(jumps.controls(37, 1200), jumps.controls(73, 1200));
        assert_ne!(jumps.controls(73, 1200), jumps.controls(74, 1200));
    }

    #[test]
    fn clock_flags_mark_edges() {
        let f = Clock::Steady { period: 4, high: 2 }.flags(9);
        assert_eq!(f, vec![3, 1, 4, 0, 3, 1, 4, 0, 3]);
        let p = Clock::Paused { period: 4, high: 2, at: 8, pause: 6 }.flags(18);
        assert_eq!(&p[8..14], &[0; 6]);
        assert_eq!(p[14], 3);
    }
}

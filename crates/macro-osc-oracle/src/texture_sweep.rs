//! The fixed sweep the texture effect chain is rendered with at every quality (ADR 0033 gate,
//! #241).
//!
//! A point is rendered from a fresh chain on the module's memory, after seeding the random state
//! with [`SEED`], at the quality's codec rate in the module's blocks ([`Media::block_size`]: 12
//! frames at 48 kHz, 8 at 32 kHz, 6 at 24 kHz), with the same deterministic input
//! ([`input_signal`]). Fresh state per point keeps a residual local: a failing point names its
//! controls, not the history before it.
//!
//! Points, per quality ([`Quality`]: four media, stereo or mono) and [`Gate`] pattern: each of the
//! twelve continuous controls ([`Axis`]) is swept over its five [`levels`] with the others at the
//! granular [`center`] (TIME's center, 0.1, is not one of its levels, so TIME has five points;
//! SIZE's top level, 1, is the delay mode's center instead, so SIZE has three: 1 + 5 + 10 × 4 + 3 =
//! 49 points), and each but SIZE again around the delay-mode [`delay_center`] (1 + 5 + 10 × 4 =
//! 46): 95 points, 285 per quality with the three patterns, 2280 in all.
//!
//! A granular point lasts [`GRANULAR_POINT_SECONDS`], or longer when TIME reaches further back:
//! until 1.5 s after its grains start reading recorded audio (on a fresh buffer the grains read
//! silence until the recording reaches their position, up to 32 s back in the 8-bit mono buffer).
//! A delay point lasts [`DELAY_POINT_SECONDS`] (the firmware takes 1.5 s to cross into the delay
//! mode and fades for 0.75 s more), or longer when its taps reach further back ([`Point::frames`]). The port (#241) renders the same points through
//! [`Point::frames`], [`Point::controls`], [`Point::freeze`], [`Point::seed`] and [`input_signal`],
//! so both sides see the same values block for block.
//!
//! The synth is swept the same way around [`synth_center`] at each of its four resolutions
//! ([`synth_points`]: 147 points, 588 in all, 2 s each), and the input stage runs a fixed
//! 10-second schedule ([`input_step`]) at each codec rate and soft-clipping mode ([`INPUT_RUNS`]).

#![forbid(unsafe_code)]

use crate::texture::{Controls, Media, Parameter, SeedButton};
use crate::{Error, Session};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0241_5EED;

/// The length of a granular point, seconds.
pub const GRANULAR_POINT_SECONDS: f32 = 2.0;

/// The length of a delay-mode point, seconds.
pub const DELAY_POINT_SECONDS: f32 = 3.5;

/// A buffer quality and the mono setting (the module sets mono when only the left input is
/// patched; the buffer then holds twice as long).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Quality {
    /// The buffer quality.
    pub media: Media,
    /// Mono recording.
    pub mono: bool,
}

impl Quality {
    /// Every quality, digital stereo first.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all = Vec::new();
        for media in [Media::Digital, Media::VintageDigital, Media::CleanTape, Media::DirtyTape] {
            for mono in [false, true] {
                all.push(Self { media, mono });
            }
        }
        all
    }

    /// How far back the grains can reach at TIME 1, seconds at the codec rate: the quality's delay
    /// line on the module's memory (`kDelayBufferSizeBits` divided by the item size; mono holds
    /// twice as many items) at the rate the grains run.
    #[must_use]
    pub fn buffer_seconds(self) -> f32 {
        let (items, rate) = match self.media {
            Media::Digital => (192_000.0, 48_000.0),
            Media::VintageDigital => (256_000.0, 32_000.0),
            Media::CleanTape => (256_000.0, 24_000.0),
            Media::DirtyTape => (384_000.0, 24_000.0),
        };
        items * if self.mono { 2.0 } else { 1.0 } / rate
    }

    /// A stable label, such as `digital-stereo`.
    #[must_use]
    pub fn label(self) -> String {
        format!("{}-{}", self.media.label(), if self.mono { "mono" } else { "stereo" })
    }
}

/// The number of continuous controls ([`Axis`]).
pub const AXES: usize = 12;

/// One continuous control of the sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// TIME.
    Time,
    /// SIZE.
    Size,
    /// SHAPE.
    Shape,
    /// PITCH.
    Pitch,
    /// TIME's attenuverter (internal randomness).
    TimeAmount,
    /// SIZE's attenuverter (randomness; also the delay's bandwidth).
    SizeAmount,
    /// SHAPE's attenuverter.
    ShapeAmount,
    /// PITCH's attenuverter.
    PitchAmount,
    /// DENSITY.
    Density,
    /// FEEDBACK.
    Feedback,
    /// DRY/WET.
    DryWet,
    /// REVERB.
    Reverb,
}

impl Axis {
    /// Every axis, in sweep order.
    pub const ALL: [Self; AXES] = [
        Self::Time,
        Self::Size,
        Self::Shape,
        Self::Pitch,
        Self::TimeAmount,
        Self::SizeAmount,
        Self::ShapeAmount,
        Self::PitchAmount,
        Self::Density,
        Self::Feedback,
        Self::DryWet,
        Self::Reverb,
    ];

    fn slot(self, c: &mut Controls) -> &mut f32 {
        match self {
            Self::Time => &mut c.time.base,
            Self::Size => &mut c.size.base,
            Self::Shape => &mut c.shape.base,
            Self::Pitch => &mut c.pitch.base,
            Self::TimeAmount => &mut c.time.attenuverter,
            Self::SizeAmount => &mut c.size.attenuverter,
            Self::ShapeAmount => &mut c.shape.attenuverter,
            Self::PitchAmount => &mut c.pitch.attenuverter,
            Self::Density => &mut c.density,
            Self::Feedback => &mut c.feedback,
            Self::DryWet => &mut c.dry_wet,
            Self::Reverb => &mut c.reverb,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Size => "size",
            Self::Shape => "shape",
            Self::Pitch => "pitch",
            Self::TimeAmount => "time-amt",
            Self::SizeAmount => "size-amt",
            Self::ShapeAmount => "shape-amt",
            Self::PitchAmount => "pitch-amt",
            Self::Density => "density",
            Self::Feedback => "feedback",
            Self::DryWet => "dry-wet",
            Self::Reverb => "reverb",
        }
    }
}

/// The five values an axis is swept over: 0, 0.25, 0.5, 0.75 and 1 for the knobs; -24, -12, 0,
/// 12 and 24 semitones for PITCH (the knob's whole range); -1, -0.5, 0, 0.5 and 1 for the
/// attenuverters (negative: per-grain randomness, positive: smooth random modulation).
#[must_use]
pub fn levels(axis: Axis) -> [f32; 5] {
    match axis {
        Axis::Pitch => [-24.0, -12.0, 0.0, 12.0, 24.0],
        Axis::TimeAmount | Axis::SizeAmount | Axis::ShapeAmount | Axis::PitchAmount => [-1.0, -0.5, 0.0, 0.5, 1.0],
        _ => [0.0, 0.25, 0.5, 0.75, 1.0],
    }
}

/// The granular center: TIME at 0.1 (the grains start a tenth of the buffer back), SIZE and SHAPE
/// at noon, PITCH 0, no randomness, DENSITY 0.75
/// (randomly seeded grains: between 0.48 and 0.52 only taps seed them), FEEDBACK and REVERB
/// 0.25 (so both paths run at every point), fully wet, the SEED button latched (grains flow
/// without a SEED input; released, nothing seeds them). Every center value but TIME's is one of
/// its axis's levels.
#[must_use]
pub fn center() -> Controls {
    let knob = |base: f32| Parameter { base, attenuverter: 0.0, cv: 0.0, has_cv: false };
    Controls {
        time: knob(0.1),
        size: knob(0.5),
        shape: knob(0.5),
        pitch: knob(0.0),
        density: 0.75,
        density_cv: 0.0,
        feedback: 0.25,
        dry_wet: 1.0,
        reverb: 0.25,
        seed_button: SeedButton::Locked,
        monophonic: false,
        output_gate: false,
    }
}

/// The delay-mode center: the granular [`center`] with SIZE at 1 (above 0.99 the firmware crosses
/// into the delay mode within 1.5 s).
#[must_use]
pub fn delay_center() -> Controls {
    let mut c = center();
    c.size.base = 1.0;
    c
}

/// How freeze and the SEED input move during a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gate {
    /// Neither: the SEED button is latched and nothing is patched.
    Free,
    /// As [`Gate::Free`], with freeze held from 1 s before the point's end to 0.25 s before it,
    /// then recording again (the freeze tail's crossfade on resume). Every point's grains or taps
    /// read recorded audio before the freeze starts.
    Freeze,
    /// A pulse on the SEED input (5 ms every 125 ms) with the button released: each pulse seeds
    /// a grain; in the delay mode the pulses clock the delay.
    Seed,
}

impl Gate {
    /// Every pattern, in sweep order.
    pub const ALL: [Self; 3] = [Self::Free, Self::Freeze, Self::Seed];

    fn label(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Freeze => "freeze",
            Self::Seed => "seed",
        }
    }
}

/// One point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The controls (the SEED button changes with [`Gate::Seed`]).
    pub base: Controls,
    /// How freeze and the SEED input move.
    pub gate: Gate,
}

impl Point {
    /// Whether the point is in the delay mode (SIZE above 0.99).
    #[must_use]
    pub fn delay(&self) -> bool {
        self.base.size.base > 0.99
    }

    /// How far back TIME reaches, as a fraction of the buffer: its knob plus half its random
    /// spread (an attenuverter of 1 spreads the positions over the whole buffer).
    #[must_use]
    pub fn reach(&self) -> f32 {
        (self.base.time.base + 0.5 * self.base.time.attenuverter.abs()).min(1.0)
    }

    /// The point's length at `quality`, frames, in whole module blocks, long enough for the grains
    /// or taps to read recorded audio (a fresh buffer is silent until the recording reaches their
    /// position). A granular point: [`GRANULAR_POINT_SECONDS`], or 1.5 s past the moment the
    /// recording reaches [`Point::reach`], whichever is longer. A delay point:
    /// [`DELAY_POINT_SECONDS`], or 1 s past the moment the longest tap reads recorded audio after
    /// the 2.25 s crossing into the delay mode, whichever is longer. Away from noon, DENSITY sets
    /// the delay's base interval to `2^-(|density - 0.5| * 18)` of the buffer (1/22.6 at the
    /// center's 0.75); TIME multiplies it by up to 16, and the longest tap is one interval more,
    /// but never longer than the buffer (the firmware constrains every tap to it).
    #[must_use]
    pub fn frames(&self, quality: Quality) -> usize {
        let buffer = quality.buffer_seconds();
        let seconds = if self.delay() {
            let interval = buffer * (-(self.base.density - 0.5).abs() * 18.0).exp2();
            DELAY_POINT_SECONDS.max(2.25 + ((self.reach() * 15.0 + 2.0) * interval).min(buffer) + 1.0)
        } else {
            GRANULAR_POINT_SECONDS.max(self.reach() * buffer + 1.5)
        };
        let media = quality.media;
        let rate = usize::try_from(media.sample_rate()).unwrap_or(48_000);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let frames = (seconds * rate as f32) as usize;
        frames.div_ceil(media.block_size()) * media.block_size()
    }

    /// The controls of every block.
    #[must_use]
    pub fn controls(&self) -> Controls {
        let mut c = self.base;
        if self.gate == Gate::Seed {
            c.seed_button = SeedButton::Off;
        }
        c
    }

    /// Freeze at frame `frame` of a point `frames` long at `media`.
    #[must_use]
    pub fn freeze(&self, media: Media, frame: usize, frames: usize) -> bool {
        let rate = usize::try_from(media.sample_rate()).unwrap_or(48_000);
        self.gate == Gate::Freeze && frame + rate >= frames && frame + rate / 4 < frames
    }

    /// The SEED input's gate flags for frames `start..start + out.len()` at `media`, or `None` when
    /// nothing is patched. The gate is high for 5 ms of every 125 ms.
    #[must_use]
    pub fn seed(&self, media: Media, start: usize, out: &mut [u8]) -> Option<()> {
        if self.gate != Gate::Seed {
            return None;
        }
        let rate = usize::try_from(media.sample_rate()).unwrap_or(48_000);
        let (period, high) = (rate / 8, rate / 200);
        let gate = |n: usize| n % period < high;
        for (i, flags) in out.iter_mut().enumerate() {
            let n = start + i;
            let (now, before) = (gate(n), n > 0 && gate(n - 1));
            *flags = match (now, before) {
                (true, false) => 3,
                (true, true) => 1,
                (false, true) => 4,
                (false, false) => 0,
            };
        }
        Some(())
    }

    /// A stable label, such as `granular time0.50 ... free`.
    #[must_use]
    pub fn label(&self) -> String {
        let c = &self.base;
        format!(
            "{} time{:.2} size{:.2} shape{:.2} pitch{} ta{:.2} sa{:.2} ha{:.2} pa{:.2} den{:.2} fb{:.2} wet{:.2} rev{:.2} {}",
            if self.delay() { "delay" } else { "granular" },
            c.time.base,
            c.size.base,
            c.shape.base,
            c.pitch.base,
            c.time.attenuverter,
            c.size.attenuverter,
            c.shape.attenuverter,
            c.pitch.attenuverter,
            c.density,
            c.feedback,
            c.dry_wet,
            c.reverb,
            self.gate.label()
        )
    }
}

/// The 285 points of one quality, in a fixed order.
#[must_use]
pub fn points() -> Vec<Point> {
    let mut points = Vec::new();
    for gate in Gate::ALL {
        for (center, delay) in [(center(), false), (delay_center(), true)] {
            points.push(Point { base: center, gate });
            for axis in Axis::ALL {
                if delay && axis == Axis::Size {
                    continue;
                }
                for level in levels(axis) {
                    // SIZE 1 is the delay mode: the delay center, rendered in its own sweep.
                    #[allow(clippy::float_cmp)]
                    if !delay && axis == Axis::Size && level == 1.0 {
                        continue;
                    }
                    let mut base = center;
                    #[allow(clippy::float_cmp)] // Exact constants: the center is rendered once.
                    if level == *axis.slot(&mut base) {
                        continue;
                    }
                    *axis.slot(&mut base) = level;
                    points.push(Point { base, gate });
                }
            }
        }
    }
    points
}

/// The label of an axis (for reports).
#[must_use]
pub fn axis_label(axis: Axis) -> &'static str {
    axis.label()
}

/// The input every point is rendered with, `frames` stereo float frames, from integer arithmetic
/// only (so every runner builds the same samples; each float is an int16 value / 32768, exact): on
/// the left a triangle of period 145 samples, on the right a sawtooth of period 97, both with noise
/// from a linear congruential generator. In every 48000 frames: full level for 30000, silence for
/// 6000 (the tails decay toward the subnormal range), a quarter level for 12000; and overdriven to
/// one and a half times full scale for 2000 frames in every 96000 (from frame 10000), into the
/// quantizers' and the limiters' clipping.
#[must_use]
pub fn input_signal(frames: usize) -> Vec<[f32; 2]> {
    let mut state = 0x0241_1A9E_u32;
    (0..frames)
        .map(|n| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = i32::from((state >> 16) as u16 as i16) >> 3;
            let n32 = u32::try_from(n).unwrap_or(0);
            let tri_phase = i32::try_from((n32 % 145) * 65_536 / 145).unwrap_or(0);
            let triangle = (tri_phase - 32_768).abs() * 2 - 32_768;
            let saw = i32::try_from((n32 % 97) * 65_536 / 97).unwrap_or(0) - 32_768;
            let (mut l, mut r) = (triangle * 3 / 8 + noise, saw / 4 - noise / 2);
            let within = n % 48_000;
            if (30_000..36_000).contains(&within) {
                (l, r) = (0, 0);
            } else if within >= 36_000 {
                (l, r) = (l / 4, r / 4);
            }
            #[allow(clippy::cast_precision_loss)]
            let mut frame = [l as f32 / 32_768.0, r as f32 / 32_768.0];
            if (10_000..12_000).contains(&(n % 96_000)) {
                frame = [frame[0] * 4.0, frame[1] * 6.0];
            }
            frame
        })
        .collect()
}

/// The longest point at any quality, frames (a delay point at DENSITY noon, where the taps reach
/// the whole buffer, in the clean tape's mono buffer: 24.6 s at 48 kHz).
#[must_use]
pub fn longest_point_frames() -> usize {
    Quality::all().into_iter().flat_map(|q| points().into_iter().map(move |p| p.frames(q))).max().unwrap_or(0)
}

/// Renders one point from a fresh chain on the module's memory, over the first [`Point::frames`]
/// frames of `input` (take it from [`input_signal`] at [`longest_point_frames`]).
///
/// # Errors
///
/// An [`Error`] from the oracle; [`Error::BadBlock`] when `input` is shorter than the point.
pub fn render_point(session: &Session, quality: Quality, point: &Point, input: &[[f32; 2]]) -> Result<Vec<[f32; 2]>, Error> {
    session.seed(SEED);
    let mut chain = session.texture(quality.media, quality.mono)?;
    let frames = point.frames(quality);
    let input = input.get(..frames).ok_or(Error::BadBlock)?;
    let block = quality.media.block_size();
    let controls = point.controls();
    let mut out = vec![[0.0_f32; 2]; frames];
    let mut freeze = vec![false; block];
    let mut seed = vec![0_u8; block];
    for (b, (i, o)) in input.chunks(block).zip(out.chunks_mut(block)).enumerate() {
        let start = b * block;
        for (k, f) in freeze.iter_mut().enumerate() {
            *f = point.freeze(quality.media, start + k, frames);
        }
        let seed = point.seed(quality.media, start, &mut seed).map(|()| &seed[..]);
        chain.process(&controls, &freeze, seed, i, o)?;
    }
    Ok(out)
}

/// FNV-1a 64 over the frames' bits (left then right, little-endian).
#[must_use]
pub fn digest_frames(frames: &[[f32; 2]]) -> u64 {
    crate::sweep::digest(frames.iter().flatten().flat_map(|s| s.to_bits().to_le_bytes()))
}

/// One line per quality and point, `<quality> <point> <digest>`, for the whole sweep, then one per
/// synth resolution and point and one per input-stage run. Two runs with equal lines rendered
/// bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let input = input_signal(longest_point_frames());
    let mut lines = Vec::new();
    for quality in Quality::all() {
        for point in points() {
            let out = render_point(session, quality, &point, &input)?;
            lines.push(format!("{} {} {:016x}", quality.label(), point.label(), digest_frames(&out)));
        }
    }
    for resolution in 0..4 {
        for point in synth_points() {
            let out = render_synth_point(session, resolution, &point)?;
            lines.push(format!("synth{resolution} {} {:016x}", point.label(), digest_frames(&out)));
        }
    }
    for (rate, mode) in INPUT_RUNS {
        let out = render_input_run(session, rate, mode)?;
        lines.push(format!("input {rate} {mode} {:016x}", digest_frames(&out)));
    }
    Ok(lines)
}

/// The length of a synth point, frames: 2 seconds at 48 kHz.
pub const SYNTH_POINT_FRAMES: usize = 96_000;

/// The synth's center: the granular [`center`] with TIME at noon (the middle of the wave scan),
/// FEEDBACK 0.25 (the bank: on the synth FEEDBACK picks one of 12) and DRY/WET at noon (the
/// oscillator and the wavetable grains together).
#[must_use]
pub fn synth_center() -> Controls {
    let mut c = center();
    c.time.base = 0.5;
    c.dry_wet = 0.5;
    c
}

/// The synth's 147 points, per resolution: each axis over its five levels around
/// [`synth_center`] (1 + 12 × 4 = 49), in each [`Gate`] pattern.
#[must_use]
pub fn synth_points() -> Vec<Point> {
    let mut points = Vec::new();
    let center = synth_center();
    for gate in Gate::ALL {
        points.push(Point { base: center, gate });
        for axis in Axis::ALL {
            for level in levels(axis) {
                let mut base = center;
                #[allow(clippy::float_cmp)] // Exact constants: the center is rendered once.
                if level == *axis.slot(&mut base) {
                    continue;
                }
                *axis.slot(&mut base) = level;
                points.push(Point { base, gate });
            }
        }
    }
    points
}

/// Renders one synth point from a fresh synth at `resolution` (0 to 3), seeded with [`SEED`], in
/// the module's 12-frame blocks at 48 kHz, for [`SYNTH_POINT_FRAMES`] (the synth has no input).
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_synth_point(session: &Session, resolution: usize, point: &Point) -> Result<Vec<[f32; 2]>, Error> {
    session.seed(SEED);
    let mut synth = session.texture_synth()?;
    let block = Media::Digital.block_size();
    let controls = point.controls();
    let mut out = vec![[0.0_f32; 2]; SYNTH_POINT_FRAMES];
    let mut freeze = vec![false; block];
    let mut seed = vec![0_u8; block];
    for (b, o) in out.chunks_mut(block).enumerate() {
        let start = b * block;
        for (k, f) in freeze.iter_mut().enumerate() {
            *f = point.freeze(Media::Digital, start + k, SYNTH_POINT_FRAMES);
        }
        let seed = point.seed(Media::Digital, start, &mut seed).map(|()| &seed[..]);
        synth.render(&controls, resolution, &freeze, seed, o)?;
    }
    Ok(out)
}

/// The input stage's runs: each codec rate with each soft-clipping mode.
pub const INPUT_RUNS: [(f32, usize); 9] =
    [(24_000.0, 0), (24_000.0, 1), (24_000.0, 2), (32_000.0, 0), (32_000.0, 1), (32_000.0, 2), (48_000.0, 0), (48_000.0, 1), (48_000.0, 2)];

/// The length of an input-stage run, seconds.
pub const INPUT_RUN_SECONDS: usize = 10;

/// One block of an input-stage run (see [`input_step`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputStep {
    /// `GainControl::Init(rate, mode, false)` before the block (a codec restart).
    pub reinit: bool,
    /// The codec is running.
    pub active: bool,
    /// The gain setting: 0 automatic, 1 to 255 manual.
    pub gain: u8,
    /// Which inputs are unpatched.
    pub normalized: [bool; 2],
}

/// The schedule of an input-stage run at block `block` (the module's blocks last 1/4000 s at every
/// rate):
/// the automatic gain through its 1-second wait and 5-second learning phase (with a codec stall at
/// 3 s and a restart that keeps the learning at 6.5 s), then manual gains of 40, 128 and 255, and
/// at the end the right input, then the left, then both unpatched.
#[must_use]
pub fn input_step(block: usize) -> InputStep {
    #[allow(clippy::cast_precision_loss)]
    let t = block as f32 / 4000.0;
    let gain = if t < 7.0 { 0 } else { [40, 128, 255][(t as usize) % 3] };
    let normalized = if (9.0..9.25).contains(&t) {
        [false, true]
    } else if (9.25..9.5).contains(&t) {
        [true, false]
    } else if (9.5..9.75).contains(&t) {
        [true, true]
    } else {
        [false, false]
    };
    InputStep { reinit: block == 26_000, active: !(3.0..3.25).contains(&t), gain, normalized }
}

/// The raw codec frames of an input-stage run at `rate`: [`input_signal`] at 32-bit full scale
/// (each value an int16 times 65536, the overdriven stretches clipped to the 32-bit range).
#[must_use]
pub fn input_frames(rate: f32) -> Vec<[i32; 2]> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = rate as usize * INPUT_RUN_SECONDS;
    input_signal(frames)
        .into_iter()
        .map(|f| {
            f.map(|x| {
                #[allow(clippy::cast_possible_truncation)]
                let v = (f64::from(x) * 2_147_483_648.0).clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                v
            })
        })
        .collect()
}

/// Renders one input-stage run through the oracle.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_input_run(session: &Session, rate: f32, mode: usize) -> Result<Vec<[f32; 2]>, Error> {
    let mut stage = session.texture_input(rate, mode)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let block = rate as usize / 4000;
    let input = input_frames(rate);
    let mut out = vec![[0.0_f32; 2]; input.len()];
    for (b, (i, o)) in input.chunks(block).zip(out.chunks_mut(block)).enumerate() {
        let step = input_step(b);
        if step.reinit {
            stage.init(rate, mode, false)?;
        }
        stage.process(step.active, step.gain, step.normalized, i, o)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(points().len(), 285);
        assert_eq!(Quality::all().len(), 8);
        let labels: std::collections::HashSet<_> = points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 285, "every point is rendered once");
        assert!(points().iter().all(|p| crate::texture::controls_in_range(&p.controls())));
        assert_eq!(points().iter().filter(|p| p.delay()).count(), 3 * 46);
        assert_eq!(longest_point_frames(), 1_180_008);
        let granular = Point { base: center(), gate: Gate::Free };
        let q = |media, mono| Quality { media, mono };
        assert_eq!(granular.frames(q(Media::Digital, false)), 96_000);
        assert_eq!(granular.frames(q(Media::VintageDigital, false)), 73_600, "0.8 s back, plus 1.5 s");
        assert_eq!(granular.frames(q(Media::DirtyTape, true)), 112_800, "3.2 s back, plus 1.5 s");
        let delay = Point { base: delay_center(), gate: Gate::Free };
        let near = delay.frames(q(Media::Digital, false));
        assert!((185_688..=185_712).contains(&near), "2.25 s + 3.5 intervals of 4 s / 22.6 + 1 s: {near}");
        let deep = delay.frames(q(Media::DirtyTape, true));
        assert!((196_788..=196_800).contains(&deep), "8.2 s at 24 kHz: {deep}");
    }

    #[test]
    fn the_synth_sweep_and_the_input_runs_have_the_documented_shape() {
        assert_eq!(synth_points().len(), 147);
        let labels: std::collections::HashSet<_> = synth_points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 147);
        assert!(synth_points().iter().all(|p| crate::texture::controls_in_range(&p.controls())));
        assert_eq!(input_frames(24_000.0).len(), 240_000);
        assert!(input_step(26_000).reinit && !input_step(26_001).reinit);
        assert!(!input_step(12_000).active && input_step(13_000).active);
        assert_eq!(input_step(27_999).gain, 0);
        assert_eq!(input_step(28_000).gain, 128);
        assert_eq!(input_step(38_000).normalized, [true, true]);
    }

    #[test]
    fn freeze_and_seed_follow_their_patterns() {
        let p = Point { base: center(), gate: Gate::Freeze };
        let d = Media::Digital;
        assert!(!p.freeze(d, 47_999, 96_000) && p.freeze(d, 48_000, 96_000) && p.freeze(d, 83_999, 96_000) && !p.freeze(d, 84_000, 96_000));
        let mut flags = [0_u8; 12];
        assert!(p.seed(Media::Digital, 0, &mut flags).is_none());
        let s = Point { base: center(), gate: Gate::Seed };
        assert_eq!(s.controls().seed_button, SeedButton::Off);
        assert!(s.seed(Media::Digital, 0, &mut flags).is_some());
        assert_eq!(flags[0], 3, "rising at frame 0");
        assert_eq!(flags[1], 1);
        assert!(s.seed(Media::Digital, 240, &mut flags).is_some());
        assert_eq!(flags[0], 4, "falling after 5 ms at 48 kHz");
        assert!(s.seed(Media::Digital, 6000, &mut flags).is_some());
        assert_eq!(flags[0], 3, "the next pulse 125 ms later");
    }

    #[test]
    fn the_input_is_deterministic_and_reaches_the_clipping() {
        let a = input_signal(96_000);
        assert_eq!(a, input_signal(96_000));
        assert!(a.iter().any(|[l, _]| *l > 1.0) && a.iter().any(|[_, r]| *r < -1.0));
        assert!(a[30_000..36_000].iter().all(|f| *f == [0.0, 0.0]));
        assert!(a.iter().all(|[l, r]| l.abs() <= crate::texture::INPUT_LIMIT && r.abs() <= crate::texture::INPUT_LIMIT));
    }
}

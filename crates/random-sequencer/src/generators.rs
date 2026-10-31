//! The firmware's main loop around the generators (`Process` in `marbles/marbles.cc`): at 32 kHz
//! in blocks of 5 frames, the t generator first, then the X and Y generators, with the knob values
//! the CV reader hands them. Nulled against the oracle frame for frame.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::quantizer::Scale;
use crate::random::RandomStream;
use crate::stmlib::HysteresisQuantizer2;
use crate::t_generator::{Model, Ramps, Range, TGenerator};
use crate::xy_generator::{ClockSource, ControlMode, GroupSettings, NUM_SCALES, VoltageRange, XYGenerator};
use crate::{BLOCK_SIZE, SAMPLE_RATE};

/// The deja vu loop lengths the length knob steps through (`loop_length` in `marbles/marbles.cc`).
pub static LOOP_LENGTHS: [i32; 75] = [
    1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 8, 10, 10, 10, 12, 12, 12, 12, 12, 12, 12, 14, 14, 16,
];

/// The Y channel's clock divisions (`y_divider_ratios` in `marbles/marbles.cc`), as `(p, q)`.
pub static Y_DIVIDERS: [(i32, i32); 12] = [(1, 64), (1, 48), (1, 32), (1, 24), (1, 16), (1, 12), (1, 8), (1, 6), (1, 4), (1, 3), (1, 2), (1, 1)];

/// `DejaVuState`.
pub const DEJA_VU_OFF: i32 = 0;
/// See [`DEJA_VU_OFF`].
pub const DEJA_VU_ON: i32 = 1;
/// See [`DEJA_VU_OFF`].
pub const DEJA_VU_LOCKED: i32 = 2;

/// The deja vu knob through the main loop's dead band around 12 o'clock: below 0.47 and above 0.53
/// stretched to fill 0 to 1, 0.5 (the loop locked) between.
#[must_use]
#[allow(clippy::excessive_precision)] // The firmware's constant, as written.
pub fn deja_vu_dead_band(deja_vu: F) -> F {
    if deja_vu < 0.47 {
        deja_vu * 1.063_829_787_23
    } else if deja_vu > 0.53 {
        0.5 + (deja_vu - 0.53) * 1.063_829_787_23
    } else {
        F(0.5)
    }
}

/// What the firmware's main loop reads each block (as the oracle's `Controls`; see there for each
/// field's range).
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(missing_docs)]
pub struct Controls {
    pub t_model: i32,
    pub t_range: i32,
    pub t_rate: f32,
    pub t_bias: f32,
    pub t_jitter: f32,
    pub deja_vu: f32,
    pub deja_vu_length: f32,
    pub t_deja_vu: i32,
    pub x_deja_vu: i32,
    pub t_pulse_width_mean: i32,
    pub t_pulse_width_std: i32,
    pub x_control_mode: i32,
    pub x_range: i32,
    pub x_register_mode: i32,
    pub x_register_value: f32,
    pub x_spread: f32,
    pub x_bias: f32,
    pub x_steps: f32,
    pub x_scale: i32,
    pub y_spread: i32,
    pub y_bias: i32,
    pub y_steps: i32,
    pub y_divider: i32,
    pub y_range: i32,
    pub t_reset: i32,
    pub x_reset: i32,
    pub t_external: i32,
    pub xy_clock_source: i32,
}

/// The module's generators: the t generator, the X and Y generators and their random source.
#[derive(Clone, Debug)]
pub struct Generators {
    stream: RandomStream,
    t: TGenerator,
    xy: XYGenerator,
    deja_vu_length_quantizer: HysteresisQuantizer2,
    x: GroupSettings,
    y: GroupSettings,
    ramps: Ramps<BLOCK_SIZE>,
}

impl Generators {
    /// The generators as the firmware's `Init` leaves them, the random source seeded with `seed`
    /// (the module seeds it with 1), the six preset scales loaded.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        let mut deja_vu_length_quantizer = HysteresisQuantizer2::default();
        deja_vu_length_quantizer.init(LOOP_LENGTHS.len() as i32, 0.25, false);
        let mut stream = RandomStream::new(seed);
        let t = TGenerator::new(&mut stream, SAMPLE_RATE);
        let mut xy = XYGenerator::new(&mut stream, SAMPLE_RATE);
        for (i, scale) in crate::scales::PRESETS.iter().enumerate().take(NUM_SCALES) {
            xy.load_scale(i, &scale.to_scale());
        }
        Self { stream, t, xy, deja_vu_length_quantizer, x: GroupSettings::default(), y: GroupSettings::default(), ramps: Ramps::default() }
    }

    /// Loads `scale` into slot `index` (0 to 5) of the three X channels.
    pub fn load_scale(&mut self, index: usize, scale: &Scale) {
        if index < NUM_SCALES {
            self.xy.load_scale(index, scale);
        }
    }

    /// One block of `gates.len()` frames (1 to [`BLOCK_SIZE`]): three gates per frame (t1, the
    /// master clock t2, t3) and four voltages (X1, X2, X3, Y). A clock the controls use but the
    /// caller does not pass reads as low.
    pub fn process(&mut self, c: &Controls, t_clock: Option<&[u8]>, xy_clock: Option<&[u8]>, gates: &mut [[u8; 3]], voltages: &mut [[f32; 4]]) {
        let size = gates.len().min(voltages.len()).min(BLOCK_SIZE);
        let mut t_flags = [0_u8; BLOCK_SIZE];
        let mut xy_flags = [0_u8; BLOCK_SIZE];
        for i in 0..size {
            t_flags[i] = t_clock.and_then(|f| f.get(i)).copied().unwrap_or(0);
            xy_flags[i] = xy_clock.and_then(|f| f.get(i)).copied().unwrap_or(0);
        }
        let deja_vu = deja_vu_dead_band(F(c.deja_vu));
        let deja_vu_length = self.deja_vu_length_quantizer.lookup(&LOOP_LENGTHS, F(c.deja_vu_length));
        let mut t_section_reset = c.t_reset != 0;

        let t = &mut self.t;
        t.set_model(Model::from_index(c.t_model));
        t.set_range(match c.t_range {
            0 => Range::Quarter,
            1 => Range::Normal,
            _ => Range::Times4,
        });
        t.set_rate(F(c.t_rate));
        t.set_bias(F(c.t_bias));
        t.set_jitter(F(c.t_jitter));
        t.set_deja_vu(match c.t_deja_vu {
            DEJA_VU_LOCKED => F(0.5),
            DEJA_VU_ON => deja_vu,
            _ => F::ZERO,
        });
        t.set_length(deja_vu_length);
        t.set_pulse_width_mean(F::from_i32(c.t_pulse_width_mean) / 256.0);
        t.set_pulse_width_std(F::from_i32(c.t_pulse_width_std) / 256.0);
        let mut gate = [false; 2 * BLOCK_SIZE];
        t.process(&mut self.stream, c.t_external != 0, &mut t_section_reset, &t_flags[..size], &mut self.ramps, &mut gate, size);

        let x = &mut self.x;
        x.control_mode = ControlMode::from_index(c.x_control_mode);
        x.voltage_range = VoltageRange::from_index(c.x_range % 3);
        x.register_mode = c.x_register_mode != 0;
        x.register_value = F(c.x_register_value);
        x.spread = F(c.x_spread);
        x.bias = F(c.x_bias);
        x.steps = F(c.x_steps);
        x.deja_vu = match c.x_deja_vu {
            DEJA_VU_LOCKED => F(0.5),
            DEJA_VU_ON => deja_vu,
            _ => F::ZERO,
        };
        x.length = deja_vu_length;
        x.ratio = crate::ramp::Ratio { p: 1, q: 1 };
        let y = &mut self.y;
        y.control_mode = ControlMode::Identical;
        y.voltage_range = VoltageRange::from_index(c.y_range);
        y.register_mode = false;
        y.register_value = F::ZERO;
        y.spread = F::from_i32(c.y_spread) / 256.0;
        y.bias = F::from_i32(c.y_bias) / 256.0;
        y.steps = F::from_i32(c.y_steps) / 256.0;
        y.deja_vu = F::ZERO;
        y.length = 1;
        let (p, q) = Y_DIVIDERS[(usize::try_from(c.y_divider).unwrap_or(0).min(255) * 12) >> 8];
        y.ratio = crate::ramp::Ratio { p, q };
        let scale = usize::try_from(c.x_scale).unwrap_or(0).min(NUM_SCALES - 1);
        x.scale_index = scale;
        y.scale_index = scale;

        let clock_source = ClockSource::from_index(c.xy_clock_source);
        let mut x_section_reset = c.x_reset != 0;
        if clock_source != ClockSource::External {
            x_section_reset |= t_section_reset;
        }
        let mut out = [F::ZERO; 4 * BLOCK_SIZE];
        let (xs, ys) = (self.x, self.y);
        self.xy.process(&mut self.stream, clock_source, &xs, &ys, &mut x_section_reset, &xy_flags[..size], &mut self.ramps, &mut out, size);
        for i in 0..size {
            gates[i] = [u8::from(gate[2 * i]), u8::from(self.ramps.master[i] < 0.5), u8::from(gate[2 * i + 1])];
            for k in 0..4 {
                voltages[i][k] = out[4 * i + k].0;
            }
        }
    }
}

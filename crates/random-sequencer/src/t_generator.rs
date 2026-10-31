//! The gate generator: `marbles/random/t_generator.{h,cc}`. A master clock (internal, or recovered
//! from a clock input) ticks; on each tick a random vector decides, by the model, which of the two
//! gate channels fire and with what pulse width, and the jitter bends the next tick's timing.

#![forbid(unsafe_code)]

use crate::distributions::{beta_sample, fast_beta_sample};
use crate::float::F;
use crate::ramp::{Ratio, SlaveRamp};
use crate::ramp_extractor::RampExtractor;
use crate::random::{RandomSequence, RandomStream};
use crate::resources::LUT_LOGIT;
use crate::stmlib::{HysteresisQuantizer2, constrain, index, semitones_to_ratio};

/// `kNumTChannels`.
pub const NUM_T_CHANNELS: usize = 2;
const MARKOV_HISTORY_SIZE: usize = 16;
const NUM_DRUM_PATTERNS: usize = 18;
const DRUM_PATTERN_SIZE: usize = 8;
const NUM_DIVIDER_PATTERNS: usize = 17;

/// `TGeneratorModel`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Model {
    /// One coin toss per tick: channel 1 or channel 2 (`T_GENERATOR_MODEL_COMPLEMENTARY_BERNOULLI`).
    #[default]
    CoinToss,
    /// Bursts at multiples and divisions of the clock (`CLUSTERS`).
    Clusters,
    /// Eight-step drum patterns (`DRUMS`).
    Drums,
    /// A coin toss per channel (`INDEPENDENT_BERNOULLI`).
    Independent,
    /// Fixed divisions and multiplications of the clock (`DIVIDER`).
    Divider,
    /// Channel 1, channel 2 or neither (`THREE_STATES`).
    ThreeStates,
    /// A Markov model of the last 16 ticks (`MARKOV`; the module does not offer it).
    Markov,
}

impl Model {
    /// The firmware's model by index (0 to 6; anything else is the Markov model's index clamped).
    #[must_use]
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Self::CoinToss,
            1 => Self::Clusters,
            2 => Self::Drums,
            3 => Self::Independent,
            4 => Self::Divider,
            5 => Self::ThreeStates,
            _ => Self::Markov,
        }
    }
}

/// `TGeneratorRange`: the internal clock at a quarter, normal (2 Hz at the rate's center) or four
/// times, and for a patched clock the same factors on the division.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Range {
    Quarter,
    #[default]
    Normal,
    Times4,
}

/// `DividerPattern`: each channel's ratio to the master clock and the pattern's length in ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DividerPattern {
    pub ratios: [Ratio; NUM_T_CHANNELS],
    pub length: i32,
}

const fn pattern(a: (i32, i32), b: (i32, i32), length: i32) -> DividerPattern {
    DividerPattern { ratios: [Ratio { p: a.0, q: a.1 }, Ratio { p: b.0, q: b.1 }], length }
}

static DIVIDER_PATTERNS: [DividerPattern; NUM_DIVIDER_PATTERNS] = [
    pattern((1, 1), (1, 1), 1),
    pattern((1, 1), (2, 1), 1),
    pattern((1, 2), (1, 1), 2),
    pattern((1, 1), (4, 1), 1),
    pattern((1, 2), (2, 1), 2),
    pattern((1, 1), (3, 2), 2),
    pattern((1, 4), (4, 1), 4),
    pattern((1, 4), (2, 1), 4),
    pattern((1, 2), (3, 2), 2),
    pattern((1, 1), (8, 1), 1),
    pattern((1, 1), (3, 1), 1),
    pattern((1, 3), (1, 1), 3),
    pattern((1, 1), (5, 4), 4),
    pattern((1, 2), (5, 4), 4),
    pattern((1, 1), (6, 1), 1),
    pattern((1, 3), (2, 1), 3),
    pattern((1, 1), (16, 1), 1),
];

static FIXED_DIVIDER_PATTERNS: [DividerPattern; NUM_DIVIDER_PATTERNS] = [
    pattern((8, 1), (1, 8), 8),
    pattern((6, 1), (1, 6), 6),
    pattern((4, 1), (1, 4), 4),
    pattern((3, 1), (1, 3), 3),
    pattern((2, 1), (1, 2), 2),
    pattern((3, 2), (2, 3), 6),
    pattern((4, 3), (3, 4), 12),
    pattern((5, 4), (4, 5), 20),
    pattern((1, 1), (1, 1), 1),
    pattern((4, 5), (5, 4), 20),
    pattern((3, 4), (4, 3), 12),
    pattern((2, 2), (3, 2), 6),
    pattern((1, 2), (2, 1), 2),
    pattern((1, 3), (3, 1), 3),
    pattern((1, 4), (4, 1), 4),
    pattern((1, 6), (6, 1), 6),
    pattern((1, 8), (8, 1), 8),
];

/// `input_divider_ratios`: the patched clock's divisions the rate knob picks.
pub static INPUT_DIVIDER_RATIOS: [Ratio; 9] = [
    Ratio { p: 1, q: 4 },
    Ratio { p: 1, q: 3 },
    Ratio { p: 1, q: 2 },
    Ratio { p: 2, q: 3 },
    Ratio { p: 1, q: 1 },
    Ratio { p: 3, q: 2 },
    Ratio { p: 2, q: 1 },
    Ratio { p: 3, q: 1 },
    Ratio { p: 4, q: 1 },
];

static DRUM_PATTERNS: [[u8; DRUM_PATTERN_SIZE]; NUM_DRUM_PATTERNS] = [
    [1, 0, 0, 0, 2, 0, 0, 0],
    [0, 0, 1, 0, 2, 0, 0, 0],
    [1, 0, 1, 0, 2, 0, 0, 0],
    [0, 0, 1, 0, 2, 0, 0, 2],
    [1, 0, 1, 0, 2, 0, 1, 0],
    [0, 2, 1, 0, 2, 0, 0, 2],
    [1, 0, 0, 0, 2, 0, 1, 0],
    [0, 2, 1, 0, 2, 0, 1, 2],
    [1, 0, 0, 1, 2, 0, 0, 0],
    [0, 2, 1, 1, 2, 0, 1, 2],
    [1, 0, 0, 1, 2, 0, 1, 0],
    [0, 2, 1, 1, 2, 2, 1, 2],
    [1, 0, 0, 1, 2, 0, 1, 2],
    [0, 2, 0, 1, 2, 0, 1, 2],
    [1, 0, 1, 1, 2, 0, 1, 2],
    [2, 0, 1, 2, 0, 1, 2, 0],
    [1, 2, 1, 1, 2, 0, 1, 2],
    [2, 0, 1, 2, 0, 1, 2, 2],
];

/// `RandomVector`: the pulse widths' and the two channels' uniform draws, the deja vu draw and the
/// jitter's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RandomVector(pub [F; 6]);

impl RandomVector {
    fn pulse_width(&self, i: usize) -> F {
        self.0[i]
    }
    fn u(&self, i: usize) -> F {
        self.0[2 + i]
    }
    fn p(&self) -> F {
        self.0[4]
    }
    /// The jitter's draw.
    #[must_use]
    pub fn jitter(&self) -> F {
        self.0[5]
    }
}

/// The three ramp buffers of a block the t generator writes (`Ramps`): the master ramp, the
/// external ramp (the recovered clock, or the master when internal) and the two slave ramps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramps<const B: usize> {
    pub master: [F; B],
    pub external: [F; B],
    pub slave: [[F; B]; NUM_T_CHANNELS],
}

impl<const B: usize> Default for Ramps<B> {
    fn default() -> Self {
        Self { master: [F::ZERO; B], external: [F::ZERO; B], slave: [[F::ZERO; B]; NUM_T_CHANNELS] }
    }
}

/// `TGenerator`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TGenerator {
    one_hertz: F,
    pub(crate) model: Model,
    range: Range,
    rate: F,
    pub(crate) bias: F,
    jitter: F,
    pulse_width_mean: F,
    pulse_width_std: F,
    master_phase: F,
    pub(crate) jitter_multiplier: F,
    phase_difference: F,
    previous_external_ramp_value: F,
    use_external_clock: bool,
    divider_pattern_length: i32,
    streak_counter: [i32; MARKOV_HISTORY_SIZE],
    markov_history: [i32; MARKOV_HISTORY_SIZE],
    markov_history_ptr: i32,
    drum_pattern_step: usize,
    drum_pattern_index: usize,
    pub(crate) sequence: RandomSequence,
    ramp_extractor: RampExtractor,
    pub(crate) slave_ramp: [SlaveRamp; NUM_T_CHANNELS],
    bias_quantizer: HysteresisQuantizer2,
    rate_quantizer: HysteresisQuantizer2,
    /// The divider pattern chosen at the last tick that chose one (bookkeeping for the
    /// transport-locked sequencer; the firmware keeps it only in the slave ramps).
    pub(crate) pattern: DividerPattern,
    /// Whether the last configuration started a new divider pattern.
    pub(crate) pattern_started: bool,
    /// Per channel, whether the last configuration restarted its Bernoulli ramp.
    pub(crate) restarted: [bool; NUM_T_CHANNELS],
}

impl TGenerator {
    /// `Init(random_stream, sr)`.
    #[must_use]
    pub fn new(stream: &mut RandomStream, sample_rate: f32) -> Self {
        let mut ramp_extractor = RampExtractor::default();
        ramp_extractor.init(F(1000.0) / sample_rate);
        let mut slave_ramp = [SlaveRamp::default(); NUM_T_CHANNELS];
        for s in &mut slave_ramp {
            s.init();
        }
        let mut bias_quantizer = HysteresisQuantizer2::default();
        // Ideal hysteresis: voltage error / voltage range * number of values.
        bias_quantizer.init(NUM_DIVIDER_PATTERNS as i32, 0.1, false);
        let mut rate_quantizer = HysteresisQuantizer2::default();
        rate_quantizer.init(INPUT_DIVIDER_RATIOS.len() as i32, 0.05, false);
        Self {
            one_hertz: 1.0 / F(sample_rate),
            model: Model::CoinToss,
            range: Range::Normal,
            rate: F::ZERO,
            bias: F(0.5),
            jitter: F::ZERO,
            pulse_width_mean: F::ZERO,
            pulse_width_std: F::ZERO,
            master_phase: F::ZERO,
            jitter_multiplier: F::ONE,
            phase_difference: F::ZERO,
            previous_external_ramp_value: F::ZERO,
            use_external_clock: false,
            divider_pattern_length: 0,
            streak_counter: [0; MARKOV_HISTORY_SIZE],
            markov_history: [0; MARKOV_HISTORY_SIZE],
            markov_history_ptr: 0,
            drum_pattern_step: 0,
            drum_pattern_index: 0,
            sequence: RandomSequence::new(stream),
            ramp_extractor,
            slave_ramp,
            bias_quantizer,
            rate_quantizer,
            pattern: DividerPattern::default(),
            pattern_started: false,
            restarted: [false; NUM_T_CHANNELS],
        }
    }

    /// `set_model`.
    pub fn set_model(&mut self, model: Model) {
        self.model = model;
    }
    /// `set_range`.
    pub fn set_range(&mut self, range: Range) {
        self.range = range;
    }
    /// `set_rate`.
    pub fn set_rate(&mut self, rate: F) {
        self.rate = rate;
    }
    /// `set_bias`.
    pub fn set_bias(&mut self, bias: F) {
        self.bias = bias;
    }
    /// `set_jitter`.
    pub fn set_jitter(&mut self, jitter: F) {
        self.jitter = jitter;
    }
    /// `set_deja_vu`.
    pub fn set_deja_vu(&mut self, deja_vu: F) {
        self.sequence.set_deja_vu(deja_vu);
    }
    /// `set_length`.
    pub fn set_length(&mut self, length: i32) {
        self.sequence.set_length(length);
    }
    /// `set_pulse_width_mean`.
    pub fn set_pulse_width_mean(&mut self, v: F) {
        self.pulse_width_mean = v;
    }
    /// `set_pulse_width_std`.
    pub fn set_pulse_width_std(&mut self, v: F) {
        self.pulse_width_std = v;
    }

    /// `RandomPulseWidth(i, u)`.
    fn random_pulse_width(&self, u: F) -> F {
        if self.pulse_width_std == 0.0 {
            0.05 + 0.9 * self.pulse_width_mean
        } else {
            0.05 + 0.9 * beta_sample(u, self.pulse_width_std, self.pulse_width_mean)
        }
    }

    fn complementary_bernoulli(&self, x: &RandomVector) -> u32 {
        let mut bitmask = 0;
        for i in 0..NUM_T_CHANNELS {
            if (x.u(i >> 1) > self.bias) ^ (i & 1 != 0) {
                bitmask |= 1 << i;
            }
        }
        bitmask
    }

    fn independent_bernoulli(&self, x: &RandomVector) -> u32 {
        let mut bitmask = 0;
        for i in 0..NUM_T_CHANNELS {
            if (x.u(i) > self.bias) ^ (i & 1 != 0) {
                bitmask |= 1 << i;
            }
        }
        bitmask
    }

    fn three_states(&self, x: &RandomVector) -> u32 {
        let mut bitmask = 0;
        let p_none = 0.75 - (self.bias - 0.5).abs();
        let threshold = p_none + (1.0 - p_none) * (0.25 + (self.bias * 0.5));
        for i in 0..NUM_T_CHANNELS {
            let u = x.u(i >> 1);
            if u > p_none && ((u > threshold) ^ (i & 1 != 0)) {
                bitmask |= 1 << i;
            }
        }
        bitmask
    }

    fn drums(&mut self, x: &RandomVector) -> u32 {
        self.drum_pattern_step += 1;
        if self.drum_pattern_step >= DRUM_PATTERN_SIZE {
            self.drum_pattern_step = 0;
            let u = x.u(0) * 2.0 * (self.bias - 0.5).abs();
            // The oracle's `patches/all/0011`: the index stays on the last pattern at u = 1.0.
            let i = (F::from_usize(NUM_DRUM_PATTERNS) * u).trunc_i32();
            self.drum_pattern_index = index(i64::from(i), NUM_DRUM_PATTERNS);
            if self.bias <= 0.5 {
                self.drum_pattern_index -= self.drum_pattern_index % 2;
            }
        }
        u32::from(DRUM_PATTERNS[self.drum_pattern_index % NUM_DRUM_PATTERNS][self.drum_pattern_step % DRUM_PATTERN_SIZE])
    }

    fn markov(&mut self, x: &RandomVector) -> u32 {
        let mut bitmask = 0_i32;
        let b = 1.5 * self.bias - 0.5;
        let p = usize::try_from(self.markov_history_ptr).unwrap_or(0) % MARKOV_HISTORY_SIZE;
        self.markov_history[p] = 0;
        let h = |k: usize| self.markov_history[(p + k) % MARKOV_HISTORY_SIZE];
        for i in 0..NUM_T_CHANNELS {
            let mask = 1_i32 << i;
            // Repeat what played 8 ticks ago; avoid both channels at once; favor sparse patterns;
            // favor one channel echoing what the other played 4 ticks before.
            let periodic = h(8) & mask != 0;
            let simultaneous = h(8) & !mask != 0;
            let dense = h(1) & mask != 0;
            let alternate = h(4) & !mask != 0;
            let mut logit = F(-1.5);
            logit += if self.streak_counter[i] > 24 { 10.0 } else { 0.0 };
            logit += 8.0 * b.abs() * (if periodic { b } else { -b });
            logit -= 2.0 * (if simultaneous { b } else { -b });
            logit -= 1.0 * (if dense { b } else { F::ZERO });
            logit += 1.0 * (if alternate { b } else { F::ZERO });
            let logit = constrain(logit, -10.0, 10.0);
            let probability = F(LUT_LOGIT[index(i64::from((logit * 12.8 + 128.0).trunc_i32()), LUT_LOGIT.len())]);
            let mut state = x.u(i) < probability;
            if self.sequence.deja_vu() >= x.p() {
                let back = usize::try_from(self.sequence.length()).unwrap_or(0);
                state = h(back) & mask != 0;
            }
            if state {
                bitmask |= mask;
                self.streak_counter[i] = 0;
            } else {
                self.streak_counter[i] += 1;
            }
        }
        self.markov_history[p] |= bitmask;
        self.markov_history_ptr = ((p + MARKOV_HISTORY_SIZE - 1) % MARKOV_HISTORY_SIZE) as i32;
        bitmask as u32
    }

    fn schedule_output_pulses(&mut self, x: &RandomVector, bitmask: u32) {
        let mut bitmask = bitmask;
        for i in 0..NUM_T_CHANNELS {
            let pw = self.random_pulse_width(x.pulse_width(i));
            self.restarted[i] = self.slave_ramp[i].init_bernoulli(bitmask & 1 != 0, pw, 0.5);
            bitmask >>= 1;
        }
    }

    /// `ConfigureSlaveRamps(random_vector)`.
    pub(crate) fn configure_slave_ramps(&mut self, x: &RandomVector) {
        self.pattern_started = false;
        self.restarted = [false; NUM_T_CHANNELS];
        match self.model {
            Model::CoinToss => {
                let m = self.complementary_bernoulli(x);
                self.schedule_output_pulses(x, m);
            }
            Model::Independent => {
                let m = self.independent_bernoulli(x);
                self.schedule_output_pulses(x, m);
            }
            Model::ThreeStates => {
                let m = self.three_states(x);
                self.schedule_output_pulses(x, m);
            }
            Model::Drums => {
                let m = self.drums(x);
                self.schedule_output_pulses(x, m);
            }
            Model::Markov => {
                let m = self.markov(x);
                self.schedule_output_pulses(x, m);
            }
            Model::Clusters | Model::Divider => {
                self.divider_pattern_length -= 1;
                if self.divider_pattern_length <= 0 {
                    let mut pattern;
                    if self.model == Model::Divider {
                        pattern = self.bias_quantizer.lookup(&FIXED_DIVIDER_PATTERNS, self.bias);
                    } else {
                        let strength = (self.bias - 0.5).abs() * 2.0;
                        let mut u = x.u(0);
                        u *= u + strength * strength * (1.0 - u);
                        u *= strength;
                        // The oracle's `patches/all/0011`: the index stays on the last pattern at
                        // u = 1.0.
                        let i = (u * NUM_DIVIDER_PATTERNS as f32).trunc_usize().min(NUM_DIVIDER_PATTERNS - 1);
                        pattern = DIVIDER_PATTERNS[i];
                        if self.bias < 0.5 {
                            pattern.ratios.swap(0, NUM_T_CHANNELS - 1);
                        }
                    }
                    for i in 0..NUM_T_CHANNELS {
                        let pw = self.random_pulse_width(x.pulse_width(i));
                        self.slave_ramp[i].init_pattern(pattern.length, pattern.ratios[i], pw);
                    }
                    self.divider_pattern_length = pattern.length;
                    self.pattern = pattern;
                    self.pattern_started = true;
                }
            }
        }
    }

    /// The jitter's multiplier of the master clock's speed for the next tick, from the tick's
    /// random vector and how far the jittery clock has drifted from the straight one (`Process`,
    /// on a master tick).
    #[must_use]
    pub fn jitter_multiplier_for(&self, x: &RandomVector, phase_difference: F) -> F {
        let jitter_amount = self.jitter * self.jitter * self.jitter * self.jitter * 36.0;
        let v = fast_beta_sample(x.jitter());
        let mut multiplier = semitones_to_ratio((v * 2.0 - 1.0) * jitter_amount);
        // Keeps the jittery clock close to the straight one: the further apart, the likelier it
        // speeds up or slows down to catch up.
        multiplier *= if phase_difference > 0.0 { 1.0 + phase_difference } else { 1.0 / (1.0 - phase_difference) };
        multiplier
    }

    /// The reset branch of `Process`: the slave ramps, the sequence and the patterns restart, and
    /// (but for the divider model) a tick is configured at once.
    pub(crate) fn reset(&mut self, stream: &mut RandomStream) {
        for s in &mut self.slave_ramp {
            s.reset();
        }
        self.sequence.reset();
        self.divider_pattern_length = 0;
        self.drum_pattern_step = DRUM_PATTERN_SIZE;
        if self.model != Model::Divider {
            let mut v = RandomVector::default();
            self.sequence.next_vector(stream, &mut v.0);
            self.configure_slave_ramps(&v);
        }
    }

    /// `Process(use_external_clock, &reset, external_clock, ramps, gate, size)`: `gate` holds two
    /// gates per frame.
    pub fn process<const B: usize>(
        &mut self,
        stream: &mut RandomStream,
        use_external_clock: bool,
        reset: &mut bool,
        external_clock: &[u8],
        ramps: &mut Ramps<B>,
        gate: &mut [bool],
        size: usize,
    ) {
        let internal_frequency = if use_external_clock {
            if !self.use_external_clock {
                self.ramp_extractor.reset();
            }
            let mut ratio = self.rate_quantizer.lookup(&INPUT_DIVIDER_RATIOS, 1.05 * self.rate / 96.0 + 0.5);
            if self.range == Range::Quarter {
                ratio.q *= 4;
            } else if self.range == Range::Times4 {
                ratio.p *= 4;
            }
            ratio.simplify2();
            let n = size.min(B).min(external_clock.len());
            self.ramp_extractor.process(ratio, true, reset, &external_clock[..n], &mut ramps.external[..n]);
            F::ZERO
        } else {
            let rate = match self.range {
                Range::Times4 => 8.0,
                Range::Quarter => 0.5,
                Range::Normal => 2.0,
            };
            rate * self.one_hertz * semitones_to_ratio(self.rate)
        };
        self.use_external_clock = use_external_clock;

        if *reset {
            self.reset(stream);
        }

        for i in 0..size.min(B) {
            let mut frequency = if use_external_clock { ramps.external[i] - self.previous_external_ramp_value } else { internal_frequency };
            frequency += if frequency < 0.0 { 1.0 } else { 0.0 };
            let jittery_frequency = frequency * self.jitter_multiplier;
            self.master_phase += jittery_frequency;
            self.phase_difference += frequency - jittery_frequency;
            if self.master_phase > 1.0 {
                self.master_phase -= 1.0;
                let mut v = RandomVector::default();
                self.sequence.next_vector(stream, &mut v.0);
                self.jitter_multiplier = self.jitter_multiplier_for(&v, self.phase_difference);
                self.configure_slave_ramps(&v);
            }
            if internal_frequency != 0.0 {
                ramps.external[i] = self.master_phase;
            }
            self.previous_external_ramp_value = ramps.external[i];
            ramps.master[i] = self.master_phase;
            for j in 0..NUM_T_CHANNELS {
                let (phase, g) = self.slave_ramp[j].process(frequency * self.jitter_multiplier);
                ramps.slave[j][i] = phase;
                if let Some(slot) = gate.get_mut(2 * i + j) {
                    *slot = g;
                }
            }
        }
    }
}

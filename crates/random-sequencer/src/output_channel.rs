//! One random voltage channel: `marbles/random/output_channel.{h,cc}`. On each tick of its clock it
//! draws a value from its sequence, shapes it by the spread and bias (a beta distribution that
//! narrows to the bias at no spread and turns into a coin toss at full spread), scales it to its
//! voltage range, then quantizes it (steps above the center) or glides to it (below).

#![forbid(unsafe_code)]

use crate::distributions::beta_sample;
use crate::float::F;
use crate::lag::LagProcessor;
use crate::quantizer::{Quantizer, Scale};
use crate::random::{RandomSequence, RandomStream};
use crate::stmlib::{ParameterInterpolator, constrain, index};

/// `kNumReacquisitions`: how many blocks the input-sampling mode tracks its input after a tick.
const NUM_REACQUISITIONS: u32 = 20;

/// `ScaleOffset`: `x * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleOffset {
    pub scale: F,
    pub offset: F,
}

impl ScaleOffset {
    /// `ScaleOffset(s, o)`.
    #[must_use]
    pub fn new(scale: f32, offset: f32) -> Self {
        Self { scale: F(scale), offset: F(offset) }
    }

    fn apply(self, x: F) -> F {
        x * self.scale + self.offset
    }
}

/// `OutputChannel`, with `N` scale slots (the firmware has 6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputChannel<const N: usize> {
    spread: F,
    bias: F,
    steps: F,
    scale_index: usize,
    register_mode: bool,
    register_value: F,
    register_transposition: F,
    previous_steps: F,
    previous_phase: F,
    reacquisition_counter: u32,
    previous_voltage: F,
    voltage: F,
    quantized_voltage: F,
    scale_offset: ScaleOffset,
    lag_processor: LagProcessor,
    quantizer: [Quantizer; N],
}

impl<const N: usize> Default for OutputChannel<N> {
    fn default() -> Self {
        let mut c = Self {
            spread: F::ZERO,
            bias: F::ZERO,
            steps: F::ZERO,
            scale_index: 0,
            register_mode: false,
            register_value: F::ZERO,
            register_transposition: F::ZERO,
            previous_steps: F::ZERO,
            previous_phase: F::ZERO,
            reacquisition_counter: 0,
            previous_voltage: F::ZERO,
            voltage: F::ZERO,
            quantized_voltage: F::ZERO,
            scale_offset: ScaleOffset::new(1.0, 0.0),
            lag_processor: LagProcessor::default(),
            quantizer: [Quantizer::default(); N],
        };
        c.init();
        c
    }
}

impl<const N: usize> OutputChannel<N> {
    /// `Init()`.
    pub fn init(&mut self) {
        self.spread = F(0.5);
        self.bias = F(0.5);
        self.steps = F(0.5);
        self.scale_index = 0;
        self.register_mode = false;
        self.register_value = F::ZERO;
        self.register_transposition = F::ZERO;
        self.previous_steps = F::ZERO;
        self.previous_phase = F::ZERO;
        self.reacquisition_counter = 0;
        self.previous_voltage = F::ZERO;
        self.voltage = F::ZERO;
        self.quantized_voltage = F::ZERO;
        self.scale_offset = ScaleOffset::new(10.0, -5.0);
        self.lag_processor.init();
        let scale = Scale::default();
        for q in &mut self.quantizer {
            q.init(&scale);
        }
    }

    /// `LoadScale(i, scale)`.
    pub fn load_scale(&mut self, i: usize, scale: &Scale) {
        if let Some(q) = self.quantizer.get_mut(i) {
            q.init(scale);
        }
    }

    /// `set_spread`.
    pub fn set_spread(&mut self, v: F) {
        self.spread = v;
    }

    /// `set_bias`.
    pub fn set_bias(&mut self, v: F) {
        self.bias = v;
    }

    /// `set_steps`.
    pub fn set_steps(&mut self, v: F) {
        self.steps = v;
    }

    /// `set_scale_index`.
    pub fn set_scale_index(&mut self, i: usize) {
        self.scale_index = i;
    }

    /// `set_register_mode`.
    pub fn set_register_mode(&mut self, v: bool) {
        self.register_mode = v;
    }

    /// `set_register_value`.
    pub fn set_register_value(&mut self, v: F) {
        self.register_value = v;
    }

    /// `set_register_transposition`.
    pub fn set_register_transposition(&mut self, v: F) {
        self.register_transposition = v;
    }

    /// `set_scale_offset`.
    pub fn set_scale_offset(&mut self, v: ScaleOffset) {
        self.scale_offset = v;
    }

    /// `Quantize(voltage, amount)`.
    pub fn quantize(&mut self, voltage: F, amount: F) -> F {
        let i = index(self.scale_index as i64, N);
        self.quantizer[i].process(voltage, amount, false)
    }

    /// The voltage last drawn (before quantization).
    #[must_use]
    pub fn voltage(&self) -> F {
        self.voltage
    }

    /// The last drawn voltage, quantized.
    #[must_use]
    pub fn quantized(&self) -> F {
        self.quantized_voltage
    }

    /// The steps the channel quantizes at (as `set_steps` took it).
    #[must_use]
    pub fn steps(&self) -> F {
        self.steps
    }

    /// `GenerateNewVoltage(random_sequence)`.
    pub fn generate_new_voltage(&mut self, sequence: &mut RandomSequence, stream: &mut RandomStream) -> F {
        let u = sequence.next_value(stream, self.register_mode, self.register_value);
        if self.register_mode {
            10.0 * (u - 0.5) + self.register_transposition
        } else {
            let degenerate_amount = constrain(1.25 - self.spread * 25.0, 0.0, 1.0);
            let bernoulli_amount = constrain(self.spread * 25.0 - 23.75, 0.0, 1.0);
            let mut value = beta_sample(u, self.spread, self.bias);
            let bernoulli_value = if u >= 1.0 - self.bias { F(0.999_999) } else { F::ZERO };
            value += degenerate_amount * (self.bias - value);
            value += bernoulli_amount * (bernoulli_value - value);
            self.scale_offset.apply(value)
        }
    }

    /// Draws the next voltage now, as a tick of the channel's clock does: the new value (shaped and
    /// scaled), then its quantization at `steps` (0 to 1, as `set_steps` takes it). Returns
    /// `(voltage, quantized)`. The transport-locked sequencer's entry point; the firmware's loop is
    /// [`OutputChannel::process`].
    pub fn tick(&mut self, sequence: &mut RandomSequence, stream: &mut RandomStream) -> (F, F) {
        self.previous_voltage = self.voltage;
        self.voltage = self.generate_new_voltage(sequence, stream);
        self.lag_processor.reset_ramp();
        let steps = self.steps;
        self.quantized_voltage = self.quantize(self.voltage, 2.0 * steps - 1.0);
        (self.voltage, self.quantized_voltage)
    }

    /// `Process(random_sequence, phase, output, size, stride)`: one block; a new voltage whenever
    /// `phase` wraps. Writes `output[k * stride]` for each frame `k`.
    pub fn process(&mut self, sequence: &mut RandomSequence, stream: &mut RandomStream, phase: &[F], output: &mut [F], stride: usize) {
        let size = phase.len();
        let mut steps_modulation = ParameterInterpolator::new(self.previous_steps, self.steps, size);
        // The input-sampling mode keeps tracking its input for a few blocks after a tick, for
        // sequencers whose CV slews (the firmware's comment).
        if self.reacquisition_counter > 0 {
            self.reacquisition_counter -= 1;
            let u = sequence.rewrite_value(self.register_value);
            self.voltage = 10.0 * (u - 0.5) + self.register_transposition;
            let amount = 2.0 * self.steps - 1.0;
            self.quantized_voltage = self.quantize(self.voltage, amount);
        }
        for (k, &p) in phase.iter().enumerate() {
            let steps = steps_modulation.next();
            if p < self.previous_phase {
                self.previous_voltage = self.voltage;
                self.voltage = self.generate_new_voltage(sequence, stream);
                self.lag_processor.reset_ramp();
                self.quantized_voltage = self.quantize(self.voltage, 2.0 * steps - 1.0);
                if self.register_mode {
                    self.reacquisition_counter = NUM_REACQUISITIONS;
                }
            }
            let out = if steps >= 0.5 {
                self.quantized_voltage
            } else {
                let smoothness = 1.0 - 2.0 * steps;
                self.lag_processor.process(self.voltage, smoothness, p)
            };
            if let Some(o) = output.get_mut(k * stride) {
                *o = out;
            }
            self.previous_phase = p;
        }
        self.previous_steps = steps_modulation.value();
    }
}

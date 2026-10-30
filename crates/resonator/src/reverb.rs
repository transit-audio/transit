//! The reverb of the string-and-reverb model (`rings/dsp/fx/reverb.h` on `fx/fx_engine.h`): a
//! Griesinger topology (four input allpasses, then a loop of two allpasses and a delay per side) on
//! 16-bit delay memory, with two slow LFOs modulating the long delays.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::cosine_oscillator::CosineOscillator;
use crate::stmlib::{clip16, integral_fractional};

/// The delay memory, `FxEngine<32768, FORMAT_16_BIT>` on the firmware's 32768-word buffer.
const SIZE: usize = 32768;
const MASK: i32 = (SIZE - 1) as i32;

/// The lengths of the ten delay lines, in the order the firmware reserves them: ap1 to ap4, dap1a,
/// dap1b, del1, dap2a, dap2b, del2.
const LENGTHS: [i32; 10] = [150, 214, 319, 527, 2182, 2690, 4501, 2525, 2197, 6312];

/// Each line's base in the memory: the previous base plus the previous length plus one, as the
/// firmware's `DelayLine<Memory, index>::base`.
const BASES: [i32; 10] = {
    let mut bases = [0; 10];
    let mut i = 1;
    while i < 10 {
        bases[i] = bases[i - 1] + LENGTHS[i - 1] + 1;
        i += 1;
    }
    bases
};

const AP1: usize = 0;
const AP2: usize = 1;
const AP3: usize = 2;
const AP4: usize = 3;
const DAP1A: usize = 4;
const DAP1B: usize = 5;
const DEL1: usize = 6;
const DAP2A: usize = 7;
const DAP2B: usize = 8;
const DEL2: usize = 9;

/// `DataType<FORMAT_16_BIT>::Compress`: `Clip16(int32_t(value * 32768))` as 16 bits.
#[inline(always)]
fn compress(value: F) -> u16 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let w = clip16((value * 32768.0).trunc_i32()) as u16;
    w
}

/// `DataType<FORMAT_16_BIT>::Decompress`.
#[inline(always)]
fn decompress(value: u16) -> F {
    #[allow(clippy::cast_possible_wrap)]
    let s = value as i16;
    F(f32::from(s)) / 32768.0
}

/// The per-sample context of the firmware's `FxEngine::Context`.
struct Context<'a> {
    accumulator: F,
    previous_read: F,
    lfo_value: [F; 2],
    buffer: &'a mut [u16],
    write_ptr: i32,
}

impl Context<'_> {
    #[inline(always)]
    fn slot(&self, line: usize, offset: i32) -> usize {
        #[allow(clippy::cast_sign_loss)]
        let i = ((self.write_ptr + BASES[line] + offset) & MASK) as usize;
        i
    }

    /// `Read(value, scale)`.
    #[inline(always)]
    fn read_value(&mut self, value: F, scale: F) {
        self.accumulator += value * scale;
    }

    /// `Read(d TAIL, scale)`: the line's last sample.
    #[inline(always)]
    fn read_tail(&mut self, line: usize, scale: F) {
        let r = decompress(self.buffer[self.slot(line, LENGTHS[line] - 1)]);
        self.previous_read = r;
        self.accumulator += r * scale;
    }

    /// `Write(d, scale)` (offset 0).
    #[inline(always)]
    fn write_line(&mut self, line: usize, scale: F) {
        let i = self.slot(line, 0);
        self.buffer[i] = compress(self.accumulator);
        self.accumulator *= scale;
    }

    /// `WriteAllPass(d, scale)`.
    #[inline(always)]
    fn write_all_pass(&mut self, line: usize, scale: F) {
        self.write_line(line, scale);
        self.accumulator += self.previous_read;
    }

    /// `Interpolate(d, offset, lfo, amplitude, scale)`.
    #[inline(always)]
    fn interpolate(&mut self, line: usize, offset: F, lfo: usize, amplitude: F, scale: F) {
        let offset = offset + amplitude * self.lfo_value[lfo];
        let (integral, fractional) = integral_fractional(offset);
        let a = decompress(self.buffer[self.slot(line, integral)]);
        let b = decompress(self.buffer[self.slot(line, integral + 1)]);
        let x = a + (b - a) * fractional;
        self.previous_read = x;
        self.accumulator += x * scale;
    }

    /// `Lp(state, coefficient)`.
    #[inline(always)]
    fn lp(&mut self, state: &mut F, coefficient: F) {
        *state += coefficient * (self.accumulator - *state);
        self.accumulator = *state;
    }
}

/// `rings::Reverb`.
#[derive(Clone, Debug)]
pub struct Reverb {
    buffer: Box<[u16]>,
    write_ptr: i32,
    lfo: [CosineOscillator; 2],
    amount: F,
    input_gain: F,
    reverb_time: F,
    diffusion: F,
    lp: F,
    lp_decay_1: F,
    lp_decay_2: F,
}

impl Default for Reverb {
    fn default() -> Self {
        Self {
            buffer: vec![0; SIZE].into_boxed_slice(),
            write_ptr: 0,
            lfo: [CosineOscillator::default(); 2],
            amount: F::ZERO,
            input_gain: F::ZERO,
            reverb_time: F::ZERO,
            diffusion: F::ZERO,
            lp: F::ZERO,
            lp_decay_1: F::ZERO,
            lp_decay_2: F::ZERO,
        }
    }
}

impl Reverb {
    /// `Init(buffer)`: the memory cleared, the LFOs at 0.5 and 0.3 Hz. The amount, gain, time and
    /// low-pass states are left as they are (zero at power-on), as the firmware's `Init`.
    pub fn init(&mut self) {
        self.buffer.fill(0);
        self.write_ptr = 0;
        // `SetLFOFrequency(index, f)` runs the oscillator at 32 times f: it steps once every 32
        // samples.
        self.lfo[0].init_approximate(F(0.5) / 48000.0 * 32.0);
        self.lfo[1].init_approximate(F(0.3) / 48000.0 * 32.0);
        self.lp = F(0.7);
        self.diffusion = F(0.625);
    }

    /// `set_amount`.
    pub fn set_amount(&mut self, amount: F) {
        self.amount = amount;
    }

    /// `set_input_gain`.
    pub fn set_input_gain(&mut self, input_gain: F) {
        self.input_gain = input_gain;
    }

    /// `set_time`.
    pub fn set_time(&mut self, reverb_time: F) {
        self.reverb_time = reverb_time;
    }

    /// `set_diffusion`.
    pub fn set_diffusion(&mut self, diffusion: F) {
        self.diffusion = diffusion;
    }

    /// `set_lp`.
    pub fn set_lp(&mut self, lp: F) {
        self.lp = lp;
    }

    /// `Process(left, right, size)`, in place.
    pub fn process(&mut self, left: &mut [F], right: &mut [F]) {
        let kap = self.diffusion;
        let klp = self.lp;
        let krt = self.reverb_time;
        let amount = self.amount;
        let gain = self.input_gain;

        let mut lp_1 = self.lp_decay_1;
        let mut lp_2 = self.lp_decay_2;

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // `engine_.Start(&c)`.
            self.write_ptr -= 1;
            if self.write_ptr < 0 {
                self.write_ptr += SIZE as i32;
            }
            let lfo_value =
                if (self.write_ptr & 31) == 0 { [self.lfo[0].next(), self.lfo[1].next()] } else { [self.lfo[0].value(), self.lfo[1].value()] };
            let mut c = Context { accumulator: F::ZERO, previous_read: F::ZERO, lfo_value, buffer: &mut self.buffer, write_ptr: self.write_ptr };

            c.read_value(*l + *r, gain);

            // Diffuse through four allpasses.
            c.read_tail(AP1, kap);
            c.write_all_pass(AP1, -kap);
            c.read_tail(AP2, kap);
            c.write_all_pass(AP2, -kap);
            c.read_tail(AP3, kap);
            c.write_all_pass(AP3, -kap);
            c.read_tail(AP4, kap);
            c.write_all_pass(AP4, -kap);
            // `c.Write(apout)`.
            let apout = c.accumulator;

            // Main reverb loop. `c.Load(apout)`: the accumulator already holds it.
            c.interpolate(DEL2, F(6261.0), 1, F(50.0), krt);
            c.lp(&mut lp_1, klp);
            c.read_tail(DAP1A, -kap);
            c.write_all_pass(DAP1A, kap);
            c.read_tail(DAP1B, kap);
            c.write_all_pass(DAP1B, -kap);
            c.write_line(DEL1, F(2.0));
            // `c.Write(wet, 0.0f)`: the accumulator is cleared, and reloaded below.
            let wet = c.accumulator;

            *l += (wet - *l) * amount;

            c.accumulator = apout;
            c.interpolate(DEL1, F(4460.0), 0, F(40.0), krt);
            c.lp(&mut lp_2, klp);
            c.read_tail(DAP2A, kap);
            c.write_all_pass(DAP2A, -kap);
            c.read_tail(DAP2B, -kap);
            c.write_all_pass(DAP2B, kap);
            c.write_line(DEL2, F(2.0));
            let wet = c.accumulator;

            *r += (wet - *r) * amount;
        }

        self.lp_decay_1 = lp_1;
        self.lp_decay_2 = lp_2;
    }
}

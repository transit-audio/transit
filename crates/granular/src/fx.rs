//! The post-processing effects of the granular mode (`clouds/dsp/fx/diffuser.h`, `fx/reverb.h`),
//! each on its own `FxEngine` memory (`fx/fx_engine.h`): the diffuser's four allpasses per side on
//! float memory, and the reverb (a Griesinger topology: four input allpasses, then a loop of two
//! allpasses and a delay per side) on 12-bit memory with two slow LFOs.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::cosine_oscillator::CosineOscillator;
use crate::stmlib::{clip16, integral_fractional};

/// Each line's base in an engine's memory: the previous base plus the previous length plus one, as
/// the firmware's `DelayLine<Memory, index>::base`.
const fn bases<const N: usize>(lengths: [i32; N]) -> [i32; N] {
    let mut bases = [0; N];
    let mut i = 1;
    while i < N {
        bases[i] = bases[i - 1] + lengths[i - 1] + 1;
        i += 1;
    }
    bases
}

/// The diffuser's memory, `FxEngine<2048, FORMAT_32_BIT>`.
const DIFFUSER_SIZE: usize = 2048;
/// apl1 to apl4, then apr1 to apr4.
const DIFFUSER_LENGTHS: [i32; 8] = [126, 180, 269, 444, 151, 205, 245, 405];
const DIFFUSER_BASES: [i32; 8] = bases(DIFFUSER_LENGTHS);

/// `Diffuser`: four allpasses per side, crossfaded with the dry signal by the amount.
#[derive(Clone, Debug)]
pub(crate) struct Diffuser {
    buffer: Box<[F]>,
    write_ptr: i32,
    amount: F,
}

impl Default for Diffuser {
    fn default() -> Self {
        Self { buffer: vec![F::ZERO; DIFFUSER_SIZE].into_boxed_slice(), write_ptr: 0, amount: F::ZERO }
    }
}

impl Diffuser {
    /// `Init(buffer)`: the memory cleared. (The engine's LFOs are never set and never read here.)
    pub(crate) fn init(&mut self) {
        self.buffer.fill(F::ZERO);
        self.write_ptr = 0;
    }

    /// `set_amount`.
    pub(crate) fn set_amount(&mut self, amount: F) {
        self.amount = amount;
    }

    #[inline(always)]
    fn slot(&self, line: usize, offset: i32) -> usize {
        #[allow(clippy::cast_sign_loss)]
        let i = ((self.write_ptr + DIFFUSER_BASES[line] + offset) & (DIFFUSER_SIZE as i32 - 1)) as usize;
        i
    }

    /// One side: `Read(in)`, four `Read(ap TAIL, kap)` / `WriteAllPass(ap, -kap)` pairs, then
    /// `Write(wet, 0)`. Returns the wet sample and leaves the accumulator at `acc * 0`.
    #[inline(always)]
    fn side(&mut self, accumulator: &mut F, previous_read: &mut F, input: F, first_line: usize) -> F {
        const KAP: f32 = 0.625;
        *accumulator += input;
        for (line, &length) in DIFFUSER_LENGTHS.iter().enumerate().skip(first_line).take(4) {
            let r = self.buffer[self.slot(line, length - 1)];
            *previous_read = r;
            *accumulator += r * KAP;
            let w = self.slot(line, 0);
            self.buffer[w] = *accumulator;
            *accumulator *= -KAP;
            *accumulator += *previous_read;
        }
        let wet = *accumulator;
        *accumulator *= 0.0;
        wet
    }

    /// `Process(in_out, size)`.
    pub(crate) fn process(&mut self, in_out: &mut [[F; 2]]) {
        let amount = self.amount;
        for frame in in_out.iter_mut() {
            // `engine_.Start(&c)`.
            self.write_ptr -= 1;
            if self.write_ptr < 0 {
                self.write_ptr += DIFFUSER_SIZE as i32;
            }
            let (mut accumulator, mut previous_read) = (F::ZERO, F::ZERO);
            let wet = self.side(&mut accumulator, &mut previous_read, frame[0], 0);
            frame[0] += amount * (wet - frame[0]);
            let wet = self.side(&mut accumulator, &mut previous_read, frame[1], 4);
            frame[1] += amount * (wet - frame[1]);
        }
    }
}

/// The reverb's memory, `FxEngine<16384, FORMAT_12_BIT>`.
const REVERB_SIZE: usize = 16384;
const REVERB_MASK: i32 = REVERB_SIZE as i32 - 1;
/// ap1 to ap4, dap1a, dap1b, del1, dap2a, dap2b, del2.
const REVERB_LENGTHS: [i32; 10] = [113, 162, 241, 399, 1653, 2038, 3411, 1913, 1663, 4782];
const REVERB_BASES: [i32; 10] = bases(REVERB_LENGTHS);

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

/// `DataType<FORMAT_12_BIT>::Compress`: `Clip16(int32_t(value * 4096))` as 16 bits.
#[inline(always)]
fn compress(value: F) -> u16 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let w = clip16((value * 4096.0).trunc_i32()) as u16;
    w
}

/// `DataType<FORMAT_12_BIT>::Decompress`.
#[inline(always)]
fn decompress(value: u16) -> F {
    #[allow(clippy::cast_possible_wrap)]
    let s = value as i16;
    F(f32::from(s)) / 4096.0
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
        let i = ((self.write_ptr + REVERB_BASES[line] + offset) & REVERB_MASK) as usize;
        i
    }

    /// `Load(value)`.
    #[inline(always)]
    fn load(&mut self, value: F) {
        self.accumulator = value;
    }

    /// `Read(value, scale)`.
    #[inline(always)]
    fn read_value(&mut self, value: F, scale: F) {
        self.accumulator += value * scale;
    }

    /// `Read(d TAIL, scale)`: the line's last sample.
    #[inline(always)]
    fn read_tail(&mut self, line: usize, scale: F) {
        let r = decompress(self.buffer[self.slot(line, REVERB_LENGTHS[line] - 1)]);
        self.previous_read = r;
        self.accumulator += r * scale;
    }

    /// `Write(d, offset, scale)`.
    #[inline(always)]
    fn write_at(&mut self, line: usize, offset: i32, scale: F) {
        let i = self.slot(line, offset);
        self.buffer[i] = compress(self.accumulator);
        self.accumulator *= scale;
    }

    /// `WriteAllPass(d, scale)` (offset 0).
    #[inline(always)]
    fn write_all_pass(&mut self, line: usize, scale: F) {
        self.write_at(line, 0, scale);
        self.accumulator += self.previous_read;
    }

    /// `Write(value, scale)` into a local: the accumulator, then scaled.
    #[inline(always)]
    fn take(&mut self, scale: F) -> F {
        let value = self.accumulator;
        self.accumulator *= scale;
        value
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

/// `clouds::Reverb`.
#[derive(Clone, Debug)]
pub(crate) struct Reverb {
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
            buffer: vec![0; REVERB_SIZE].into_boxed_slice(),
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
    pub(crate) fn init(&mut self) {
        self.buffer.fill(0);
        self.write_ptr = 0;
        // `SetLFOFrequency(index, f)` runs the oscillator at 32 times f: it steps once every 32
        // samples.
        self.lfo[0].init_approximate(F(0.5) / 32000.0 * 32.0);
        self.lfo[1].init_approximate(F(0.3) / 32000.0 * 32.0);
        self.lp = F(0.7);
        self.diffusion = F(0.625);
    }

    /// `set_amount`.
    pub(crate) fn set_amount(&mut self, amount: F) {
        self.amount = amount;
    }

    /// `set_input_gain`.
    pub(crate) fn set_input_gain(&mut self, input_gain: F) {
        self.input_gain = input_gain;
    }

    /// `set_time`.
    pub(crate) fn set_time(&mut self, reverb_time: F) {
        self.reverb_time = reverb_time;
    }

    /// `set_diffusion`.
    pub(crate) fn set_diffusion(&mut self, diffusion: F) {
        self.diffusion = diffusion;
    }

    /// `set_lp`.
    pub(crate) fn set_lp(&mut self, lp: F) {
        self.lp = lp;
    }

    /// `Process(in_out, size)`.
    pub(crate) fn process(&mut self, in_out: &mut [[F; 2]]) {
        let kap = self.diffusion;
        let klp = self.lp;
        let krt = self.reverb_time;
        let amount = self.amount;
        let gain = self.input_gain;

        let mut lp_1 = self.lp_decay_1;
        let mut lp_2 = self.lp_decay_2;

        for frame in in_out.iter_mut() {
            // `engine_.Start(&c)`.
            self.write_ptr -= 1;
            if self.write_ptr < 0 {
                self.write_ptr += REVERB_SIZE as i32;
            }
            let lfo_value =
                if (self.write_ptr & 31) == 0 { [self.lfo[0].next(), self.lfo[1].next()] } else { [self.lfo[0].value(), self.lfo[1].value()] };
            let mut c = Context { accumulator: F::ZERO, previous_read: F::ZERO, lfo_value, buffer: &mut self.buffer, write_ptr: self.write_ptr };

            // Smear AP1 inside the loop.
            c.interpolate(AP1, F(10.0), 0, F(60.0), F(1.0));
            c.write_at(AP1, 100, F(0.0));

            c.read_value(frame[0] + frame[1], gain);

            // Diffuse through four allpasses.
            c.read_tail(AP1, kap);
            c.write_all_pass(AP1, -kap);
            c.read_tail(AP2, kap);
            c.write_all_pass(AP2, -kap);
            c.read_tail(AP3, kap);
            c.write_all_pass(AP3, -kap);
            c.read_tail(AP4, kap);
            c.write_all_pass(AP4, -kap);
            let apout = c.accumulator;

            // Main reverb loop.
            c.load(apout);
            c.interpolate(DEL2, F(4680.0), 1, F(100.0), krt);
            c.lp(&mut lp_1, klp);
            c.read_tail(DAP1A, -kap);
            c.write_all_pass(DAP1A, kap);
            c.read_tail(DAP1B, kap);
            c.write_all_pass(DAP1B, -kap);
            c.write_at(DEL1, 0, F(2.0));
            let wet = c.take(F(0.0));

            frame[0] += (wet - frame[0]) * amount;

            c.load(apout);
            c.read_tail(DEL1, krt);
            c.lp(&mut lp_2, klp);
            c.read_tail(DAP2A, kap);
            c.write_all_pass(DAP2A, -kap);
            c.read_tail(DAP2B, -kap);
            c.write_all_pass(DAP2B, kap);
            c.write_at(DEL2, 0, F(2.0));
            let wet = c.take(F(0.0));

            frame[1] += (wet - frame[1]) * amount;
        }

        self.lp_decay_1 = lp_1;
        self.lp_decay_2 = lp_2;
    }
}

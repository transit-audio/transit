//! The two effects on the firmware's FX engine (`elements/dsp/fx/{fx_engine,diffuser,reverb}.h`):
//! the blow path's diffuser (four allpasses on 32-bit float memory) and the output reverb (a
//! Griesinger loop with a smeared first allpass, on 16-bit memory, two slow LFOs).

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::cosine_oscillator::CosineOscillator;
use transit_resonator::stmlib::{clip16, integral_fractional};

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

/// The diffuser's memory, `FxEngine<1024, FORMAT_32_BIT>` on the voice's 1024-float buffer.
const DIFFUSER_SIZE: usize = 1024;
const DIFFUSER_MASK: i32 = (DIFFUSER_SIZE - 1) as i32;
const DIFFUSER_LENGTHS: [i32; 4] = [126, 180, 269, 444];
const DIFFUSER_BASES: [i32; 4] = bases(DIFFUSER_LENGTHS);

/// `elements::Diffuser`: four allpasses at 0.625, applied in place.
#[derive(Clone, Debug)]
pub(crate) struct Diffuser {
    buffer: Box<[F]>,
    write_ptr: i32,
}

impl Default for Diffuser {
    fn default() -> Self {
        Self { buffer: vec![F::ZERO; DIFFUSER_SIZE].into_boxed_slice(), write_ptr: 0 }
    }
}

impl Diffuser {
    /// This diffuser as [`Default`] builds it, moving its memory out without allocating.
    #[must_use]
    pub(crate) fn take_zeroed(&mut self) -> Self {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.fill(F::ZERO);
        Self { buffer, write_ptr: 0 }
    }

    /// `Init(buffer)`: the memory cleared. (The engine's LFOs are never set and never read here.)
    pub(crate) fn init(&mut self) {
        self.buffer.fill(F::ZERO);
        self.write_ptr = 0;
    }

    #[inline(always)]
    fn slot(&self, line: usize, offset: i32) -> usize {
        #[allow(clippy::cast_sign_loss)]
        let i = ((self.write_ptr + DIFFUSER_BASES[line] + offset) & DIFFUSER_MASK) as usize;
        i
    }

    /// `Process(in_out, size)`.
    pub(crate) fn process(&mut self, in_out: &mut [F]) {
        const KAP: f32 = 0.625;
        for x in in_out.iter_mut() {
            self.write_ptr -= 1;
            if self.write_ptr < 0 {
                self.write_ptr += DIFFUSER_SIZE as i32;
            }
            let mut acc = F::ZERO;
            acc += *x;
            for (line, length) in DIFFUSER_LENGTHS.iter().enumerate() {
                // `Read(ap TAIL, kap)`, then `WriteAllPass(ap, -kap)`.
                let r = self.buffer[self.slot(line, length - 1)];
                acc += r * KAP;
                let w = self.slot(line, 0);
                self.buffer[w] = acc;
                acc *= -KAP;
                acc += r;
            }
            *x = acc;
        }
    }
}

/// The reverb's memory, `FxEngine<32768, FORMAT_16_BIT>` on the firmware's 32768-word buffer.
const REVERB_SIZE: usize = 32768;
const REVERB_MASK: i32 = (REVERB_SIZE - 1) as i32;
/// ap1 to ap4, dap1a, dap1b, del1, dap2a, dap2b, del2.
const REVERB_LENGTHS: [i32; 10] = [150, 214, 319, 527, 2182, 2690, 4501, 2525, 2197, 6312];
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

/// The per-sample context of the firmware's `FxEngine::Context` on the reverb's memory.
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

    /// `WriteAllPass(d, scale)`.
    #[inline(always)]
    fn write_all_pass(&mut self, line: usize, scale: F) {
        self.write_at(line, 0, scale);
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

/// `elements::Reverb`.
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
    /// This reverb as [`Default`] builds it, moving its memory out without allocating.
    #[must_use]
    pub(crate) fn take_zeroed(&mut self) -> Self {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.fill(0);
        Self {
            buffer,
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

    pub(crate) fn set_amount(&mut self, amount: F) {
        self.amount = amount;
    }

    pub(crate) fn set_input_gain(&mut self, input_gain: F) {
        self.input_gain = input_gain;
    }

    pub(crate) fn set_time(&mut self, reverb_time: F) {
        self.reverb_time = reverb_time;
    }

    pub(crate) fn set_diffusion(&mut self, diffusion: F) {
        self.diffusion = diffusion;
    }

    pub(crate) fn set_lp(&mut self, lp: F) {
        self.lp = lp;
    }

    /// `Process(left, right, size)`, in place.
    pub(crate) fn process(&mut self, left: &mut [F], right: &mut [F]) {
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
                self.write_ptr += REVERB_SIZE as i32;
            }
            let lfo_value =
                if (self.write_ptr & 31) == 0 { [self.lfo[0].next(), self.lfo[1].next()] } else { [self.lfo[0].value(), self.lfo[1].value()] };
            let mut c = Context { accumulator: F::ZERO, previous_read: F::ZERO, lfo_value, buffer: &mut self.buffer, write_ptr: self.write_ptr };

            // Smear AP1 inside the loop.
            c.interpolate(AP1, F(10.0), 0, F(80.0), F(1.0));
            c.write_at(AP1, 100, F::ZERO);

            c.accumulator += (*l + *r) * gain;

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
            c.interpolate(DEL2, F(6211.0), 1, F(100.0), krt);
            c.lp(&mut lp_1, klp);
            c.read_tail(DAP1A, -kap);
            c.write_all_pass(DAP1A, kap);
            c.read_tail(DAP1B, kap);
            c.write_all_pass(DAP1B, -kap);
            c.write_at(DEL1, 0, F(2.0));
            // `c.Write(wet, 0.0f)`.
            let wet = c.accumulator;
            c.accumulator *= 0.0;

            *l += (wet - *l) * amount;

            c.accumulator = apout;
            c.read_tail(DEL1, krt);
            c.lp(&mut lp_2, klp);
            c.read_tail(DAP2A, kap);
            c.write_all_pass(DAP2A, -kap);
            c.read_tail(DAP2B, -kap);
            c.write_all_pass(DAP2B, kap);
            c.write_at(DEL2, 0, F(2.0));
            let wet = c.accumulator;

            *r += (wet - *r) * amount;
        }

        self.lp_decay_1 = lp_1;
        self.lp_decay_2 = lp_2;
    }
}

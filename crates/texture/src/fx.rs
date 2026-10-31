//! The effect chain's shared stages: `beads/dsp/limiter.h`, `beads/dsp/fx/reverb.h` on its
//! `fx_engine.h`, and `beads/dsp/fx/wow_and_flutter.h`.

#![forbid(unsafe_code)]

use crate::delay_line::Frame;
use crate::float::F;
use crate::sine::SineOscillator;
use crate::stmlib::cosine_oscillator::CosineOscillator;
use crate::stmlib::delay_line::DelayLine;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{clip16, integral_fractional, one_pole, slope};

/// `Limiter`: a peak follower that turns the gain down above a ceiling.
#[derive(Clone, Copy, Debug, Default)]
pub struct Limiter {
    peak: F,
    peak_lp: F,
    decay: F,
    max_peak: F,
}

impl Limiter {
    /// `Init(decay, max_peak)`.
    pub fn init(&mut self, decay: F, max_peak: F) {
        self.peak = F(0.5);
        self.peak_lp = F(0.5);
        self.decay = decay;
        self.max_peak = max_peak;
    }

    /// `Process(in_out, size)`.
    pub fn process(&mut self, frames: &mut [Frame]) {
        for f in frames {
            self.process_frame(f);
        }
    }

    /// `Process(in_out, 1)`.
    #[inline(always)]
    pub fn process_frame(&mut self, f: &mut Frame) {
        let peak = f[0].abs().max(f[1].abs()).min(F(40.0));
        slope(&mut self.peak, peak, F(0.1), self.decay);
        slope(&mut self.peak_lp, self.peak, F(0.5), self.decay);
        let gain = if self.peak_lp <= self.max_peak { F(1.0) } else { self.max_peak / self.peak_lp };
        f[0] *= gain;
        f[1] *= gain;
    }
}

/// The reverb's memory, words (`FxEngine<32768, FORMAT_16_BIT>`).
pub const REVERB_SIZE: usize = 32768;
const MASK: i32 = REVERB_SIZE as i32 - 1;

/// The reverb's delay lines (`E::Reserve<...>`): lengths, each line followed by one spare word.
const LENGTHS: [i32; 10] = [150, 214, 319, 527, 2182, 2690, 4501, 2525, 2197, 6312];

const fn bases() -> [i32; 10] {
    let mut out = [0; 10];
    let mut i = 1;
    while i < 10 {
        out[i] = out[i - 1] + LENGTHS[i - 1] + 1;
        i += 1;
    }
    out
}

const BASE: [i32; 10] = bases();
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

/// `FxEngine::Context`: the accumulator a reverb sample is built in.
struct Context<'a> {
    accumulator: F,
    previous_read: F,
    lfo: [F; 2],
    buffer: &'a mut [u16],
    write_ptr: i32,
}

impl Context<'_> {
    #[inline(always)]
    fn index(&self, line: usize, offset: i32) -> usize {
        let o = if offset == -1 { LENGTHS[line] - 1 } else { offset };
        usize::try_from((self.write_ptr + BASE[line] + o) & MASK).unwrap_or(0)
    }

    /// `DataType<FORMAT_16_BIT>::Decompress`.
    #[inline(always)]
    fn decompress(v: u16) -> F {
        F::from_i32(i32::from(v.cast_signed())) / 32768.0
    }

    #[inline(always)]
    fn read_input(&mut self, value: F, scale: F) {
        self.accumulator += value * scale;
    }

    /// `Write(d, offset, scale)`.
    #[inline(always)]
    fn write(&mut self, line: usize, offset: i32, scale: F) {
        let w = i16::try_from(clip16((self.accumulator * 32768.0).trunc_i32())).unwrap_or(0).cast_unsigned();
        let i = self.index(line, offset);
        self.buffer[i] = w;
        self.accumulator *= scale;
    }

    #[inline(always)]
    fn write_all_pass(&mut self, line: usize, scale: F) {
        self.write(line, 0, scale);
        self.accumulator += self.previous_read;
    }

    /// `Read(d, offset, scale)`.
    #[inline(always)]
    fn read(&mut self, line: usize, offset: i32, scale: F) {
        let r = Self::decompress(self.buffer[self.index(line, offset)]);
        self.previous_read = r;
        self.accumulator += r * scale;
    }

    #[inline(always)]
    fn lp(&mut self, state: &mut F, coefficient: F) {
        *state += coefficient * (self.accumulator - *state);
        self.accumulator = *state;
    }

    /// `Interpolate(d, offset, lfo, amplitude, scale)`.
    #[inline(always)]
    fn interpolate(&mut self, line: usize, offset: F, lfo: usize, amplitude: F, scale: F) {
        let (oi, of) = integral_fractional(offset + amplitude * self.lfo[lfo]);
        let at = |k: i32| usize::try_from((self.write_ptr + oi + BASE[line] + k) & MASK).unwrap_or(0);
        let a = Self::decompress(self.buffer[at(0)]);
        let b = Self::decompress(self.buffer[at(1)]);
        let x = a + (b - a) * of;
        self.previous_read = x;
        self.accumulator += x * scale;
    }
}

/// `Reverb`: the Griesinger topology (four input all-passes, then a loop of two all-passes and a
/// delay per side), on 16-bit memory.
#[derive(Clone, Debug)]
pub struct Reverb {
    buffer: Box<[u16]>,
    write_ptr: i32,
    lfo: [CosineOscillator; 2],
    mix: F,
    wet: F,
    dry: F,
    send: F,
    reverb_time: F,
    lp: F,
    lp_1: F,
    lp_2: F,
}

impl Default for Reverb {
    fn default() -> Self {
        Self {
            buffer: vec![0; REVERB_SIZE].into_boxed_slice(),
            write_ptr: 0,
            lfo: [CosineOscillator::default(); 2],
            mix: F::ZERO,
            wet: F::ZERO,
            dry: F::ZERO,
            send: F::ZERO,
            reverb_time: F::ZERO,
            lp: F::ZERO,
            lp_1: F::ZERO,
            lp_2: F::ZERO,
        }
    }
}

impl Reverb {
    /// `Init(buffer, sample_rate)`: the memory cleared, then [`Reverb::reset`].
    pub fn init(&mut self, sample_rate: F) {
        self.buffer.fill(0);
        self.write_ptr = 0;
        self.reset(sample_rate);
    }

    /// `Reset(sample_rate)`: the modulation and the mix, not the memory.
    pub fn reset(&mut self, sample_rate: F) {
        self.lfo[0].init_approximate(0.5 / sample_rate * 32.0);
        self.lfo[1].init_approximate(0.3 / sample_rate * 32.0);
        self.lp = F(0.7);
        self.lp_1 = F::ZERO;
        self.lp_2 = F::ZERO;
        self.mix = F::ZERO;
        self.dry = F::ZERO;
        self.wet = F::ZERO;
        self.send = F::ZERO;
    }

    /// `set_mix`.
    pub fn set_mix(&mut self, mix: F) {
        self.mix = mix;
    }

    /// `set_time`.
    pub fn set_time(&mut self, time: F) {
        self.reverb_time = time;
    }

    /// `set_lp`.
    pub fn set_lp(&mut self, lp: F) {
        self.lp = lp;
    }

    /// `Process(in_out, size)`.
    pub fn process(&mut self, frames: &mut [Frame]) {
        let size = frames.len();
        let kap = F(0.625);
        let klp = self.lp;
        let krt = self.reverb_time;
        let input_gain = 0.2 * (1.0 - 0.333 * self.reverb_time);
        let output_gain = 0.4 / input_gain;
        let send_amount_sqrt = self.mix * (2.0 - self.mix);
        let send_amount = send_amount_sqrt * (2.0 - send_amount_sqrt);
        let dry = (1.0 - self.mix) * (1.0 + self.mix);
        let mut wet_modulation = ParameterInterpolator::new(self.wet, output_gain, size);
        let mut dry_modulation = ParameterInterpolator::new(self.dry, dry, size);
        let mut send_modulation = ParameterInterpolator::new(self.send, send_amount * input_gain, size);
        for f in frames {
            let dry_amount = dry_modulation.next();
            let wet_amount = wet_modulation.next();
            let send_amount = send_modulation.next();
            // `FxEngine::Start`.
            self.write_ptr -= 1;
            if self.write_ptr < 0 {
                self.write_ptr += REVERB_SIZE as i32;
            }
            let lfo = if self.write_ptr & 31 == 0 { [self.lfo[0].next(), self.lfo[1].next()] } else { [self.lfo[0].value(), self.lfo[1].value()] };
            let mut c = Context { accumulator: F::ZERO, previous_read: F::ZERO, lfo, buffer: &mut self.buffer, write_ptr: self.write_ptr };
            // Smear AP1 inside the loop.
            c.interpolate(AP1, F(10.0), 0, F(80.0), F(1.0));
            c.write(AP1, 100, F::ZERO);
            c.read_input(f[0] + f[1], send_amount);
            for ap in [AP1, AP2, AP3, AP4] {
                c.read(ap, -1, kap);
                c.write_all_pass(ap, -kap);
            }
            // `Write(apout)` then `Load(apout)`: the accumulator already holds it.
            let apout = c.accumulator;
            c.interpolate(DEL2, F(6261.0), 1, F(50.0), krt);
            c.lp(&mut self.lp_1, klp);
            c.read(DAP1A, -1, -kap);
            c.write_all_pass(DAP1A, kap);
            c.read(DAP1B, -1, kap);
            c.write_all_pass(DAP1B, -kap);
            c.write(DEL1, 0, F(1.0));
            let wet = c.accumulator;
            f[0] = f[0] * dry_amount + wet * wet_amount;
            c.accumulator = apout;
            c.interpolate(DEL1, F(4460.0), 0, F(40.0), krt);
            c.lp(&mut self.lp_2, klp);
            c.read(DAP2A, -1, kap);
            c.write_all_pass(DAP2A, -kap);
            c.read(DAP2B, -1, -kap);
            c.write_all_pass(DAP2B, kap);
            c.write(DEL2, 0, F(1.0));
            let wet = c.accumulator;
            f[1] = f[1] * dry_amount + wet * wet_amount;
        }
        self.wet = wet_modulation.value();
        self.dry = dry_modulation.value();
        self.send = send_modulation.value();
    }
}

/// `WowAndFlutter`: tape speed wobble, a short modulated delay and a low-pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct WowAndFlutter {
    one_hertz: F,
    delay: F,
    amount: F,
    lp: F,
    random_frequency: F,
    lp_state: Frame,
    random_modulation: SineOscillator,
    wow: SineOscillator,
    flutter: SineOscillator,
    delay_line: [DelayLine<128>; 2],
}

impl WowAndFlutter {
    /// `Init(sample_rate)`.
    pub fn init(&mut self, sample_rate: F) {
        for d in &mut self.delay_line {
            d.init();
        }
        self.wow.init(F::ZERO);
        self.flutter.init(F::ZERO);
        self.random_modulation.init(F::ZERO);
        self.one_hertz = 1.0 / sample_rate;
        self.random_frequency = 0.125 * self.one_hertz;
        self.delay = F::ZERO;
        self.amount = F(1.0);
        self.lp = F(1.0);
        self.lp_state = [F::ZERO; 2];
    }

    /// `set_lp`.
    pub fn set_lp(&mut self, lp: F) {
        self.lp = lp;
    }

    /// `Process(in_out, size)`.
    pub fn process(&mut self, rng: &mut Random, frames: &mut [Frame]) {
        let size = F::from_usize(frames.len());
        let previous_phase = self.random_modulation.phase();
        let random_modulation = self.random_modulation.render_at(self.random_frequency * size);
        if self.random_modulation.phase() >= 0.25 && previous_phase < 0.25 {
            self.random_frequency = self.one_hertz * (0.05 + rng.get_float() * 0.2);
        }
        let fm = 0.5 + 1.5 * random_modulation * random_modulation;
        let amount = (1.0 - 0.4 * random_modulation) * self.amount;
        let wow_frequency = self.one_hertz * 0.23 * size;
        let wow = self.wow.render_at(wow_frequency * fm);
        let flutter_frequency = self.one_hertz * 1.99 * size;
        let flutter = self.flutter.render_at(flutter_frequency);
        let delay = 1.0 + (105.0 * wow + 17.0 * flutter) * amount;
        let mut delay_modulation = ParameterInterpolator::new(self.delay, delay, frames.len());
        for f in frames {
            let d = delay_modulation.next();
            let l = self.delay_line[0].write_read(f[0], d);
            one_pole(&mut self.lp_state[0], l, self.lp);
            let r = self.delay_line[1].write_read(f[1], d);
            one_pole(&mut self.lp_state[1], r, self.lp);
            *f = self.lp_state;
        }
        self.delay = delay_modulation.value();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reverb_lines_sit_where_the_firmware_reserves_them() {
        assert_eq!(BASE, [0, 151, 366, 686, 1214, 3397, 6088, 10590, 13116, 15314]);
        assert!(BASE[9] + LENGTHS[9] <= REVERB_SIZE as i32);
    }

    #[test]
    fn the_limiter_holds_a_loud_signal_near_its_ceiling() {
        let mut l = Limiter::default();
        l.init(F(1.0 / 192_000.0), F(0.9));
        let mut last = [F::ZERO; 2];
        for _ in 0..48_000 {
            let mut f = [F(4.0), F(-4.0)];
            l.process_frame(&mut f);
            last = f;
        }
        assert!((last[0].0 - 0.9).abs() < 0.01, "{last:?}");
    }
}

//! Effects: the delay-line FX engine and the effects built on it.

// Based on MIT-licensed code (c) 2014 by Emilie Gillet (emilie.o.gillet@gmail.com)
//
// The FX engine follows the firmware's `FxEngine` exactly: one ring buffer shared by every
// delay line of an effect, a write pointer that moves back by one sample per `start`, and lines
// laid out one after the other (`Reserve` and `DelayLine` in `fx/fx_engine.h`). A tap at offset
// k reads the sample written k samples ago; the TAIL tap reads at `length - 1`.

pub mod diffuser;
pub mod ensemble;
pub mod low_pass_gate;
pub mod overdrive;
pub mod sample_rate_reducer;

use crate::utils::clip_16;
use crate::utils::cosine_oscillator::{CosineOscillator, CosineOscillatorMode};

/// How samples are stored in the ring buffer (`Format` and `DataType` in `fx/fx_engine.h`).
pub trait Format {
    /// The stored type.
    type T: Copy + Default + std::fmt::Debug;

    /// A stored sample as a float.
    fn decompress(value: Self::T) -> f32;

    /// A float as a stored sample.
    fn compress(value: f32) -> Self::T;
}

/// 12-bit samples in 16-bit words (`FORMAT_12_BIT`).
#[derive(Debug, Default, Clone)]
pub struct Format12Bit;

/// 32-bit float samples (`FORMAT_32_BIT`).
#[derive(Debug, Default, Clone)]
pub struct Format32Bit;

impl Format for Format12Bit {
    type T = i16;

    #[inline]
    fn decompress(value: i16) -> f32 {
        f32::from(value) / 4096.0
    }

    #[inline]
    fn compress(value: f32) -> i16 {
        clip_16((value * 4096.0) as i32) as i16
    }
}

impl Format for Format32Bit {
    type T = f32;

    #[inline]
    fn decompress(value: f32) -> f32 {
        value
    }

    #[inline]
    fn compress(value: f32) -> f32 {
        value
    }
}

/// One delay line in an effect's buffer: where it starts and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    base: i32,
    length: i32,
}

impl Line {
    /// The TAIL tap: the oldest sample of the line.
    pub const TAIL: i32 = -1;
}

/// The lines of an effect, laid out back to back as the firmware's `Reserve` chain does (each
/// line's base is the previous base plus the previous length plus one).
pub const fn lines<const N: usize>(lengths: [i32; N]) -> [Line; N] {
    let mut out = [Line { base: 0, length: 0 }; N];
    let mut i = 0;
    let mut base = 0;
    while i < N {
        out[i] = Line { base, length: lengths[i] };
        base += lengths[i] + 1;
        i += 1;
    }
    out
}

/// Which of the engine's two LFOs a modulated tap follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lfo {
    One = 0,
    Two = 1,
}

/// The FX engine with its per-sample context (`FxEngine` and `FxEngine::Context`).
#[derive(Debug, Clone)]
pub struct FxEngine<const SIZE: usize, F: Format> {
    buffer: Box<[F::T]>,
    write_ptr: i32,
    lfo: [CosineOscillator; 2],

    // The context of the current sample.
    accumulator: f32,
    previous_read: f32,
    lfo_value: [f32; 2],
}

impl<const SIZE: usize, F: Format> Default for FxEngine<SIZE, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SIZE: usize, F: Format> FxEngine<SIZE, F> {
    const MASK: i32 = SIZE as i32 - 1;

    /// A cleared engine. Allocates the buffer; nothing else does.
    pub fn new() -> Self {
        assert!(SIZE.is_power_of_two(), "the FX buffer size is a power of two");
        Self {
            buffer: vec![F::T::default(); SIZE].into_boxed_slice(),
            write_ptr: 0,
            lfo: [CosineOscillator::default(), CosineOscillator::default()],
            accumulator: 0.0,
            previous_read: 0.0,
            lfo_value: [0.0; 2],
        }
    }

    /// Zeroes the buffer and the write pointer (`Clear`).
    pub fn clear(&mut self) {
        self.buffer.fill(F::T::default());
        self.write_ptr = 0;
    }

    /// `SetLFOFrequency`.
    pub fn set_lfo_frequency(&mut self, lfo: Lfo, frequency: f32) {
        self.lfo[lfo as usize].init(frequency * 32.0, CosineOscillatorMode::Approximate);
    }

    /// Starts a sample (`Start`).
    #[inline]
    pub fn start(&mut self) {
        self.write_ptr -= 1;
        if self.write_ptr < 0 {
            self.write_ptr += SIZE as i32;
        }
        self.accumulator = 0.0;
        self.previous_read = 0.0;
        if (self.write_ptr & 31) == 0 {
            self.lfo_value[0] = self.lfo[0].next();
            self.lfo_value[1] = self.lfo[1].next();
        } else {
            self.lfo_value[0] = self.lfo[0].value();
            self.lfo_value[1] = self.lfo[1].value();
        }
    }

    #[inline]
    fn index(&self, line: Line, offset: i32) -> usize {
        let offset = if offset == Line::TAIL { line.length - 1 } else { offset };
        ((self.write_ptr + line.base + offset) & Self::MASK) as usize
    }

    /// `Load`.
    #[inline]
    pub fn load(&mut self, value: f32) {
        self.accumulator = value;
    }

    /// `Read(value, scale)`.
    #[inline]
    pub fn read(&mut self, value: f32, scale: f32) {
        self.accumulator += value * scale;
    }

    /// `Read(value)`.
    #[inline]
    pub fn read_value(&mut self, value: f32) {
        self.accumulator += value;
    }

    /// `Write(value, scale)`.
    #[inline]
    pub fn write(&mut self, value: &mut f32, scale: f32) {
        *value = self.accumulator;
        self.accumulator *= scale;
    }

    /// `Write(line, offset, scale)`.
    #[inline]
    pub fn write_line(&mut self, line: Line, offset: i32, scale: f32) {
        let i = self.index(line, offset);
        self.buffer[i] = F::compress(self.accumulator);
        self.accumulator *= scale;
    }

    /// `WriteAllPass(line, offset, scale)`.
    #[inline]
    pub fn write_all_pass(&mut self, line: Line, offset: i32, scale: f32) {
        self.write_line(line, offset, scale);
        self.accumulator += self.previous_read;
    }

    /// `Read(line, offset, scale)`.
    #[inline]
    pub fn read_line(&mut self, line: Line, offset: i32, scale: f32) {
        let r = F::decompress(self.buffer[self.index(line, offset)]);
        self.previous_read = r;
        self.accumulator += r * scale;
    }

    /// `Lp(state, coefficient)`.
    #[inline]
    pub fn lp(&mut self, state: &mut f32, coefficient: f32) {
        *state += coefficient * (self.accumulator - *state);
        self.accumulator = *state;
    }

    /// `Hp(state, coefficient)`.
    #[inline]
    pub fn hp(&mut self, state: &mut f32, coefficient: f32) {
        *state += coefficient * (self.accumulator - *state);
        self.accumulator -= *state;
    }

    /// `Interpolate(line, offset, scale)`.
    #[inline]
    pub fn interpolate(&mut self, line: Line, offset: f32, scale: f32) {
        let offset_integral = offset as i32;
        let offset_fractional = offset - offset_integral as f32;
        let a = F::decompress(self.buffer[((self.write_ptr + offset_integral + line.base) & Self::MASK) as usize]);
        let b = F::decompress(self.buffer[((self.write_ptr + offset_integral + line.base + 1) & Self::MASK) as usize]);
        let x = a + (b - a) * offset_fractional;
        self.previous_read = x;
        self.accumulator += x * scale;
    }

    /// `Interpolate(line, offset, lfo, amplitude, scale)`.
    #[inline]
    pub fn interpolate_lfo(&mut self, line: Line, offset: f32, lfo: Lfo, amplitude: f32, scale: f32) {
        self.interpolate(line, offset + amplitude * self.lfo_value[lfo as usize], scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_laid_out_back_to_back() {
        let l = lines([511, 511]);
        assert_eq!(l[0], Line { base: 0, length: 511 });
        assert_eq!(l[1], Line { base: 512, length: 511 });
    }

    #[test]
    fn a_tap_at_offset_k_reads_the_sample_written_k_samples_ago() {
        let [line] = lines([16]);
        let mut e = FxEngine::<64, Format32Bit>::new();
        for i in 0..10 {
            e.start();
            e.load(i as f32);
            e.write_line(line, 0, 0.0);
        }
        e.start();
        e.read_line(line, 3, 1.0);
        let mut v = 0.0;
        e.write(&mut v, 0.0);
        assert_eq!(v, 7.0);
    }
}

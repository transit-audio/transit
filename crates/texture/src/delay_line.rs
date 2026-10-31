//! `beads/dsp/delay_line.h`: the recording buffer, at three resolutions, stereo or mono, with the
//! freeze tail and its crossfade on resume.
//!
//! The firmware gives every quality's delay lines the same bytes and clears them whenever the
//! quality or the mono setting changes; only the selected quality's lines are ever read or
//! written in between. The port keeps one [`DelayMemory`] per processor with one `i16` per stored
//! sample (the 16-bit value, the 12-bit value, or the 8-bit mu-law code), so every layout reads
//! exactly the values the firmware's packed items hold; where the firmware's items sit in its
//! banks changes nothing it reads.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::mu_law::{lin_to_mulaw, mulaw_to_lin};
use crate::stmlib::{clip16, smooth_step};

/// A stereo frame.
pub type Frame = [F; 2];

/// The buffer's resolution (`Resolution`), in the firmware's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// `RESOLUTION_8_BIT`: mu-law codes.
    Bits8,
    /// `RESOLUTION_12_BIT`: 16-bit values shifted right by 4.
    Bits12,
    /// `RESOLUTION_16_BIT`.
    Bits16,
}

impl Resolution {
    /// The firmware's index (the processor's per-resolution coefficient tables use it).
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Bits8 => 0,
            Self::Bits12 => 1,
            Self::Bits16 => 2,
        }
    }

    /// The stored value of silence (`zero_item.Store(0, zero)`): the code of 0.
    fn zero(self) -> i16 {
        match self {
            Self::Bits8 => i16::from(lin_to_mulaw(0)),
            Self::Bits12 | Self::Bits16 => 0,
        }
    }

    /// `Item::Store`'s quantizer for one 16-bit sample.
    #[inline(always)]
    fn encode(self, sample: i32) -> i16 {
        let sample = i16::try_from(clip16(sample)).unwrap_or(0);
        match self {
            Self::Bits8 => i16::from(lin_to_mulaw(sample)),
            Self::Bits12 => sample >> 4,
            Self::Bits16 => sample,
        }
    }

    /// `Item::value`'s decoder.
    #[inline(always)]
    fn decode(self, stored: i16) -> F {
        match self {
            Self::Bits8 => mulaw_to_lin(u8::try_from(stored).unwrap_or(0)),
            Self::Bits12 => F::from_i32(i32::from(stored)) / 2048.0,
            Self::Bits16 => F::from_i32(i32::from(stored)) / 32768.0,
        }
    }
}

/// The length of the freeze tail, frames (`freeze_tail_size`).
pub const FREEZE_TAIL: usize = 256;

/// The samples past the end every line keeps as a copy of its start (`interpolation_tail`), so
/// the Hermite reads never wrap mid-read.
const INTERPOLATION_TAIL: i32 = 4;

/// The memory every delay line of a processor shares: the stored samples and the freeze tail.
#[derive(Clone, Debug)]
pub struct DelayMemory {
    items: Box<[i16]>,
    freeze_tail: Box<[Frame; FREEZE_TAIL]>,
}

impl DelayMemory {
    /// Memory for `items` stored samples (the largest layout the processor will use).
    #[must_use]
    pub fn new(items: usize) -> Self {
        Self { items: vec![0; items].into_boxed_slice(), freeze_tail: Box::new([[F::ZERO; 2]; FREEZE_TAIL]) }
    }
}

/// One delay line's state (`DelayLine<resolution, max_delay, num_channels>`) over a
/// [`DelayMemory`]. Positions are counted backwards from the write pointer; the 22.10
/// fixed-point reads ([`DelayLine::read_hermite_fixed`]) need `max_delay < 2^21`.
#[derive(Clone, Copy, Debug)]
pub struct DelayLine {
    resolution: Resolution,
    stereo: bool,
    max_delay: i32,
    write_ptr: i32,
    tail_write_ptr: i32,
    tail_xfade_duration: i32,
    written_since_last_frozen: i32,
    duration: i32,
    frozen: bool,
}

impl DelayLine {
    /// A line of `max_delay` frames (`MAX_DELAY`), `stereo` or mono. Call [`DelayLine::reset`]
    /// before use.
    #[must_use]
    pub fn new(resolution: Resolution, stereo: bool, max_delay: i32) -> Self {
        Self {
            resolution,
            stereo,
            max_delay,
            write_ptr: 0,
            tail_write_ptr: 0,
            tail_xfade_duration: 0,
            written_since_last_frozen: 0,
            duration: max_delay,
            frozen: false,
        }
    }

    /// The stored samples this line needs: its frames and the interpolation tail, per channel.
    #[must_use]
    pub fn items_needed(&self) -> usize {
        let frames = usize::try_from(self.max_delay + INTERPOLATION_TAIL).unwrap_or(0);
        if self.stereo { 2 * frames } else { frames }
    }

    /// `MAX_DELAY`.
    #[must_use]
    pub fn max_delay(&self) -> i32 {
        self.max_delay
    }

    /// `Reset(true)`: the state, and the whole memory cleared to this resolution's silence (the
    /// freeze tail to zeros).
    pub fn reset(&mut self, memory: &mut DelayMemory) {
        self.write_ptr = 0;
        self.tail_write_ptr = 0;
        self.tail_xfade_duration = 0;
        self.frozen = false;
        self.written_since_last_frozen = 0;
        self.duration = self.max_delay;
        memory.items.fill(self.resolution.zero());
        memory.freeze_tail.fill([F::ZERO; 2]);
    }

    /// `Trim()`: the recording's length drops to zero.
    pub fn trim(&mut self) {
        self.tail_write_ptr = 0;
        self.duration = 0;
    }

    /// `frozen()`.
    #[must_use]
    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// `duration()`: how much has been recorded since the last trim, up to `MAX_DELAY`.
    #[must_use]
    pub fn duration(&self) -> i32 {
        self.duration
    }

    /// `remaining()`: what is left before a frozen-free recording overwrites itself.
    #[must_use]
    pub fn remaining(&self) -> i32 {
        self.max_delay - self.written_since_last_frozen
    }

    #[inline(always)]
    fn store(&self, memory: &mut DelayMemory, i: i32, s: Frame) {
        let i = usize::try_from(i).unwrap_or(0);
        let r = self.resolution;
        if self.stereo {
            memory.items[2 * i] = r.encode((s[0] * 32768.0).trunc_i32());
            memory.items[2 * i + 1] = r.encode((s[1] * 32768.0).trunc_i32());
        } else {
            memory.items[i] = r.encode(((s[0] + s[1]) * 16384.0).trunc_i32());
        }
    }

    #[inline(always)]
    fn load(&self, memory: &DelayMemory, i: i32) -> Frame {
        let i = usize::try_from(i).unwrap_or(0);
        let r = self.resolution;
        if self.stereo {
            [r.decode(memory.items[2 * i]), r.decode(memory.items[2 * i + 1])]
        } else {
            let v = r.decode(memory.items[i]);
            [v, v]
        }
    }

    /// `Write(in)`: one frame at the write pointer, which then moves back one.
    #[inline(always)]
    fn write_frame(&mut self, memory: &mut DelayMemory, s: Frame) {
        self.store(memory, self.write_ptr, s);
        if self.write_ptr < INTERPOLATION_TAIL {
            self.store(memory, self.write_ptr + self.max_delay, s);
        }
        if self.write_ptr == 0 {
            self.write_ptr = self.max_delay - 1;
        } else {
            self.write_ptr -= 1;
        }
        self.written_since_last_frozen += 1;
        if self.written_since_last_frozen >= self.max_delay {
            self.written_since_last_frozen = 0;
        }
        self.duration += 1;
        if self.duration >= self.max_delay {
            self.duration = self.max_delay;
        }
    }

    /// `Write(in, frozen)`: while frozen, the first 256 frames go to the freeze tail and nothing is
    /// recorded; on resume, the input crossfades from the tail back to itself over as many frames
    /// as the tail holds.
    #[inline(always)]
    pub fn write(&mut self, memory: &mut DelayMemory, mut s: Frame, frozen: bool) {
        if frozen {
            if !self.frozen {
                self.tail_write_ptr = 0;
            }
            if self.tail_write_ptr < FREEZE_TAIL as i32 {
                memory.freeze_tail[usize::try_from(self.tail_write_ptr).unwrap_or(0)] = s;
                self.tail_write_ptr += 1;
            }
            self.written_since_last_frozen = 0;
        } else {
            if self.frozen && self.tail_write_ptr != 0 {
                self.tail_xfade_duration = self.tail_write_ptr;
            }
            if self.tail_write_ptr != 0 {
                let position = usize::try_from(self.tail_xfade_duration - self.tail_write_ptr).unwrap_or(0);
                let t = F::from_i32(self.tail_write_ptr) / F::from_i32(self.tail_xfade_duration);
                let gain = smooth_step(t);
                let tail = memory.freeze_tail[position.min(FREEZE_TAIL - 1)];
                s[0] += (tail[0] - s[0]) * gain;
                s[1] += (tail[1] - s[1]) * gain;
                self.tail_write_ptr -= 1;
            }
            self.write_frame(memory, s);
        }
        self.frozen = frozen;
    }

    /// `Read<INTERPOLATION_HERMITE>(delay_integral, delay_fractional)`: four-point Hermite
    /// interpolation `delay` frames back (the firmware adds `MAX_DELAY - 1` to fix the
    /// interpolator's latency).
    #[inline(always)]
    #[must_use]
    pub fn read_hermite(&self, memory: &DelayMemory, delay_integral: i32, t: F) -> Frame {
        let delay_integral = delay_integral + self.max_delay - 1;
        // The firmware's callers keep the sum non-negative; `rem_euclid` equals its `%` there.
        let i = (self.write_ptr + delay_integral).rem_euclid(self.max_delay);
        let x = [self.load(memory, i), self.load(memory, i + 1), self.load(memory, i + 2), self.load(memory, i + 3)];
        let mut out = [F::ZERO; 2];
        for c in 0..2 {
            let cc = (x[2][c] - x[0][c]) * 0.5;
            let v = x[1][c] - x[2][c];
            let w = cc + v;
            let a = w + v + (x[3][c] - x[1][c]) * 0.5;
            let b_neg = w + a;
            out[c] = (((a * t) - b_neg) * t + cc) * t + x[1][c];
        }
        out
    }

    /// `Read<INTERPOLATION_HERMITE>(int32_t delay)`: `delay` in 22.10 fixed point.
    #[inline(always)]
    #[must_use]
    pub fn read_hermite_fixed(&self, memory: &DelayMemory, delay: i32) -> Frame {
        self.read_hermite(memory, delay >> 10, F::from_i32(delay & 1023) / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_frame_reads_back_one_frame_later() {
        for (resolution, stereo) in [(Resolution::Bits16, true), (Resolution::Bits12, false), (Resolution::Bits8, true)] {
            let mut line = DelayLine::new(resolution, stereo, 64);
            let mut memory = DelayMemory::new(line.items_needed());
            line.reset(&mut memory);
            line.write(&mut memory, [F(0.5), F(-0.25)], false);
            for _ in 0..3 {
                line.write(&mut memory, [F::ZERO; 2], false);
            }
            // Hermite at a delay of n and fraction 0 reads x[1]: the frame of the n-th last write
            // (here from the interpolation tail's copy of position 0).
            let back = line.read_hermite(&memory, 4, F::ZERO);
            let expected = if stereo { [0.5, -0.25] } else { [0.125, 0.125] };
            for c in 0..2 {
                assert!((back[c].0 - expected[c]).abs() < 0.04, "{resolution:?} {stereo}: {back:?}");
            }
        }
    }

    #[test]
    fn the_8_bit_silence_is_the_code_of_zero() {
        let mut line = DelayLine::new(Resolution::Bits8, false, 16);
        let mut memory = DelayMemory::new(line.items_needed());
        line.reset(&mut memory);
        assert_eq!(line.read_hermite(&memory, 3, F(0.5)), [F::ZERO; 2]);
    }
}

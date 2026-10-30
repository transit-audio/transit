//! The recording buffer (`clouds/dsp/audio_buffer.h`) at the two resolutions the processor uses,
//! 16-bit linear and 8-bit mu-law (`clouds/dsp/mu_law.h`), with its interpolated reads and the
//! crossfade that hides the seam when recording resumes after a freeze.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::LUT_ULAW;
use crate::stmlib::clip16;

/// `kCrossFadeSize`: samples recorded during a freeze, crossfaded in when recording resumes.
const CROSSFADE_SIZE: i32 = 256;

/// `kInterpolationTail`: samples past the end that mirror the start, for the interpolators.
const INTERPOLATION_TAIL: i32 = 8;

/// The resolutions the granular processor uses (`RESOLUTION_16_BIT`, `RESOLUTION_8_BIT_MU_LAW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// 16-bit linear samples.
    Linear16,
    /// 8-bit mu-law samples.
    MuLaw8,
}

/// The read interpolation a grain uses (`InterpolationMethod(quality)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interpolation {
    /// `INTERPOLATION_ZOH`.
    Zoh,
    /// `INTERPOLATION_LINEAR`.
    Linear,
    /// `INTERPOLATION_HERMITE`.
    Hermite,
}

/// `Lin2MuLaw(pcm_val)`.
fn lin_to_mu_law(pcm: i16) -> u8 {
    let mut pcm = pcm >> 2;
    let mask: u8 = if pcm < 0 {
        pcm = -pcm;
        0x7f
    } else {
        0xff
    };
    if pcm > 8159 {
        pcm = 8159;
    }
    pcm += 0x84 >> 2;
    let seg: i16 = match pcm {
        i16::MIN..=0x3f => 0,
        0x40..=0x7f => 1,
        0x80..=0xff => 2,
        0x100..=0x1ff => 3,
        0x200..=0x3ff => 4,
        0x400..=0x7ff => 5,
        0x800..=0xfff => 6,
        0x1000..=0x1fff => 7,
        _ => 8,
    };
    if seg >= 8 {
        0x7f ^ mask
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let uval = ((seg << 4) | ((pcm >> (seg + 1)) & 0x0f)) as u8;
        uval ^ mask
    }
}

/// `AudioBuffer<resolution>` on its own memory. The C++ keeps one buffer object per resolution
/// over the same memory and re-initializes the one in use when the quality changes; here one
/// object switches resolution in [`AudioBuffer::init`], which resets every field the C++'s `Init`
/// resets. The 256-sample crossfade tail is the processor's `tail_buffer_[i]` in the C++, shared by
/// both resolutions and never cleared by `Init`; here it lives in the buffer and survives `init` too.
#[derive(Clone, Debug)]
pub(crate) struct AudioBuffer {
    memory: Box<[u8]>,
    resolution: Resolution,
    size: i32,
    write_head: i32,
    crossfade_counter: i32,
    tail: [i16; CROSSFADE_SIZE as usize],
}

impl AudioBuffer {
    /// A buffer owning `bytes` bytes of zeroed memory (allocated here, once).
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            memory: vec![0; bytes].into_boxed_slice(),
            resolution: Resolution::Linear16,
            size: 0,
            write_head: 0,
            crossfade_counter: 0,
            tail: [0; CROSSFADE_SIZE as usize],
        }
    }

    /// `Init(buffer, size, tail)` over the first `bytes` bytes of the memory: `bytes / 2` 16-bit
    /// samples or `bytes` mu-law samples, cleared (mu-law silence is 127), less the interpolation
    /// tail.
    pub(crate) fn init(&mut self, resolution: Resolution, bytes: usize) {
        self.resolution = resolution;
        let elements = match resolution {
            Resolution::Linear16 => bytes >> 1,
            Resolution::MuLaw8 => bytes,
        };
        self.size = i32::try_from(elements).unwrap_or(i32::MAX) - INTERPOLATION_TAIL;
        self.write_head = 0;
        self.crossfade_counter = 0;
        match resolution {
            Resolution::Linear16 => self.memory[..elements * 2].fill(0),
            Resolution::MuLaw8 => self.memory[..elements].fill(127),
        }
    }

    /// `size()`: samples, less the interpolation tail.
    pub(crate) fn size(&self) -> i32 {
        self.size
    }

    /// `head()`: the write position.
    pub(crate) fn head(&self) -> i32 {
        self.write_head
    }

    #[inline(always)]
    fn put16(&mut self, index: i32, value: i16) {
        let i = usize::try_from(index).unwrap_or(0) * 2;
        self.memory[i..i + 2].copy_from_slice(&value.to_le_bytes());
    }

    #[inline(always)]
    fn get16(&self, index: i32) -> i16 {
        let i = usize::try_from(index).unwrap_or(0) * 2;
        i16::from_le_bytes([self.memory[i], self.memory[i + 1]])
    }

    /// The sample at `index` as the C++ reads it before scaling: the int16, or the mu-law byte
    /// decoded through `lut_ulaw`.
    #[inline(always)]
    fn sample(&self, index: i32) -> F {
        match self.resolution {
            Resolution::Linear16 => F(f32::from(self.get16(index))),
            Resolution::MuLaw8 => F(f32::from(LUT_ULAW[usize::from(self.memory[usize::try_from(index).unwrap_or(0)])])),
        }
    }

    /// `Write(float in)`: one sample at the write head, mirrored into the interpolation tail.
    fn write(&mut self, input: F) {
        let quantized = clip16((input * 32768.0).trunc_i32());
        #[allow(clippy::cast_possible_truncation)]
        let quantized = quantized as i16;
        let head = self.write_head;
        match self.resolution {
            Resolution::Linear16 => {
                self.put16(head, quantized);
                if head < INTERPOLATION_TAIL {
                    self.put16(head + self.size, quantized);
                }
            }
            Resolution::MuLaw8 => {
                let byte = lin_to_mu_law(quantized);
                let h = usize::try_from(head).unwrap_or(0);
                self.memory[h] = byte;
                if head < INTERPOLATION_TAIL {
                    self.memory[h + usize::try_from(self.size).unwrap_or(0)] = byte;
                }
            }
        }
        self.write_head += 1;
        if self.write_head >= self.size {
            self.write_head = 0;
        }
    }

    /// `WriteFade(in, size, 2, write)` on one channel of interleaved frames: record (crossfading
    /// from the tail after a freeze), or, while frozen, keep the first 256 incoming samples in the
    /// tail for the crossfade.
    pub(crate) fn write_fade(&mut self, frames: &[[F; 2]], channel: usize, write: bool) {
        let n = i32::try_from(frames.len()).unwrap_or(0);
        if !write {
            for frame in frames {
                if self.crossfade_counter >= CROSSFADE_SIZE {
                    break;
                }
                let quantized = clip16((frame[channel] * 32767.0).trunc_i32());
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    self.tail[self.crossfade_counter as usize] = quantized as i16;
                }
                self.crossfade_counter += 1;
            }
        } else if self.crossfade_counter == 0
            && self.resolution == Resolution::Linear16
            && self.write_head >= INTERPOLATION_TAIL
            && self.write_head < self.size - n
        {
            // The C++'s fast path for the most common case: scaled by 32767 (the slow path by
            // 32768), no tail mirroring and no wrap (the head is clear of both ends).
            for frame in frames {
                let quantized = clip16((frame[channel] * 32767.0).trunc_i32());
                #[allow(clippy::cast_possible_truncation)]
                self.put16(self.write_head, quantized as i16);
                self.write_head += 1;
            }
        } else {
            for frame in frames {
                let mut sample = frame[channel];
                if self.crossfade_counter > 0 {
                    self.crossfade_counter -= 1;
                    // `tail_[kCrossFadeSize - crossfade_counter_]`: index 1 to 256, and 256 reads one
                    // past the tail in the C++, multiplied by a zero gain; the last entry gives the
                    // same result (any finite value does).
                    #[allow(clippy::cast_sign_loss)]
                    let index = ((CROSSFADE_SIZE - self.crossfade_counter) as usize).min(self.tail.len() - 1);
                    let tail_sample = F(f32::from(self.tail[index]));
                    let gain = F::from_i32(self.crossfade_counter) * (1.0 / 256.0);
                    sample += (tail_sample / 32768.0 - sample) * gain;
                }
                self.write(sample);
            }
        }
    }

    /// `Read<method>(integral, fractional)`.
    #[inline(always)]
    pub(crate) fn read(&self, interpolation: Interpolation, integral: i32, fractional: u16) -> F {
        let integral = if integral >= self.size { integral - self.size } else { integral };
        let scale = 1.0 / 32768.0;
        match interpolation {
            Interpolation::Zoh => self.sample(integral) * scale,
            Interpolation::Linear => {
                let t = F(f32::from(fractional)) / 65536.0;
                let x0 = self.sample(integral);
                let x1 = self.sample(integral + 1);
                (x0 + (x1 - x0) * t) * scale
            }
            Interpolation::Hermite => {
                let t = F(f32::from(fractional)) / 65536.0;
                let xm1 = self.sample(integral);
                let x0 = self.sample(integral + 1);
                let x1 = self.sample(integral + 2);
                let x2 = self.sample(integral + 3);
                // Laurent de Soras's Hermite interpolator, in the C++'s order of operations.
                let c = (x1 - xm1) * 0.5;
                let v = x0 - x1;
                let w = c + v;
                let a = w + v + (x2 - x0) * 0.5;
                let b_neg = w + a;
                ((((a * t) - b_neg) * t + c) * t + x0) * scale
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mu_law_round_trips_through_the_decoding_table() {
        // The encoder and `lut_ulaw` are inverse to within the mu-law step.
        for pcm in [-32768_i16, -20000, -1000, -4, 0, 4, 1000, 20000, 32767] {
            let decoded = i32::from(LUT_ULAW[usize::from(lin_to_mu_law(pcm))]);
            let error = (decoded - i32::from(pcm)).abs();
            assert!(error <= i32::from(pcm).abs() / 16 + 132, "{pcm}: {decoded}");
        }
        assert_eq!(LUT_ULAW[127], 0, "the cleared mu-law buffer is silent");
    }

    #[test]
    fn writes_wrap_and_mirror_the_interpolation_tail() {
        let mut b = AudioBuffer::new(64);
        b.init(Resolution::Linear16, 64);
        assert_eq!(b.size(), 24);
        let frames: Vec<[F; 2]> = (0..30).map(|i| [F(i as f32 / 64.0), F(0.0)]).collect();
        b.write_fade(&frames, 0, true);
        assert_eq!(b.head(), 6);
        // Sample 24 + 3 mirrors sample 3, rewritten by the wrap (value 27/64).
        assert_eq!(b.get16(3), b.get16(27));
        assert_eq!(b.get16(3), 27 * 512);
    }
}

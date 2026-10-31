//! transit's texture processor: ported to Rust from Émilie Gillet's MIT firmware (the eurorack
//! repository's `beads/dsp` at `f4cfa769`, with the stmlib helpers it uses), nulled bit for bit
//! against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #241).
//!
//! - [`Texture`] is the firmware's effect chain: it records its stereo input (with feedback) into
//!   a buffer at one of four qualities ([`Media`]), stereo or mono, and plays it back as up to 32
//!   overlapping grains (TIME, SIZE, SHAPE, PITCH, DENSITY, each with internal randomness or a
//!   CV), or, with SIZE at its top, as an eight-tap delay that freezes into slices; then a dry/wet
//!   crossfade, tape wow and flutter (dirty tape), a limiter and a reverb. The SEED button and
//!   input seed grains or clock the delay; FREEZE stops the recording.
//! - [`Synth`] is what the module plays with nothing patched in: a wavetable oscillator and grains
//!   of wavetables, then the limiters and the reverb.
//! - [`InputStage`] is the module's input stage: raw 32-bit codec frames to floats, with its
//!   automatic gain and the tape qualities' soft clipping.
//!
//! Each runs at its firmware rate only: [`Media::sample_rate`] (the module's codec runs at 48, 32
//! or 24 kHz depending on the quality), 48 kHz for the synth. Host-rate handling is the device's
//! (#241 slice 2). Blocks are up to [`MAX_BLOCK_SIZE`] frames, float stereo in and out. The buffer
//! is sized at construction ([`BufferSize`]). See [`Texture::process`] for the render contract.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once built, one random generator per
//! instance, and the module's flush-to-zero (its firmware sets FPSCR.FZ; the oracle sets the x86
//! equivalent) reproduced in its own arithmetic ([`float::F`]), never by changing the host's
//! floating-point mode.

#![forbid(unsafe_code)]

mod delay_controller;
mod delay_grain;
mod delay_line;
pub mod float;
mod fx;
mod grain;
mod granular_processor;
mod input;
mod mu_law;
mod processor;
mod randomized;
mod resources;
mod sine;
pub mod stmlib;
mod synth;
mod trigger;
mod wavetables;
mod wavetables_a;
mod wavetables_b;

pub use input::{InputStage, SoftClipping};
pub use processor::Texture;
pub use randomized::Parameter;
pub use synth::{SYNTH_SAMPLE_RATE, Synth, SynthResolution};

/// The largest block, frames: the module's codec renders its rate / 4000 frames per block (12, 8
/// or 6) and at most 24 (`kMaxCodecBlockSize`).
pub const MAX_BLOCK_SIZE: usize = 24;

/// A buffer quality (`Processor::Media`), in the firmware's order. Each runs the codec at its own
/// rate ([`Media::sample_rate`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Media {
    /// 8-bit mu-law at 24 kHz, with tape wow and flutter and soft clipping into the buffer.
    DirtyTape,
    /// 12-bit at 24 kHz behind the firmware's 2:1 converters (the codec runs at 48 kHz).
    CleanTape,
    /// 12-bit at 32 kHz.
    VintageDigital,
    /// 16-bit at 48 kHz (the module's default).
    #[default]
    Digital,
}

impl Media {
    /// Every quality, in the firmware's order.
    pub const ALL: [Self; 4] = [Self::DirtyTape, Self::CleanTape, Self::VintageDigital, Self::Digital];

    /// The firmware's index.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::DirtyTape => 0,
            Self::CleanTape => 1,
            Self::VintageDigital => 2,
            Self::Digital => 3,
        }
    }

    /// The codec rate the chain runs at, Hz (`Processor::sample_rate`, built with
    /// `ENABLE_HARDWARE_24K`): 24 kHz for dirty tape, 48 kHz for clean tape (which runs its grains
    /// at 24 kHz behind 2:1 converters) and digital, 32 kHz for vintage digital.
    #[must_use]
    pub fn sample_rate(self) -> u32 {
        match self {
            Self::DirtyTape => 24_000,
            Self::CleanTape | Self::Digital => 48_000,
            Self::VintageDigital => 32_000,
        }
    }

    /// The module's block at this rate, frames (its codec rate / 4000).
    #[must_use]
    pub fn block_size(self) -> usize {
        usize::try_from(self.sample_rate() / 4000).unwrap_or(12)
    }
}

/// The delay memory, sized at construction as seconds of 16-bit stereo at 48 kHz (the digital
/// quality's stereo buffer; every other layout divides the same bits: 12-bit buffers hold 4/3 as
/// many frames, 8-bit ones twice as many, mono ones twice as many again).
///
/// The firmware's is 4 seconds (768000 bytes, [`BufferSize::FIRMWARE`]), which is also the
/// shortest: in the frozen delay mode the firmware cuts the buffer into slices of the clock
/// interval, which can reach 4 seconds at the quality's rate, and needs at least one slice. The
/// longest is 8 seconds ([`BufferSize::MAX`]): every delay tap is a 22.10 fixed-point number in 32
/// bits, and the longest one (the clean tape quality's mono buffer, 16/3 of the frames, plus the
/// 68-sample minimum delay and the 1024-sample pitch-shifter window) must stay below 2^21 samples;
/// that holds up to 393011 frames (8.19 s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferSize {
    frames: usize,
}

impl BufferSize {
    /// The module's: 4 seconds (192000 frames).
    pub const FIRMWARE: Self = Self { frames: 192_000 };
    /// The shortest: the firmware's.
    pub const MIN: Self = Self::FIRMWARE;
    /// The longest: 8 seconds (384000 frames).
    pub const MAX: Self = Self { frames: 384_000 };

    /// A buffer of `frames` 16-bit stereo frames at 48 kHz; `None` outside [`BufferSize::MIN`] to
    /// [`BufferSize::MAX`].
    #[must_use]
    pub fn new(frames: usize) -> Option<Self> {
        (Self::MIN.frames..=Self::MAX.frames).contains(&frames).then_some(Self { frames })
    }

    /// A buffer of `seconds` of 16-bit stereo at 48 kHz, to the nearest frame; `None` outside 4 to
    /// 8 seconds.
    #[must_use]
    pub fn from_seconds(seconds: f32) -> Option<Self> {
        if !seconds.is_finite() || seconds <= 0.0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (f64::from(seconds) * 48_000.0).round() as usize;
        Self::new(frames)
    }

    /// Frames of 16-bit stereo at 48 kHz.
    #[must_use]
    pub fn frames(self) -> usize {
        self.frames
    }

    /// Seconds of 16-bit stereo at 48 kHz.
    #[must_use]
    pub fn seconds(self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let s = self.frames as f32 / 48_000.0;
        s
    }

    /// `kDelayBufferSizeBits`.
    #[must_use]
    pub fn bits(self) -> usize {
        self.frames * 32
    }

    /// The stereo delay line at `media`, frames at the quality's processing rate (the firmware's
    /// `max_delay` template argument: the bits divided by twice the resolution); the mono line holds
    /// twice as many.
    #[must_use]
    pub fn max_delay(self, media: Media) -> i32 {
        let bits_per_frame = match media {
            Media::DirtyTape => 16,
            Media::CleanTape | Media::VintageDigital => 24,
            Media::Digital => 32,
        };
        i32::try_from(self.bits() / bits_per_frame).unwrap_or(i32::MAX)
    }
}

/// The SEED button (`SeedButtonState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SeedButton {
    /// Released: nothing seeds grains unless the SEED input does.
    #[default]
    Off,
    /// Held: a gate (taps the delay clock; seeds grains while DENSITY is near noon).
    On,
    /// Latched: grains flow by DENSITY; with a SEED input patched, the input clocks them.
    Locked,
}

/// What the chain reads each block: the module's knobs, attenuverters and CVs as its CV reader
/// hands them over (`beads/beads.cc`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    /// TIME (0 to 1): where grains start, 0 newest to 1 oldest; in the delay mode the delay
    /// multiplier, or while frozen the slice. CV within plus or minus 1.6.
    pub time: Parameter,
    /// SIZE (0 to 1): grain length, reversed below a third; above 0.99 the delay mode. Its
    /// attenuverter is also the delay's bandwidth. CV within plus or minus 1.6.
    pub size: Parameter,
    /// SHAPE (0 to 1): the grain or tap envelope. CV within plus or minus 1.6.
    pub shape: Parameter,
    /// PITCH, semitones: -24 to 24 on the knob, plus or minus 60 by CV.
    pub pitch: Parameter,
    /// DENSITY (0 to 1): below noon a regular grain rate, above it a random one, near noon taps
    /// only; in the delay mode the clock ratio.
    pub density: f32,
    /// The DENSITY CV, -1 to 1 (away from noon it becomes exponential FM).
    pub density_cv: f32,
    /// FEEDBACK, 0 to 1 (the synth: the wavetable bank).
    pub feedback: f32,
    /// DRY/WET, 0 to 1.
    pub dry_wet: f32,
    /// REVERB, 0 to 1.
    pub reverb: f32,
    /// The SEED button.
    pub seed_button: SeedButton,
    /// The monophonic grain mode (an advanced setting, off in the shipped firmware).
    pub monophonic: bool,
    /// The gate output setting: the left output carries the mono mix, the right the trigger gate.
    pub output_gate: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_limits_hold() {
        assert_eq!(BufferSize::FIRMWARE.bits(), 768 * 1000 * 8);
        assert_eq!(BufferSize::FIRMWARE.max_delay(Media::Digital), 192_000);
        assert_eq!(BufferSize::FIRMWARE.max_delay(Media::VintageDigital), 256_000);
        assert_eq!(BufferSize::FIRMWARE.max_delay(Media::DirtyTape), 384_000);
        assert_eq!(BufferSize::from_seconds(4.0), Some(BufferSize::MIN));
        assert_eq!(BufferSize::from_seconds(8.0), Some(BufferSize::MAX));
        assert_eq!(BufferSize::from_seconds(3.99), None);
        assert_eq!(BufferSize::from_seconds(8.01), None);
        assert_eq!(BufferSize::from_seconds(f32::NAN), None);
        assert_eq!(BufferSize::new(191_999), None);
        assert_eq!(BufferSize::new(384_001), None);
        // The longest tap: the clean tape's mono controller sees 2 x 2 x 4/3 of the frames, plus
        // the minimum delay and the pitch-shifter window, in 22.10 fixed point below 2^31.
        let longest = i64::from(BufferSize::MAX.max_delay(Media::CleanTape)) * 4 + 68 + 1024;
        assert!(longest << 10 < i64::from(i32::MAX));
        assert!((393_011_i64 * 32 / 24 * 4 + 68 + 1024) << 10 <= i64::from(i32::MAX));
        assert!((393_012_i64 * 32 / 24 * 4 + 68 + 1024) << 10 > i64::from(i32::MAX));
    }
}

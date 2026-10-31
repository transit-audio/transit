//! The texture oracle (#241): the firmware's whole texture effect chain (`beads::Processor`), its
//! synth (`beads::Synth`) and its input stage (`beads::GainControl`), rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawTexture`], [`crate::ffi::RawTextureSynth`] and
//! [`crate::ffi::RawTextureInput`]. One [`TextureOracle`] is one effect chain: the grain and delay
//! triggers, the granular processors of the four buffer qualities ([`Media`]) sharing one delay
//! memory, the dry/wet crossfade, the tape wow and flutter, the limiters and the reverb, in float
//! stereo frames at the quality's codec rate ([`Media::sample_rate`]) in blocks of up to 24 frames
//! ([`TEXTURE_MAX_BLOCK_SIZE`]). It is driven as the firmware's desktop test harness drives it
//! (`beads/test/beads_test.cc`): `Init`, `Configure`, then `Process` per block.
//!
//! The oracle builds the chain twice: on the module's memory ([`TEXTURE_FIRMWARE_BUFFER_BITS`],
//! [`Session::texture`]) and on the port's longest buffer ([`TEXTURE_LONG_BUFFER_BITS`],
//! [`Session::texture_long`]).

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CTextureControls, CTextureParameter, RawTexture, RawTextureInput, RawTextureSynth, Session, TextureBuild};

/// The largest block, frames: the codec's `kMaxCodecBlockSize`. The module renders its codec rate
/// / 4000 frames per block ([`Media::block_size`]).
pub const TEXTURE_MAX_BLOCK_SIZE: usize = 24;

/// The module's delay memory, bits (`kDelayBufferSizeBits`, 768000 bytes): 4 seconds of 16-bit
/// stereo at 48 kHz.
pub const TEXTURE_FIRMWARE_BUFFER_BITS: usize = 768 * 1000 * 8;

/// The delay memory of the oracle's second build, bits: 8 seconds of 16-bit stereo at 48 kHz
/// (384000 frames), the port's longest buffer.
pub const TEXTURE_LONG_BUFFER_BITS: usize = 384_000 * 32;

/// The input samples the oracle accepts: finite, within plus or minus 16 (+24 dBFS). Inside it no
/// float the chain converts to an integer (the delay memory's quantizers, the dry path of the
/// vintage and dirty qualities) can leave the 32-bit range.
pub const INPUT_LIMIT: f32 = 16.0;

/// A buffer quality (`Processor::Media` in `beads/dsp/processor.h`), in the firmware's order.
/// Each runs the codec at its own rate: the port and the oracle take frames at that rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Media {
    /// 8-bit mu-law at 24 kHz (the codec itself runs at 24 kHz), with tape wow and flutter and
    /// soft clipping into the buffer.
    DirtyTape,
    /// 12-bit at 24 kHz behind the firmware's 2:1 converters (the codec runs at 48 kHz).
    CleanTape,
    /// 12-bit at 32 kHz (the codec runs at 32 kHz).
    VintageDigital,
    /// 16-bit at 48 kHz.
    Digital,
}

impl Media {
    /// Every quality, in the firmware's order.
    pub const ALL: [Self; 4] = [Self::DirtyTape, Self::CleanTape, Self::VintageDigital, Self::Digital];

    /// The firmware's index.
    #[must_use]
    pub fn index(self) -> i32 {
        match self {
            Self::DirtyTape => 0,
            Self::CleanTape => 1,
            Self::VintageDigital => 2,
            Self::Digital => 3,
        }
    }

    /// The codec rate the chain runs at (`Processor::sample_rate`, with `ENABLE_HARDWARE_24K`).
    #[must_use]
    pub fn sample_rate(self) -> u32 {
        match self {
            Self::DirtyTape => 24_000,
            Self::CleanTape | Self::Digital => 48_000,
            Self::VintageDigital => 32_000,
        }
    }

    /// The module's block at this rate, frames (the codec rate / 4000).
    #[must_use]
    pub fn block_size(self) -> usize {
        usize::try_from(self.sample_rate() / 4000).unwrap_or(12)
    }

    /// Whether a block must hold an even number of frames (the clean tape's 2:1 converters).
    #[must_use]
    pub fn even_blocks(self) -> bool {
        self == Self::CleanTape
    }

    /// A stable kebab-case label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::DirtyTape => "dirty-tape",
            Self::CleanTape => "clean-tape",
            Self::VintageDigital => "vintage",
            Self::Digital => "digital",
        }
    }
}

/// The SEED button (`SeedButtonState` in `beads/dsp/trigger_generator.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SeedButton {
    /// Released: grains come from density alone.
    #[default]
    Off,
    /// Held: a gate (taps the delay clock, seeds grains while density is near noon).
    On,
    /// Latched: grains are seeded continuously; with a SEED input patched, the input clocks them.
    Locked,
}

impl SeedButton {
    fn index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Locked => 2,
        }
    }
}

/// A parameter with its attenuverter and CV (`RandomizedParameter`): without CV the attenuverter
/// sets the amount of internal random modulation (positive: smooth, negative: per grain); with CV
/// it scales the CV.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameter {
    /// The knob.
    pub base: f32,
    /// The attenuverter, -1 to 1.
    pub attenuverter: f32,
    /// The CV, in the knob's units.
    pub cv: f32,
    /// Whether a CV is patched.
    pub has_cv: bool,
}

impl Parameter {
    fn to_c(self) -> CTextureParameter {
        CTextureParameter { base: self.base, attenuverter: self.attenuverter, cv: self.cv, has_cv: u8::from(self.has_cv), reserved: [0; 3] }
    }
}

/// What the module's main loop hands the chain each block (`beads/beads.cc`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    /// TIME: where in the buffer grains start (0 newest to 1 oldest); in the delay mode, the
    /// delay time multiplier or slice.
    pub time: Parameter,
    /// SIZE: grain length (below a third, reversed); above 0.99, the delay mode.
    pub size: Parameter,
    /// SHAPE: the grain or tap envelope.
    pub shape: Parameter,
    /// PITCH, semitones (-24 to 24 on the knob, plus or minus 60 by CV).
    pub pitch: Parameter,
    /// DENSITY: below noon a regular grain rate, above it a random one; its CV, -1 to 1.
    pub density: f32,
    /// The density CV.
    pub density_cv: f32,
    /// FEEDBACK, 0 to 1 (the synth: the wavetable bank).
    pub feedback: f32,
    /// DRY/WET, 0 to 1.
    pub dry_wet: f32,
    /// REVERB, 0 to 1.
    pub reverb: f32,
    /// The SEED button.
    pub seed_button: SeedButton,
    /// The monophonic grain mode (an advanced setting; off on the shipped firmware).
    pub monophonic: bool,
    /// The gate output setting: the right output carries the trigger gate, the left the mono mix.
    pub output_gate: bool,
}

impl Controls {
    pub(crate) fn to_c(self) -> CTextureControls {
        CTextureControls {
            time: self.time.to_c(),
            size: self.size.to_c(),
            shape: self.shape.to_c(),
            pitch: self.pitch.to_c(),
            density: self.density,
            density_cv: self.density_cv,
            feedback: self.feedback,
            dry_wet: self.dry_wet,
            reverb: self.reverb,
            seed_button: self.seed_button.index(),
            monophonic: u8::from(self.monophonic),
            output_gate: u8::from(self.output_gate),
            reserved: [0; 2],
        }
    }
}

/// Whether the C++ can take these controls (invariant 4 of the FFI): the module's own ranges.
///
/// The bounds that matter for the C++: the pitch reaches `SemitonesToRatio` (a 256-entry table for
/// -128 to 127 semitones) as at most the knob (24) plus 60 by CV, plus 24 of internal randomness in
/// the synth; the density CV reaches it times 60. Time, size and shape reach tables only through
/// values the firmware clamps to 0 to 1 first. Every value must be finite: a NaN reaching a
/// float-to-int cast is undefined behavior in C++.
#[must_use]
pub fn controls_in_range(c: &Controls) -> bool {
    let unit = |x: f32| (0.0..=1.0).contains(&x);
    let att = |x: f32| (-1.0..=1.0).contains(&x);
    let knob = |p: &Parameter| unit(p.base) && att(p.attenuverter) && (-1.6..=1.6).contains(&p.cv);
    knob(&c.time)
        && knob(&c.size)
        && knob(&c.shape)
        && (-24.0..=24.0).contains(&c.pitch.base)
        && att(c.pitch.attenuverter)
        && (-60.0..=60.0).contains(&c.pitch.cv)
        && unit(c.density)
        && att(c.density_cv)
        && unit(c.feedback)
        && unit(c.dry_wet)
        && unit(c.reverb)
}

/// Whether every input sample is finite and within [`INPUT_LIMIT`].
#[must_use]
pub fn input_in_range(input: &[[f32; 2]]) -> bool {
    input.iter().flatten().all(|x| x.abs() <= INPUT_LIMIT)
}

/// The rates the input stage is built for: the codec's (24, 32 and 48 kHz).
#[must_use]
pub fn input_sample_rate_supported(sample_rate: f32) -> bool {
    [24_000.0, 32_000.0, 48_000.0].contains(&sample_rate)
}

/// One effect chain of the oracle.
pub struct TextureOracle<'s> {
    raw: RawTexture<'s>,
}

impl TextureOracle<'_> {
    /// Selects the quality and the mono setting, as the module does when either changes: the
    /// quality's buffer is cleared, the triggers, the delay controller and the limiters restart;
    /// the reverb keeps its memory.
    pub fn configure(&mut self, media: Media, mono: bool) {
        // Every `Media` index is in range, so the wrapper never refuses it.
        let _ = self.raw.configure(media.index(), mono);
    }

    /// Whether the chain runs with flush-to-zero (`true`, the default: the module sets FPSCR.FZ and
    /// the oracle sets the x86 equivalent around every call) or in the default floating-point mode
    /// (`false`). An evidence switch: the output is float, so the tests can show the flush changes
    /// its bits, and the port, which reproduces the flush, is held to it.
    pub fn set_flush_to_zero(&mut self, flush: bool) {
        self.raw.set_flush(flush);
    }

    /// The panel's MEDIA + FREEZE gesture: the recording's length drops to zero.
    pub fn trim(&mut self) {
        self.raw.trim();
    }

    /// Processes one block of `input.len()` stereo frames (1 to [`TEXTURE_MAX_BLOCK_SIZE`], even at
    /// the clean-tape quality) into `out`. `freeze` holds one flag per frame; `seed` one gate-flags
    /// byte per frame (bit 0 high, bit 1 rising, bit 2 falling), or `None` when nothing is patched.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty, too long, odd at the clean-tape quality, or the
    /// lengths differ; [`Error::BadParameter`] when a control or a sample is outside what the C++
    /// can take ([`controls_in_range`], [`input_in_range`]).
    pub fn process(
        &mut self,
        controls: &Controls,
        freeze: &[bool],
        seed: Option<&[u8]>,
        input: &[[f32; 2]],
        out: &mut [[f32; 2]],
    ) -> Result<(), Error> {
        let n = input.len();
        if n == 0 || n > TEXTURE_MAX_BLOCK_SIZE || out.len() != n || freeze.len() != n || seed.is_some_and(|s| s.len() != n) {
            return Err(Error::BadBlock);
        }
        if !controls_in_range(controls) || !input_in_range(input) {
            return Err(Error::BadParameter);
        }
        self.raw.process(controls, freeze, seed, input, out).ok_or(Error::BadBlock)
    }
}

/// The synth of the oracle (what the module plays with nothing patched), at 48 kHz.
pub struct TextureSynthOracle<'s> {
    raw: RawTextureSynth<'s>,
}

impl TextureSynthOracle<'_> {
    /// Renders one block of `out.len()` stereo frames (1 to [`TEXTURE_MAX_BLOCK_SIZE`]) at
    /// `resolution` (the quality setting, 0 to 3: 4-bit, 16-bit with 6-bit grains, 7-bit, 16-bit).
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] for a bad block length; [`Error::BadParameter`] for a resolution outside 0
    /// to 3 or a control outside [`controls_in_range`].
    pub fn render(
        &mut self,
        controls: &Controls,
        resolution: usize,
        freeze: &[bool],
        seed: Option<&[u8]>,
        out: &mut [[f32; 2]],
    ) -> Result<(), Error> {
        let n = out.len();
        if n == 0 || n > TEXTURE_MAX_BLOCK_SIZE || freeze.len() != n || seed.is_some_and(|s| s.len() != n) {
            return Err(Error::BadBlock);
        }
        if resolution > 3 || !controls_in_range(controls) {
            return Err(Error::BadParameter);
        }
        self.raw.render(controls, i32::try_from(resolution).unwrap_or(0), freeze, seed, out).ok_or(Error::BadBlock)
    }
}

/// The input stage of the oracle: raw 32-bit codec frames to floats, with the automatic or manual
/// gain and the soft clipping.
pub struct TextureInputOracle<'s> {
    raw: RawTextureInput<'s>,
}

impl TextureInputOracle<'_> {
    /// `GainControl::Init`, as the module calls it on every codec restart (`reset` false keeps the
    /// learned gain).
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] for a mode outside 0 to 2 or a rate outside 24, 32 and 48 kHz.
    pub fn init(&mut self, sample_rate: f32, mode: usize, reset: bool) -> Result<(), Error> {
        let mode = i32::try_from(mode).map_err(|_| Error::BadParameter)?;
        self.raw.init(sample_rate, mode, reset).ok_or(Error::BadParameter)
    }

    /// Converts one block. `active` is false while the codec restarts; `gain` 0 is the automatic
    /// gain, 1 to 255 the manual gain; `normalized` says which inputs are unpatched (an unpatched
    /// side copies the other; both unpatched gives silence).
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] for a bad block length.
    pub fn process(&mut self, active: bool, gain: u8, normalized: [bool; 2], input: &[[i32; 2]], out: &mut [[f32; 2]]) -> Result<(), Error> {
        self.raw.process(active, gain, normalized, input, out).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new effect chain on the module's memory, Init'ed and Configure'd at `media` and `mono`.
    /// Seed first: the grains, the triggers and the tape flutter draw from the shared random state.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] when the allocation fails.
    pub fn texture(&self, media: Media, mono: bool) -> Result<TextureOracle<'_>, Error> {
        let raw = RawTexture::new(self, TextureBuild::Firmware, media.index(), mono).ok_or(Error::BadParameter)?;
        Ok(TextureOracle { raw })
    }

    /// As [`Session::texture`], on the port's longest buffer ([`TEXTURE_LONG_BUFFER_BITS`]).
    ///
    /// # Errors
    ///
    /// As [`Session::texture`].
    pub fn texture_long(&self, media: Media, mono: bool) -> Result<TextureOracle<'_>, Error> {
        let raw = RawTexture::new(self, TextureBuild::Long, media.index(), mono).ok_or(Error::BadParameter)?;
        Ok(TextureOracle { raw })
    }

    /// A new synth. Seed first.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] when the allocation fails.
    pub fn texture_synth(&self) -> Result<TextureSynthOracle<'_>, Error> {
        let raw = RawTextureSynth::new(self).ok_or(Error::BadParameter)?;
        Ok(TextureSynthOracle { raw })
    }

    /// A new input stage at `sample_rate` with soft-clipping `mode` (0 off, 1 on, 2 dynamic).
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] for a mode outside 0 to 2, a rate outside 24, 32 and 48 kHz, or a
    /// failed allocation.
    pub fn texture_input(&self, sample_rate: f32, mode: usize) -> Result<TextureInputOracle<'_>, Error> {
        let mode = i32::try_from(mode).map_err(|_| Error::BadParameter)?;
        let raw = RawTextureInput::new(self, sample_rate, mode).ok_or(Error::BadParameter)?;
        Ok(TextureInputOracle { raw })
    }
}

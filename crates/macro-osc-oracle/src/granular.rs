//! The granular texture oracle (#199): the firmware's granular processor
//! (`clouds::GranularProcessor`) in its granular playback mode and in its pitch-shifter /
//! time-stretcher mode ([`PlaybackMode`]), rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawGranular`]. One [`GranularOracle`] is one processor: the
//! recording buffers, up to 64 grains, the feedback path, the diffuser, the reverb and the dry/wet
//! crossfade, at 32 kHz ([`GRANULAR_SAMPLE_RATE`]) in blocks of up to 32 stereo int16 frames
//! ([`GRANULAR_BLOCK_SIZE`]), as the module's codec delivers them. It is driven as the firmware's
//! desktop test harness drives it: `Prepare` after `Init` and after every block.

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CGranularParameters, RawGranular, Session};

/// The processor's block size, `kMaxBlockSize` in `clouds/dsp/frame.h`: the codec renders 32
/// frames per call.
pub const GRANULAR_BLOCK_SIZE: usize = 32;

/// The firmware's sample rate: the codec runs at 32 kHz (`clouds/clouds.cc`), and the low-fidelity
/// qualities run the grains at half of it.
pub const GRANULAR_SAMPLE_RATE: f32 = 32_000.0;

/// The module's memory blocks, bytes (`block_mem` and `block_ccm` in `clouds/clouds.cc`): the large
/// one records the mono buffer, or the left channel plus the FX workspace in stereo; the small one
/// records the right channel, or holds the workspace in mono.
pub const FIRMWARE_LARGE_BUFFER: usize = 118_784;

/// See [`FIRMWARE_LARGE_BUFFER`].
pub const FIRMWARE_SMALL_BUFFER: usize = 65_536 - 128;

/// The largest large block the oracle accepts, bytes: every layout then records fewer than 131072
/// samples, below which a grain's 16.16 fixed-point phase in 32 bits cannot overflow.
pub const MAX_LARGE_BUFFER: usize = 131_072;

/// The largest grain size the oracle accepts: strictly below 1, so `Interpolate(lut_grain_size,
/// size, 256)` reads at most entries 255 and 256 of the upstream table's 257. (On the module size
/// reaches 1.0 and the firmware reads one entry past the table, multiplied by a zero fraction; the
/// oracle's build pads the table, but the sweep keeps to the upstream table.)
pub const MAX_SIZE: f32 = 0.999_999_94;

/// The largest dry/wet the oracle accepts. It does not bound the crossfade's table index: that
/// comes from a per-sample ramp toward the parameter, which can overshoot 1.0 by rounding
/// (`ParameterInterpolator`), so the oracle's build pads `lut_xfade_in` and `lut_xfade_out`
/// instead (`patches/all/0002`, VENDOR.md). The top value just keeps the parameter itself below
/// 1, as for size.
pub const MAX_DRY_WET: f32 = 0.999_999_94;

/// The pitch the oracle accepts, semitones: the module's CV scaler constrains it to -48 to 48, and
/// `SemitonesToRatio` reads its 256-entry table for -128 to 127.
pub const PITCH_RANGE: std::ops::RangeInclusive<f32> = -48.0..=48.0;

/// The buffer quality (`set_quality` in `clouds/dsp/granular_processor.h`), in the firmware's
/// order: bit 0 selects mono, bit 1 the 8-bit mu-law buffers with the grains at 16 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quality {
    /// 16-bit stereo at 32 kHz (quality 0, the module's default).
    Stereo,
    /// 16-bit mono at 32 kHz (quality 1): the input is summed, the buffer is about twice as long.
    Mono,
    /// 8-bit mu-law stereo at 16 kHz (quality 2): four times the stereo buffer length.
    StereoLowFidelity,
    /// 8-bit mu-law mono at 16 kHz (quality 3).
    MonoLowFidelity,
}

impl Quality {
    /// Every quality, in index order.
    pub const ALL: [Self; 4] = [Self::Stereo, Self::Mono, Self::StereoLowFidelity, Self::MonoLowFidelity];

    /// The firmware's quality index.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|q| *q == self).unwrap_or(0)
    }

    /// Whether the buffers are 8-bit mu-law and the grains run at half the rate (then a block must
    /// have an even number of frames).
    #[must_use]
    pub fn low_fidelity(self) -> bool {
        self.index() & 2 != 0
    }

    /// A stable kebab-case label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Stereo => "stereo",
            Self::Mono => "mono",
            Self::StereoLowFidelity => "stereo-lofi",
            Self::MonoLowFidelity => "mono-lofi",
        }
    }
}

/// The playback modes the oracle wires (`PlaybackMode` in `clouds/dsp/granular_processor.h`; the
/// looping delay and the spectral mode are not wired).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackMode {
    /// `PLAYBACK_MODE_GRANULAR`: overlapping grains (ported in `transit-granular`).
    Granular,
    /// `PLAYBACK_MODE_STRETCH`: the pitch shifter and time stretcher (a WSOLA player that aligns
    /// its windows with a correlator, then the low-pass and high-pass filters TEXTURE sets).
    Stretch,
}

impl PlaybackMode {
    /// The modes the oracle wires, in the firmware's order.
    pub const ALL: [Self; 2] = [Self::Granular, Self::Stretch];

    /// The firmware's mode index.
    #[must_use]
    pub fn index(self) -> i32 {
        match self {
            Self::Granular => 0,
            Self::Stretch => 1,
        }
    }

    /// A stable label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Granular => "granular",
            Self::Stretch => "stretch",
        }
    }
}

/// What the module's CV scaler hands the processor each block (`Parameters` in
/// `clouds/dsp/parameters.h`; the granular and spectral sub-structures are derived by the
/// processor).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameters {
    /// Where in the recording the grains start, 0 (newest) to 1 (oldest).
    pub position: f32,
    /// Grain size, 0 to 1 (1024 to 16384 samples).
    pub size: f32,
    /// Grain transposition, semitones.
    pub pitch: f32,
    /// Grain density: below 0.5 grains are seeded at a regular rate, above it at random; the
    /// distance from 0.5 sets the overlap.
    pub density: f32,
    /// Grain window shape, then (above 0.75) the diffuser amount.
    pub texture: f32,
    /// Dry/wet crossfade.
    pub dry_wet: f32,
    /// Random panning of the grains.
    pub stereo_spread: f32,
    /// Feedback of the processed signal into the recording.
    pub feedback: f32,
    /// Reverb amount.
    pub reverb: f32,
    /// Freeze: the recording stops.
    pub freeze: bool,
    /// A trigger in this block: one grain is seeded at once.
    pub trigger: bool,
    /// The trigger input's gate (read by the other playback modes only).
    pub gate: bool,
}

impl Parameters {
    pub(crate) fn to_c(self) -> CGranularParameters {
        CGranularParameters {
            position: self.position,
            size: self.size,
            pitch: self.pitch,
            density: self.density,
            texture: self.texture,
            dry_wet: self.dry_wet,
            stereo_spread: self.stereo_spread,
            feedback: self.feedback,
            reverb: self.reverb,
            freeze: u8::from(self.freeze),
            trigger: u8::from(self.trigger),
            gate: u8::from(self.gate),
            reserved: 0,
        }
    }
}

/// Whether the C++ can take these parameters (invariant 4 of the FFI).
///
/// The grain player reads `Interpolate(lut_grain_size, size, 256)` (257 entries upstream) and size
/// stays below 1 ([`MAX_SIZE`]); dry/wet stays below 1 too ([`MAX_DRY_WET`]), though the crossfade's
/// index is a ramp that can overshoot it, which the padded tables absorb. The pan of a grain, `0.5 + stereo_spread * (random - 0.5)`, indexes `lut_sin`
/// (1281 entries) at most at 513 for a spread within 0 to 1. Pitch reaches `SemitonesToRatio`
/// within [`PITCH_RANGE`]. Position, density, texture, feedback and reverb index no table but must
/// stay within 0 to 1, the module's range, which the grain start and the filter cutoffs assume.
/// Every value must be finite: a NaN reaching a float-to-int cast is undefined behavior in C++.
///
/// Three reads one past an array cannot be refused here because they happen inside the processor in
/// normal use: a grain's envelope reads `Interpolate(lut_window, gain, 4096)` at its peak, and when
/// the envelope phase lands exactly on 1.0 (grains whose width is a power of two, which sizes 0,
/// 0.25, 0.5 and 0.75 give) it reads entry 4097 of 4097; the dry/wet ramp's overshoot reads past
/// `lut_xfade_in` and `lut_xfade_out`; and every resume after a freeze reads entry 256 of the
/// 256-entry crossfade tail. The oracle's build gives each array one more entry
/// (`patches/all/0002`): the last repeated for the tables, a zero for the tail (read at a zero
/// gain), so each read stays inside its array and returns what the port's clamp returns.
#[must_use]
pub fn parameters_in_range(parameters: &Parameters) -> bool {
    let unit = |x: f32| (0.0..=1.0).contains(&x);
    unit(parameters.position)
        && (0.0..=MAX_SIZE).contains(&parameters.size)
        && PITCH_RANGE.contains(&parameters.pitch)
        && unit(parameters.density)
        && unit(parameters.texture)
        && (0.0..=MAX_DRY_WET).contains(&parameters.dry_wet)
        && unit(parameters.stereo_spread)
        && unit(parameters.feedback)
        && unit(parameters.reverb)
}

/// One granular processor of the oracle at 32 kHz, in the granular playback mode.
pub struct GranularOracle<'s> {
    raw: RawGranular<'s>,
}

impl GranularOracle<'_> {
    /// Selects the quality, as the panel does; the next block is silent while the buffers are
    /// re-initialized.
    pub fn set_quality(&mut self, quality: Quality) {
        // Every `Quality` index is in range, so the wrapper never refuses it.
        let _ = self.raw.set_quality(i32::try_from(quality.index()).unwrap_or(0));
    }

    /// Selects the playback mode, as the panel does. Between these two modes the firmware keeps the
    /// recording and resets the filters.
    pub fn set_mode(&mut self, mode: PlaybackMode) {
        // Every `PlaybackMode` index is in range, so the wrapper never refuses it.
        let _ = self.raw.set_mode(mode.index());
    }

    /// Bypass: the input is copied to the output.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.raw.set_bypass(bypass);
    }

    /// Silence: the output is zero and nothing is recorded (the module sets it while it loads a
    /// saved buffer).
    pub fn set_silence(&mut self, silence: bool) {
        self.raw.set_silence(silence);
    }

    /// Processes one block of `input.len()` stereo frames (1 to [`GRANULAR_BLOCK_SIZE`], even at a
    /// low-fidelity quality) into `out`.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty, too long, odd at a low-fidelity quality, or the
    /// lengths differ; [`Error::BadParameter`] when a parameter is outside what the C++ can take
    /// without reading past a table ([`parameters_in_range`]).
    pub fn process(&mut self, parameters: &Parameters, input: &[[i16; 2]], out: &mut [[i16; 2]]) -> Result<(), Error> {
        let n = input.len();
        if n == 0 || n > GRANULAR_BLOCK_SIZE || out.len() != n {
            return Err(Error::BadBlock);
        }
        if !parameters_in_range(parameters) {
            return Err(Error::BadParameter);
        }
        self.raw.process(parameters, input, out).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new granular processor on two zeroed memory blocks of `large` and `small` bytes (the
    /// module's are [`FIRMWARE_LARGE_BUFFER`] and [`FIRMWARE_SMALL_BUFFER`]), at `quality`, in the
    /// granular mode, as the firmware's harness leaves it before its first block. Seed first: the
    /// grains' seeding and panning draw from the shared random state.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] when the sizes leave no room for the FX workspace or for the longest
    /// grain (the small block must hold at least [`Session::granular_workspace_size`] bytes and
    /// more than 16392 samples of 16 bits, the large one the small one plus the workspace, both a
    /// multiple of 4), when the large block exceeds [`MAX_LARGE_BUFFER`] (a grain's 16.16 phase
    /// would overflow), or when the allocation fails.
    pub fn granular(&self, large: usize, small: usize, quality: Quality) -> Result<GranularOracle<'_>, Error> {
        self.granular_in_mode(large, small, quality, PlaybackMode::Granular)
    }

    /// As [`Session::granular`], in playback mode `mode`.
    ///
    /// # Errors
    ///
    /// As [`Session::granular`].
    pub fn granular_in_mode(&self, large: usize, small: usize, quality: Quality, mode: PlaybackMode) -> Result<GranularOracle<'_>, Error> {
        let quality = i32::try_from(quality.index()).unwrap_or(0);
        let raw = RawGranular::new(self, large, small, quality, mode.index()).ok_or(Error::BadParameter)?;
        Ok(GranularOracle { raw })
    }
}

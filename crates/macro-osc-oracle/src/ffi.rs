//! The only module with `unsafe`: the C ABI of the wrappers in `cpp/` (the macro oscillator, the
//! resonator, the modal voice, the granular processor, the texture synthesizer, the meta-modulator and the random
//! sequencer), and the session lock that makes it sound to call.
//!
//! Soundness rests on three invariants, each enforced here:
//!
//! 1. **One caller at a time.** The vendored DSP keeps its random state in one process-wide static
//!    (`stmlib::Random`), so two threads rendering at once would race on it (undefined behavior in
//!    C++) and would also make the noise sequence depend on scheduling. Every call goes through a
//!    [`Session`], which holds a process-wide mutex and is neither `Send` nor `Sync`, so all calls of
//!    one session stay on the thread that took the lock. Handles borrow their session.
//! 2. **Valid handles.** [`RawVoice`], [`RawEngine`], [`RawResonator`], [`RawModalVoice`] and [`RawGranular`] own a
//!    non-null pointer
//!    returned by the matching `_new` function and free it exactly once, in `Drop`.
//! 3. **Buffer bounds.** Every render checks the length is between 1 and the firmware's maximum block
//!    size before the call, and passes buffers whose lengths match the size it passes.
//! 4. **Table bounds.** Inputs the C++ indexes tables with are range-checked here before the call:
//!    the resonator's tonic, note, FM, chord, macros and input samples (`crate::resonator::inputs_in_range`, which
//!    derives its bounds from the C++), the modal voice's patch, note, modulation, strength and input
//!    samples (`crate::modal_voice::inputs_in_range`), and the granular processor's parameters
//!    (`crate::granular::parameters_in_range`). The granular processor's memory sizes, quality and
//!    mode are checked by the C++ itself: it returns null for sizes that leave no room for its
//!    workspace or its longest grain, or whose recording would reach 131072 samples, where a
//!    grain's 16.16 phase overflows. The firmware also reads one past an array in normal use, where
//!    no input check can prevent it: the grain window at its peak (`lut_window`), the dry/wet ramp,
//!    which can overshoot 1.0 by rounding whatever the parameter (`lut_xfade_in`,
//!    `lut_xfade_out`), and the freeze crossfade's tail (`tail_buffer_`). The build-time patch
//!    `patches/all/0002` gives each of those arrays one more entry (the last repeated, or a zero
//!    the crossfade multiplies by a zero gain), so those reads stay inside them and read what the
//!    port's clamp reads. The texture synthesizer's controls and input samples are range-checked
//!    here too (`crate::texture::controls_in_range`, `crate::texture::input_in_range`: the pitch and
//!    the density CV reach `SemitonesToRatio`, and every sample the chain converts to an integer
//!    stays in range); its media, resolution, mode and block sizes are checked by the C++. Its
//!    firmware has three out-of-bounds cases no input check can refuse, which build-time patches
//!    define (`patches/all/0005` and `0006`): a frozen slice longer than the buffer, a slice shorter
//!    than a sample (a division by zero), and the last wave of the last bank. With these checks and
//!    those patches, no input from safe Rust can make the C++ read or write outside its memory.
//!    The meta-modulator's parameters are range-checked by its wrapper, and its inputs are int16;
//!    its crossfade and fold read their tables at indices computed from the processed signal
//!    (the crossfade's ramp reaches the last entry plus one, the fold goes past either end on
//!    inputs the amplifiers push over full scale), which `patches/all/0010` clamps to the tables.
//!    The random sequencer's controls and clock flags are range-checked by its wrapper (every
//!    knob, state byte and enumeration it indexes a table or an array with); its loaded scales are
//!    checked there too. Its firmware reads past four arrays on inputs no check can refuse (a
//!    random component of exactly 1.0, a channel phase of exactly 1.0), which `patches/all/0011`
//!    clamps to the arrays.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::{Mutex, MutexGuard};

/// Mirrors `OraclePatch` in `cpp/oracle.cc` (the voice patch with fixed-width fields).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CPatch {
    pub(crate) note: f32,
    pub(crate) harmonics: f32,
    pub(crate) timbre: f32,
    pub(crate) morph: f32,
    pub(crate) frequency_modulation_amount: f32,
    pub(crate) timbre_modulation_amount: f32,
    pub(crate) morph_modulation_amount: f32,
    pub(crate) engine: i32,
    pub(crate) decay: f32,
    pub(crate) lpg_colour: f32,
}

/// Mirrors `OracleModulations` in `cpp/oracle.cc` (bools as `u8`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CModulations {
    pub(crate) engine: f32,
    pub(crate) note: f32,
    pub(crate) frequency: f32,
    pub(crate) harmonics: f32,
    pub(crate) timbre: f32,
    pub(crate) morph: f32,
    pub(crate) trigger: f32,
    pub(crate) level: f32,
    pub(crate) frequency_patched: u8,
    pub(crate) timbre_patched: u8,
    pub(crate) morph_patched: u8,
    pub(crate) trigger_patched: u8,
    pub(crate) level_patched: u8,
}

/// Mirrors `OracleEngineParameters` in `cpp/oracle.cc`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CEngineParameters {
    pub(crate) trigger: i32,
    pub(crate) note: f32,
    pub(crate) timbre: f32,
    pub(crate) morph: f32,
    pub(crate) harmonics: f32,
    pub(crate) accent: f32,
}

/// Mirrors `OracleResonatorPatch` in `cpp/resonator_oracle.cc` (the resonator's `Patch`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CResonatorPatch {
    pub(crate) structure: f32,
    pub(crate) brightness: f32,
    pub(crate) damping: f32,
    pub(crate) position: f32,
}

/// Mirrors `OraclePerformanceState` in `cpp/resonator_oracle.cc` (bools as `u8`, after the floats).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CPerformanceState {
    pub(crate) tonic: f32,
    pub(crate) note: f32,
    pub(crate) fm: f32,
    pub(crate) chord: i32,
    pub(crate) strum: u8,
    pub(crate) internal_exciter: u8,
    pub(crate) internal_strum: u8,
    pub(crate) internal_note: u8,
}

/// Mirrors `OracleModalVoicePatch` in `cpp/modal_voice_oracle.cc` (the panel fields of the modal
/// voice's `Patch`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CModalVoicePatch {
    pub(crate) exciter_envelope_shape: f32,
    pub(crate) exciter_bow_level: f32,
    pub(crate) exciter_bow_timbre: f32,
    pub(crate) exciter_blow_level: f32,
    pub(crate) exciter_blow_meta: f32,
    pub(crate) exciter_blow_timbre: f32,
    pub(crate) exciter_strike_level: f32,
    pub(crate) exciter_strike_meta: f32,
    pub(crate) exciter_strike_timbre: f32,
    pub(crate) resonator_geometry: f32,
    pub(crate) resonator_brightness: f32,
    pub(crate) resonator_damping: f32,
    pub(crate) resonator_position: f32,
    pub(crate) space: f32,
}

/// Mirrors `OracleModalVoicePerformance` in `cpp/modal_voice_oracle.cc` (the gate as `u8`, after the
/// floats).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CModalVoicePerformance {
    pub(crate) note: f32,
    pub(crate) modulation: f32,
    pub(crate) strength: f32,
    pub(crate) gate: u8,
}

/// Mirrors `OracleGranularParameters` in `cpp/granular_oracle.cc` (the granular processor's
/// `Parameters`, bools as `u8` after the floats).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGranularParameters {
    pub(crate) position: f32,
    pub(crate) size: f32,
    pub(crate) pitch: f32,
    pub(crate) density: f32,
    pub(crate) texture: f32,
    pub(crate) dry_wet: f32,
    pub(crate) stereo_spread: f32,
    pub(crate) feedback: f32,
    pub(crate) reverb: f32,
    pub(crate) freeze: u8,
    pub(crate) trigger: u8,
    pub(crate) gate: u8,
    pub(crate) reserved: u8,
}

/// Mirrors `OracleTextureParameter` in `cpp/texture_oracle.cc` and `cpp/texture_extras_oracle.cc`
/// (the texture processor's `RandomizedParameter`, the flag as `u8` after the floats).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CTextureParameter {
    pub(crate) base: f32,
    pub(crate) attenuverter: f32,
    pub(crate) cv: f32,
    pub(crate) has_cv: u8,
    pub(crate) reserved: [u8; 3],
}

/// Mirrors `OracleTextureControls` in `cpp/texture_oracle.cc` and `cpp/texture_extras_oracle.cc`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CTextureControls {
    pub(crate) time: CTextureParameter,
    pub(crate) size: CTextureParameter,
    pub(crate) shape: CTextureParameter,
    pub(crate) pitch: CTextureParameter,
    pub(crate) density: f32,
    pub(crate) density_cv: f32,
    pub(crate) feedback: f32,
    pub(crate) dry_wet: f32,
    pub(crate) reverb: f32,
    pub(crate) seed_button: i32,
    pub(crate) monophonic: u8,
    pub(crate) output_gate: u8,
    pub(crate) reserved: [u8; 2],
}

/// Mirrors `OracleModulatorParameters` in `cpp/modulator_oracle.cc` (the meta-modulator's
/// `Parameters` the main path reads).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CModulatorParameters {
    pub(crate) channel_drive: [f32; 2],
    pub(crate) modulation_algorithm: f32,
    pub(crate) modulation_parameter: f32,
    pub(crate) note: f32,
    pub(crate) carrier_shape: i32,
}

/// Mirrors `OracleSequencerControls` in `cpp/sequencer_oracle.cc` (what the random sequencer's
/// main loop reads each block).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CSequencerControls {
    pub(crate) t_model: i32,
    pub(crate) t_range: i32,
    pub(crate) t_rate: f32,
    pub(crate) t_bias: f32,
    pub(crate) t_jitter: f32,
    pub(crate) deja_vu: f32,
    pub(crate) deja_vu_length: f32,
    pub(crate) t_deja_vu: i32,
    pub(crate) x_deja_vu: i32,
    pub(crate) t_pulse_width_mean: i32,
    pub(crate) t_pulse_width_std: i32,
    pub(crate) x_control_mode: i32,
    pub(crate) x_range: i32,
    pub(crate) x_register_mode: i32,
    pub(crate) x_register_value: f32,
    pub(crate) x_spread: f32,
    pub(crate) x_bias: f32,
    pub(crate) x_steps: f32,
    pub(crate) x_scale: i32,
    pub(crate) y_spread: i32,
    pub(crate) y_bias: i32,
    pub(crate) y_steps: i32,
    pub(crate) y_divider: i32,
    pub(crate) y_range: i32,
    pub(crate) t_reset: i32,
    pub(crate) x_reset: i32,
    pub(crate) t_external: i32,
    pub(crate) xy_clock_source: i32,
}

// SAFETY: these declarations match the `extern "C"` definitions in cpp/oracle.cc,
// cpp/resonator_oracle.cc, cpp/modal_voice_oracle.cc, cpp/granular_oracle.cc, cpp/texture_oracle.cc (in both of its builds)
// cpp/texture_extras_oracle.cc, cpp/modulator_oracle.cc and cpp/sequencer_oracle.cc one for one
// (names, argument order, and C types: `size_t` is `usize`, `void*` is `*mut c_void`, the structs
// are the `#[repr(C)]` mirrors above). The build script compiles that file into this crate.
unsafe extern "C" {
    fn oracle_block_size() -> usize;
    fn oracle_max_block_size() -> usize;
    fn oracle_engine_count() -> i32;
    fn oracle_voice_new(sample_rate: f32) -> *mut std::ffi::c_void;
    fn oracle_voice_free(voice: *mut std::ffi::c_void);
    fn oracle_voice_render(
        voice: *mut std::ffi::c_void,
        patch: *const CPatch,
        modulations: *const CModulations,
        frames: *mut i16,
        size: usize,
    ) -> i32;
    fn oracle_engine_new(index: i32) -> *mut std::ffi::c_void;
    fn oracle_engine_free(engine: *mut std::ffi::c_void);
    fn oracle_engine_render(
        engine: *mut std::ffi::c_void,
        parameters: *const CEngineParameters,
        out: *mut f32,
        aux: *mut f32,
        size: usize,
        already_enveloped: *mut u8,
    ) -> i32;
    fn oracle_seed(seed: u32);
    fn oracle_rng_state() -> u32;
    fn oracle_resonator_max_block_size() -> usize;
    fn oracle_resonator_max_polyphony() -> i32;
    fn oracle_resonator_model_count() -> i32;
    fn oracle_resonator_new() -> *mut std::ffi::c_void;
    fn oracle_resonator_free(part: *mut std::ffi::c_void);
    fn oracle_resonator_set_model(part: *mut std::ffi::c_void, model: i32) -> i32;
    fn oracle_resonator_set_polyphony(part: *mut std::ffi::c_void, polyphony: i32) -> i32;
    fn oracle_resonator_set_bypass(part: *mut std::ffi::c_void, bypass: u8);
    fn oracle_resonator_process(
        part: *mut std::ffi::c_void,
        performance: *const CPerformanceState,
        patch: *const CResonatorPatch,
        input: *const f32,
        out: *mut f32,
        aux: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_modal_voice_max_block_size() -> usize;
    fn oracle_modal_voice_sample_rate() -> f32;
    fn oracle_modal_voice_new() -> *mut std::ffi::c_void;
    fn oracle_modal_voice_free(part: *mut std::ffi::c_void);
    fn oracle_modal_voice_seed(part: *mut std::ffi::c_void, words: *const u32);
    fn oracle_modal_voice_set_model(part: *mut std::ffi::c_void, model: i32) -> i32;
    fn oracle_modal_voice_set_easter_egg(part: *mut std::ffi::c_void, easter_egg: u8);
    fn oracle_modal_voice_set_bypass(part: *mut std::ffi::c_void, bypass: u8);
    fn oracle_modal_voice_process(
        part: *mut std::ffi::c_void,
        performance: *const CModalVoicePerformance,
        patch: *const CModalVoicePatch,
        blow_in: *const f32,
        strike_in: *const f32,
        main: *mut f32,
        aux: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_granular_max_block_size() -> usize;
    fn oracle_granular_firmware_large_size() -> usize;
    fn oracle_granular_firmware_small_size() -> usize;
    fn oracle_granular_workspace_size() -> usize;
    fn oracle_granular_new(large: usize, small: usize, quality: i32, mode: i32) -> *mut std::ffi::c_void;
    fn oracle_granular_set_mode(processor: *mut std::ffi::c_void, mode: i32) -> i32;
    fn oracle_granular_free(processor: *mut std::ffi::c_void);
    fn oracle_granular_set_quality(processor: *mut std::ffi::c_void, quality: i32) -> i32;
    fn oracle_granular_set_bypass(processor: *mut std::ffi::c_void, bypass: u8);
    fn oracle_granular_set_silence(processor: *mut std::ffi::c_void, silence: u8);
    fn oracle_granular_process(
        processor: *mut std::ffi::c_void,
        parameters: *const CGranularParameters,
        input: *const i16,
        out: *mut i16,
        size: usize,
    ) -> i32;
    fn oracle_texture_max_block_size() -> usize;
    fn oracle_texture_delay_buffer_size_bits() -> usize;
    fn oracle_texture_new(media: i32, mono: u8) -> *mut std::ffi::c_void;
    fn oracle_texture_free(processor: *mut std::ffi::c_void);
    fn oracle_texture_configure(processor: *mut std::ffi::c_void, media: i32, mono: u8) -> i32;
    fn oracle_texture_trim(processor: *mut std::ffi::c_void);
    fn oracle_texture_set_flush(processor: *mut std::ffi::c_void, flush: u8);
    fn oracle_texture_process(
        processor: *mut std::ffi::c_void,
        controls: *const CTextureControls,
        freeze: *const u8,
        seed: *const u8,
        input: *const f32,
        out: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_texture_long_max_block_size() -> usize;
    fn oracle_texture_long_delay_buffer_size_bits() -> usize;
    fn oracle_texture_long_new(media: i32, mono: u8) -> *mut std::ffi::c_void;
    fn oracle_texture_long_free(processor: *mut std::ffi::c_void);
    fn oracle_texture_long_configure(processor: *mut std::ffi::c_void, media: i32, mono: u8) -> i32;
    fn oracle_texture_long_trim(processor: *mut std::ffi::c_void);
    fn oracle_texture_long_set_flush(processor: *mut std::ffi::c_void, flush: u8);
    fn oracle_texture_long_process(
        processor: *mut std::ffi::c_void,
        controls: *const CTextureControls,
        freeze: *const u8,
        seed: *const u8,
        input: *const f32,
        out: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_texture_synth_new() -> *mut std::ffi::c_void;
    fn oracle_texture_synth_free(synth: *mut std::ffi::c_void);
    fn oracle_texture_synth_render(
        synth: *mut std::ffi::c_void,
        controls: *const CTextureControls,
        resolution: i32,
        freeze: *const u8,
        seed: *const u8,
        out: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_texture_input_new(sample_rate: f32, mode: i32) -> *mut std::ffi::c_void;
    fn oracle_texture_input_free(input: *mut std::ffi::c_void);
    fn oracle_texture_input_init(input: *mut std::ffi::c_void, sample_rate: f32, mode: i32, reset: u8) -> i32;
    fn oracle_texture_input_process(
        input: *mut std::ffi::c_void,
        active: u8,
        gain: u8,
        left_normalized: u8,
        right_normalized: u8,
        frames: *const i32,
        out: *mut f32,
        size: usize,
    ) -> i32;
    fn oracle_modulator_codec_block_size() -> usize;
    fn oracle_modulator_max_block_size() -> usize;
    fn oracle_modulator_oversampling() -> usize;
    fn oracle_modulator_new() -> *mut std::ffi::c_void;
    fn oracle_modulator_free(modulator: *mut std::ffi::c_void);
    fn oracle_modulator_set_flush(modulator: *mut std::ffi::c_void, flush: u8);
    fn oracle_modulator_process(
        modulator: *mut std::ffi::c_void,
        parameters: *const CModulatorParameters,
        input: *const i16,
        out: *mut i16,
        size: usize,
    ) -> i32;
    fn oracle_sequencer_block_size() -> usize;
    fn oracle_sequencer_sample_rate() -> i32;
    fn oracle_sequencer_loop_length_count() -> usize;
    fn oracle_sequencer_loop_length(i: usize) -> i32;
    fn oracle_sequencer_y_divider_count() -> usize;
    fn oracle_sequencer_y_divider(i: usize, p: *mut i32, q: *mut i32) -> i32;
    fn oracle_sequencer_preset_scale(index: usize, base_interval: *mut f32, num_degrees: *mut i32, voltages: *mut f32, weights: *mut u8) -> i32;
    fn oracle_sequencer_new(seed: u32) -> *mut std::ffi::c_void;
    fn oracle_sequencer_free(sequencer: *mut std::ffi::c_void);
    fn oracle_sequencer_set_flush(sequencer: *mut std::ffi::c_void, flush: u8);
    fn oracle_sequencer_load_scale(
        sequencer: *mut std::ffi::c_void,
        index: usize,
        base_interval: f32,
        num_degrees: i32,
        voltages: *const f32,
        weights: *const u8,
    ) -> i32;
    fn oracle_sequencer_process(
        sequencer: *mut std::ffi::c_void,
        controls: *const CSequencerControls,
        t_clock: *const u8,
        xy_clock: *const u8,
        size: usize,
        gates: *mut u8,
        voltages: *mut f32,
    ) -> i32;
    fn oracle_sequencer_recorder_new() -> *mut std::ffi::c_void;
    fn oracle_sequencer_recorder_free(recorder: *mut std::ffi::c_void);
    fn oracle_sequencer_recorder_event(recorder: *mut std::ffi::c_void, kind: i32, voltage: f32) -> i32;
    fn oracle_sequencer_recorder_extract(
        recorder: *mut std::ffi::c_void,
        base_interval: *mut f32,
        num_degrees: *mut i32,
        voltages: *mut f32,
        weights: *mut u8,
    ) -> i32;
}

static LOCK: Mutex<()> = Mutex::new(());

/// Exclusive access to the oracle for the current thread (invariant 1).
///
/// The session never changes the thread's floating-point mode: the C++ wrapper turns flush-to-zero on
/// around each call into the vendored code, as the firmware's desktop test harness does, and restores
/// it before returning, so Rust code always runs in its default mode. Taking a second session on a
/// thread that already holds one blocks forever (or panics).
pub struct Session {
    _guard: MutexGuard<'static, ()>,
    _not_send_or_sync: PhantomData<*const Cell<()>>,
}

impl Session {
    /// Waits for the process-wide oracle lock. A poisoned lock (a panic in another session) is
    /// taken anyway: the lock guards no Rust data, and every oracle object of the panicking session
    /// was dropped with it.
    #[must_use]
    pub fn lock() -> Self {
        let guard = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Self { _guard: guard, _not_send_or_sync: PhantomData }
    }

    /// Writes the vendored DSP's random state (`stmlib::Random`).
    pub fn seed(&self, seed: u32) {
        // SAFETY: the lock is held (invariant 1); the call writes one static integer.
        unsafe { oracle_seed(seed) }
    }

    /// Reads the vendored DSP's random state.
    #[must_use]
    pub fn rng_state(&self) -> u32 {
        // SAFETY: the lock is held (invariant 1); the call reads one static integer.
        unsafe { oracle_rng_state() }
    }

    /// The firmware's block size, `kBlockSize` (12 frames).
    #[must_use]
    pub fn block_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_block_size() }
    }

    /// The largest block one render call accepts, `kMaxBlockSize` (24 frames).
    #[must_use]
    pub fn max_block_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_max_block_size() }
    }

    /// The number of engines the wrapper can build (24).
    #[must_use]
    pub fn engine_count(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        usize::try_from(unsafe { oracle_engine_count() }).unwrap_or(0)
    }

    /// The resonator's block size, `kMaxBlockSize` in `rings/dsp/dsp.h` (24 frames).
    #[must_use]
    pub fn resonator_max_block_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_resonator_max_block_size() }
    }

    /// The resonator's largest polyphony, `kMaxPolyphony` (4).
    #[must_use]
    pub fn resonator_max_polyphony(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        usize::try_from(unsafe { oracle_resonator_max_polyphony() }).unwrap_or(0)
    }

    /// The number of resonator models, `RESONATOR_MODEL_LAST` (6).
    #[must_use]
    pub fn resonator_model_count(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        usize::try_from(unsafe { oracle_resonator_model_count() }).unwrap_or(0)
    }

    /// The modal voice's block size, `kMaxBlockSize` in `elements/dsp/dsp.h` (16 frames).
    #[must_use]
    pub fn modal_voice_max_block_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_modal_voice_max_block_size() }
    }

    /// The modal voice's sample rate, `kSampleRate` in `elements/dsp/dsp.h` (32 kHz).
    #[must_use]
    pub fn modal_voice_sample_rate(&self) -> f32 {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_modal_voice_sample_rate() }
    }

    /// The granular processor's block size, `kMaxBlockSize` in `clouds/dsp/frame.h` (32 frames).
    #[must_use]
    pub fn granular_max_block_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_granular_max_block_size() }
    }

    /// The module's two memory blocks, bytes: `block_mem` and `block_ccm` in `clouds/clouds.cc`.
    #[must_use]
    pub fn granular_firmware_sizes(&self) -> (usize, usize) {
        // SAFETY: both return compile-time constants; no state is touched.
        unsafe { (oracle_granular_firmware_large_size(), oracle_granular_firmware_small_size()) }
    }

    /// The texture processor's largest block, the codec's `kMaxCodecBlockSize` (24 frames), and
    /// its delay memory in bits (`kDelayBufferSizeBits`), in its firmware build and its long build.
    #[must_use]
    pub fn texture_constants(&self) -> [(usize, usize); 2] {
        // SAFETY: each returns a compile-time constant; no state is touched.
        unsafe {
            [
                (oracle_texture_max_block_size(), oracle_texture_delay_buffer_size_bits()),
                (oracle_texture_long_max_block_size(), oracle_texture_long_delay_buffer_size_bits()),
            ]
        }
    }

    /// The bytes of FX workspace the processor carves out of its memory (diffuser, reverb,
    /// correlator).
    #[must_use]
    pub fn granular_workspace_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_granular_workspace_size() }
    }

    /// The meta-modulator's constants: the module's codec block (60 frames), `kMaxBlockSize` (96)
    /// and `kOversampling` (6).
    #[must_use]
    pub fn modulator_constants(&self) -> (usize, usize, usize) {
        // SAFETY: each returns a compile-time constant; no state is touched.
        unsafe { (oracle_modulator_codec_block_size(), oracle_modulator_max_block_size(), oracle_modulator_oversampling()) }
    }

    /// The random sequencer's block size and sample rate (`kBlockSize`, `kSampleRate`).
    #[must_use]
    pub fn sequencer_constants(&self) -> (usize, i32) {
        // SAFETY: each returns a compile-time constant; no state is touched.
        unsafe { (oracle_sequencer_block_size(), oracle_sequencer_sample_rate()) }
    }

    /// The wrapper's copy of the firmware's deja vu loop lengths (`loop_length` in
    /// `marbles/marbles.cc`).
    #[must_use]
    pub fn sequencer_loop_lengths(&self) -> Vec<i32> {
        // SAFETY: returns a compile-time constant; no state is touched.
        let n = unsafe { oracle_sequencer_loop_length_count() };
        // SAFETY: reads one entry of a constant table; an index past it returns -1.
        (0..n).map(|i| unsafe { oracle_sequencer_loop_length(i) }).collect()
    }

    /// The wrapper's copy of the firmware's Y clock divisions (`y_divider_ratios`), as `(p, q)`.
    #[must_use]
    pub fn sequencer_y_dividers(&self) -> Vec<(i32, i32)> {
        // SAFETY: returns a compile-time constant; no state is touched.
        let n = unsafe { oracle_sequencer_y_divider_count() };
        (0..n)
            .filter_map(|i| {
                let (mut p, mut q) = (0, 0);
                // SAFETY: `p` and `q` are valid for one write each; the index is checked by the C++.
                let status = unsafe { oracle_sequencer_y_divider(i, &raw mut p, &raw mut q) };
                (status == 0).then_some((p, q))
            })
            .collect()
    }

    /// The wrapper's copy of preset scale `index` (`preset_scales` in `marbles/settings.cc`): base
    /// interval, degree count, and 16 voltages and weights (unused slots zero).
    #[must_use]
    pub fn sequencer_preset_scale(&self, index: usize) -> Option<(f32, i32, [f32; 16], [u8; 16])> {
        let (mut base, mut n, mut v, mut w) = (0.0_f32, 0_i32, [0.0_f32; 16], [0_u8; 16]);
        // SAFETY: every pointer is valid for the writes the C++ makes (one float, one int, 16
        // floats, 16 bytes); the index is checked by the C++.
        let status = unsafe { oracle_sequencer_preset_scale(index, &raw mut base, &raw mut n, v.as_mut_ptr(), w.as_mut_ptr()) };
        (status == 0).then_some((base, n, v, w))
    }
}

/// A complete voice (engine selection, envelopes, LPG, post-processing) on its own 16 KB arena.
pub struct RawVoice<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    session: &'s Session,
}

impl<'s> RawVoice<'s> {
    /// `None` when the sample rate is not 48 kHz (the only rate upstream supports) or the
    /// allocation fails.
    pub(crate) fn new(session: &'s Session, sample_rate: f32) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes a voice, or returns null.
        let ptr = unsafe { oracle_voice_new(sample_rate) };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, session })
    }

    /// Renders `frames.len() / 2` interleaved out/aux frames. `None` when the frame count is odd,
    /// zero (the firmware divides by the block size) or above the maximum block size.
    pub(crate) fn render(&mut self, patch: &CPatch, modulations: &CModulations, frames: &mut [i16]) -> Option<()> {
        let size = frames.len() / 2;
        if !frames.len().is_multiple_of(2) || size == 0 || size > self.session.max_block_size() {
            return None;
        }
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live voice
        // (invariant 2) used by this thread only (`RawVoice` is neither Send nor Sync); `patch`
        // and `modulations` are valid for reads; `frames` holds exactly `2 * size` values with
        // `size <= kMaxBlockSize` (invariant 3).
        let status =
            unsafe { oracle_voice_render(self.ptr.as_ptr(), std::ptr::from_ref(patch), std::ptr::from_ref(modulations), frames.as_mut_ptr(), size) };
        (status == 0).then_some(())
    }
}

impl Drop for RawVoice<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_voice_new` and is freed only here (invariant 2).
        // Freeing touches no shared state, so it needs no lock beyond the borrow.
        unsafe { oracle_voice_free(self.ptr.as_ptr()) }
    }
}

/// One engine on its own 16 KB arena, rendered without the voice's post-processing.
pub struct RawEngine<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    session: &'s Session,
}

impl<'s> RawEngine<'s> {
    /// `None` when `index` is not an engine index (0 to 23) or the allocation fails.
    pub(crate) fn new(session: &'s Session, index: i32) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call returns null for an
        // index outside 0..24 and otherwise allocates and initializes an engine.
        let ptr = unsafe { oracle_engine_new(index) };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, session })
    }

    /// Renders `out.len()` frames of float out and aux; returns the engine's already-enveloped
    /// report. `None` when the lengths differ, are zero (the firmware divides by the block size) or
    /// exceed the maximum block size.
    pub(crate) fn render(&mut self, parameters: &CEngineParameters, out: &mut [f32], aux: &mut [f32]) -> Option<bool> {
        let size = out.len();
        if aux.len() != size || size == 0 || size > self.session.max_block_size() {
            return None;
        }
        let mut enveloped = 0u8;
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live
        // engine (invariant 2) used by this thread only; `parameters` is valid for reads; `out`
        // and `aux` each hold exactly `size <= kMaxBlockSize` floats (invariant 3); `enveloped`
        // is a valid `u8` for the one write.
        let status = unsafe {
            oracle_engine_render(
                self.ptr.as_ptr(),
                std::ptr::from_ref(parameters),
                out.as_mut_ptr(),
                aux.as_mut_ptr(),
                size,
                std::ptr::from_mut(&mut enveloped),
            )
        };
        (status == 0).then_some(enveloped != 0)
    }
}

impl Drop for RawEngine<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_engine_new` and is freed only here (invariant 2),
        // through the holder of its concrete type. Freeing touches no shared state.
        unsafe { oracle_engine_free(self.ptr.as_ptr()) }
    }
}

/// The resonator's part (`rings::Part`) with its reverb memory, in zero-initialized storage as on
/// the module.
pub struct RawResonator<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    session: &'s Session,
}

impl<'s> RawResonator<'s> {
    /// `None` when the allocation fails.
    pub(crate) fn new(session: &'s Session) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes a part, or returns null.
        let ptr = unsafe { oracle_resonator_new() };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, session })
    }

    /// `None` when `model` is not a model index (0 to 5).
    pub(crate) fn set_model(&mut self, model: i32) -> Option<()> {
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live part
        // (invariant 2) used by this thread only; the wrapper range-checks `model`.
        let status = unsafe { oracle_resonator_set_model(self.ptr.as_ptr(), model) };
        (status == 0).then_some(())
    }

    /// `None` when `polyphony` is outside 1 to 4.
    pub(crate) fn set_polyphony(&mut self, polyphony: i32) -> Option<()> {
        // SAFETY: as `set_model`; the wrapper range-checks `polyphony`.
        let status = unsafe { oracle_resonator_set_polyphony(self.ptr.as_ptr(), polyphony) };
        (status == 0).then_some(())
    }

    pub(crate) fn set_bypass(&mut self, bypass: bool) {
        // SAFETY: as `set_model`; the call writes one flag of the part.
        unsafe { oracle_resonator_set_bypass(self.ptr.as_ptr(), u8::from(bypass)) }
    }

    /// Processes `input.len()` frames into `out` and `aux`. `None` when the lengths differ, are zero
    /// or exceed the maximum block size, or an input is out of range (invariant 4).
    pub(crate) fn process(
        &mut self,
        performance: &crate::resonator::PerformanceState,
        patch: &crate::resonator::ResonatorPatch,
        input: &[f32],
        out: &mut [f32],
        aux: &mut [f32],
    ) -> Option<()> {
        let size = input.len();
        if out.len() != size || aux.len() != size || size == 0 || size > self.session.resonator_max_block_size() {
            return None;
        }
        if !crate::resonator::inputs_in_range(performance, patch, input) {
            return None;
        }
        let (performance, patch) = (performance.to_c(), patch.to_c());
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live part
        // (invariant 2) used by this thread only; `performance` and `patch` are valid for reads;
        // `input`, `out` and `aux` each hold exactly `size <= kMaxBlockSize` floats (invariant 3),
        // and `input` does not alias the outputs (they are distinct borrows); every value the C++
        // indexes a table with was range-checked just above (invariant 4).
        let status = unsafe {
            oracle_resonator_process(
                self.ptr.as_ptr(),
                std::ptr::from_ref(&performance),
                std::ptr::from_ref(&patch),
                input.as_ptr(),
                out.as_mut_ptr(),
                aux.as_mut_ptr(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawResonator<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_resonator_new` and is freed only here (invariant 2).
        // Freeing touches no shared state.
        unsafe { oracle_resonator_free(self.ptr.as_ptr()) }
    }
}

/// The modal voice's part (`elements::Part`) with its reverb memory, in zero-initialized storage as
/// on the module.
pub struct RawModalVoice<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    session: &'s Session,
}

impl<'s> RawModalVoice<'s> {
    /// `None` when the allocation fails.
    pub(crate) fn new(session: &'s Session) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes a part, or returns null.
        let ptr = unsafe { oracle_modal_voice_new() };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, session })
    }

    pub(crate) fn seed(&mut self, words: [u32; 3]) {
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live part
        // (invariant 2) used by this thread only; `words` holds the three words the C++ reads.
        unsafe { oracle_modal_voice_seed(self.ptr.as_ptr(), words.as_ptr()) }
    }

    /// `None` when `model` is not a model index (0 to 2).
    pub(crate) fn set_model(&mut self, model: i32) -> Option<()> {
        // SAFETY: as `seed`; the wrapper range-checks `model`.
        let status = unsafe { oracle_modal_voice_set_model(self.ptr.as_ptr(), model) };
        (status == 0).then_some(())
    }

    pub(crate) fn set_easter_egg(&mut self, easter_egg: bool) {
        // SAFETY: as `seed`; the call writes one flag of the part.
        unsafe { oracle_modal_voice_set_easter_egg(self.ptr.as_ptr(), u8::from(easter_egg)) }
    }

    pub(crate) fn set_bypass(&mut self, bypass: bool) {
        // SAFETY: as `seed`; the call writes one flag of the part.
        unsafe { oracle_modal_voice_set_bypass(self.ptr.as_ptr(), u8::from(bypass)) }
    }

    /// Processes `blow.len()` frames into `main` and `aux`. `None` when the lengths differ, are zero
    /// or exceed the maximum block size, or an input is out of range (invariant 4).
    pub(crate) fn process(
        &mut self,
        performance: &crate::modal_voice::Performance,
        patch: &crate::modal_voice::ModalVoicePatch,
        blow: &[f32],
        strike: &[f32],
        main: &mut [f32],
        aux: &mut [f32],
    ) -> Option<()> {
        let size = blow.len();
        let lengths_match = strike.len() == size && main.len() == size && aux.len() == size;
        if !lengths_match || size == 0 || size > self.session.modal_voice_max_block_size() {
            return None;
        }
        if !crate::modal_voice::inputs_in_range(performance, patch, blow, strike) {
            return None;
        }
        let (performance, patch) = (performance.to_c(), patch.to_c());
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live part
        // (invariant 2) used by this thread only; `performance` and `patch` are valid for reads;
        // `blow`, `strike`, `main` and `aux` each hold exactly `size <= kMaxBlockSize` floats
        // (invariant 3), and the inputs do not alias the outputs (distinct borrows); every value the
        // C++ indexes a table with or converts to an integer was range-checked just above
        // (invariant 4).
        let status = unsafe {
            oracle_modal_voice_process(
                self.ptr.as_ptr(),
                std::ptr::from_ref(&performance),
                std::ptr::from_ref(&patch),
                blow.as_ptr(),
                strike.as_ptr(),
                main.as_mut_ptr(),
                aux.as_mut_ptr(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawModalVoice<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_modal_voice_new` and is freed only here (invariant 2).
        // Freeing touches no shared state.
        unsafe { oracle_modal_voice_free(self.ptr.as_ptr()) }
    }
}

/// The granular processor (`clouds::GranularProcessor`) with its two memory blocks, in
/// zero-initialized storage as on the module.
pub struct RawGranular<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    session: &'s Session,
}

impl<'s> RawGranular<'s> {
    /// `None` when the sizes leave no room for the workspace or the longest grain, the quality is
    /// outside 0 to 3, the mode outside 0 to 1, or an allocation fails.
    pub(crate) fn new(session: &'s Session, large: usize, small: usize, quality: i32, mode: i32) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call checks the sizes, the
        // quality and the mode, then allocates and initializes a processor, or returns null.
        let ptr = unsafe { oracle_granular_new(large, small, quality, mode) };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, session })
    }

    /// `None` when `quality` is outside 0 to 3.
    pub(crate) fn set_quality(&mut self, quality: i32) -> Option<()> {
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live
        // processor (invariant 2) used by this thread only; the wrapper range-checks `quality`.
        let status = unsafe { oracle_granular_set_quality(self.ptr.as_ptr(), quality) };
        (status == 0).then_some(())
    }

    /// `None` when `mode` is outside 0 to 1.
    pub(crate) fn set_mode(&mut self, mode: i32) -> Option<()> {
        // SAFETY: as `set_quality`; the wrapper range-checks `mode`.
        let status = unsafe { oracle_granular_set_mode(self.ptr.as_ptr(), mode) };
        (status == 0).then_some(())
    }

    pub(crate) fn set_bypass(&mut self, bypass: bool) {
        // SAFETY: as `set_quality`; the call writes one flag of the processor.
        unsafe { oracle_granular_set_bypass(self.ptr.as_ptr(), u8::from(bypass)) }
    }

    pub(crate) fn set_silence(&mut self, silence: bool) {
        // SAFETY: as `set_quality`; the call writes one flag of the processor.
        unsafe { oracle_granular_set_silence(self.ptr.as_ptr(), u8::from(silence)) }
    }

    /// Processes `input.len()` stereo frames into `out`. `None` when the lengths differ, the frame
    /// count is zero, exceeds the maximum block size or is odd at a low-fidelity quality (the C++
    /// checks the last), or a parameter is out of range (invariant 4).
    pub(crate) fn process(&mut self, parameters: &crate::granular::Parameters, input: &[[i16; 2]], out: &mut [[i16; 2]]) -> Option<()> {
        let size = input.len();
        if out.len() != size || size == 0 || size > self.session.granular_max_block_size() {
            return None;
        }
        if !crate::granular::parameters_in_range(parameters) {
            return None;
        }
        let parameters = parameters.to_c();
        // SAFETY: the lock is held through `self.session` (invariant 1); `self.ptr` is a live
        // processor (invariant 2) used by this thread only; `parameters` is valid for reads;
        // `input` and `out` each hold exactly `2 * size` contiguous `i16` values (`[i16; 2]` has
        // no padding) with `size <= kMaxBlockSize` (invariant 3), and they do not alias (distinct
        // borrows); every value the C++ indexes a table with was range-checked just above
        // (invariant 4).
        let status = unsafe {
            oracle_granular_process(
                self.ptr.as_ptr(),
                std::ptr::from_ref(&parameters),
                input.as_ptr().cast::<i16>(),
                out.as_mut_ptr().cast::<i16>(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawGranular<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_granular_new` and is freed only here (invariant 2).
        // Freeing touches no shared state.
        unsafe { oracle_granular_free(self.ptr.as_ptr()) }
    }
}

/// Which build of the texture processor a [`RawTexture`] belongs to (`cpp/texture_oracle.cc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextureBuild {
    /// The module's buffer.
    Firmware,
    /// The port's longest buffer, in a renamed namespace.
    Long,
}

/// The texture processor's effect chain (`beads::Processor`) with its delay and reverb memory, in
/// zero-initialized storage as on the module, in one of its two builds.
pub struct RawTexture<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    build: TextureBuild,
    /// Ties the handle to its session: every call happens while the lock is held (invariant 1).
    _session: &'s Session,
}

impl<'s> RawTexture<'s> {
    /// `None` when `media` is outside 0 to 3 or an allocation fails.
    pub(crate) fn new(session: &'s Session, build: TextureBuild, media: i32, mono: bool) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call checks the media, then
        // allocates and initializes a processor, or returns null.
        let ptr = unsafe {
            match build {
                TextureBuild::Firmware => oracle_texture_new(media, u8::from(mono)),
                TextureBuild::Long => oracle_texture_long_new(media, u8::from(mono)),
            }
        };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, build, _session: session })
    }

    /// `None` when `media` is outside 0 to 3.
    pub(crate) fn configure(&mut self, media: i32, mono: bool) -> Option<()> {
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live
        // processor of this build (invariant 2) used by this thread only; the wrapper range-checks
        // `media`.
        let status = unsafe {
            match self.build {
                TextureBuild::Firmware => oracle_texture_configure(self.ptr.as_ptr(), media, u8::from(mono)),
                TextureBuild::Long => oracle_texture_long_configure(self.ptr.as_ptr(), media, u8::from(mono)),
            }
        };
        (status == 0).then_some(())
    }

    pub(crate) fn trim(&mut self) {
        // SAFETY: as `configure`; the call writes two integers of each delay line it trims.
        unsafe {
            match self.build {
                TextureBuild::Firmware => oracle_texture_trim(self.ptr.as_ptr()),
                TextureBuild::Long => oracle_texture_long_trim(self.ptr.as_ptr()),
            }
        }
    }

    pub(crate) fn set_flush(&mut self, flush: bool) {
        // SAFETY: as `configure`; the call writes one flag of the holder.
        unsafe {
            match self.build {
                TextureBuild::Firmware => oracle_texture_set_flush(self.ptr.as_ptr(), u8::from(flush)),
                TextureBuild::Long => oracle_texture_long_set_flush(self.ptr.as_ptr(), u8::from(flush)),
            }
        }
    }

    /// Processes `input.len()` stereo frames into `out`. `None` when the lengths differ, the frame
    /// count is zero, exceeds the maximum block size or is odd at the clean-tape quality (the C++
    /// checks the last), or an input is out of range (invariant 4).
    pub(crate) fn process(
        &mut self,
        controls: &crate::texture::Controls,
        freeze: &[bool],
        seed: Option<&[u8]>,
        input: &[[f32; 2]],
        out: &mut [[f32; 2]],
    ) -> Option<()> {
        let size = input.len();
        if out.len() != size
            || freeze.len() != size
            || seed.is_some_and(|s| s.len() != size)
            || size == 0
            || size > crate::texture::TEXTURE_MAX_BLOCK_SIZE
        {
            return None;
        }
        if !crate::texture::controls_in_range(controls) || !crate::texture::input_in_range(input) {
            return None;
        }
        let c = controls.to_c();
        let freeze: [u8; crate::texture::TEXTURE_MAX_BLOCK_SIZE] = std::array::from_fn(|i| u8::from(freeze.get(i).copied().unwrap_or(false)));
        let seed_ptr = seed.map_or(std::ptr::null(), <[u8]>::as_ptr);
        let f = match self.build {
            TextureBuild::Firmware => oracle_texture_process,
            TextureBuild::Long => oracle_texture_long_process,
        };
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live
        // processor of this build (invariant 2) used by this thread only; `c` is valid for reads;
        // `freeze` holds at least `size` bytes, `seed` is null or holds exactly `size` bytes, and
        // `input` and `out` each hold exactly `2 * size` contiguous floats (`[f32; 2]` has no
        // padding) with `size <= kMaxBlockSize` (invariant 3); they do not alias (distinct
        // borrows); every input the C++ indexes a table with or converts to an integer was
        // range-checked just above (invariant 4).
        let status = unsafe {
            f(
                self.ptr.as_ptr(),
                std::ptr::from_ref(&c),
                freeze.as_ptr(),
                seed_ptr,
                input.as_ptr().cast::<f32>(),
                out.as_mut_ptr().cast::<f32>(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawTexture<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from the `_new` function of the same build and is freed only
        // here (invariant 2). Freeing touches no shared state.
        unsafe {
            match self.build {
                TextureBuild::Firmware => oracle_texture_free(self.ptr.as_ptr()),
                TextureBuild::Long => oracle_texture_long_free(self.ptr.as_ptr()),
            }
        }
    }
}

/// The texture processor's synth (`beads::Synth`) with its own reverb memory, zero-initialized.
pub struct RawTextureSynth<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    /// Ties the handle to its session (invariant 1).
    _session: &'s Session,
}

impl<'s> RawTextureSynth<'s> {
    /// `None` when the allocation fails.
    pub(crate) fn new(session: &'s Session) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes a synth, or returns null.
        let ptr = unsafe { oracle_texture_synth_new() };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, _session: session })
    }

    /// Renders `out.len()` stereo frames. `None` when the lengths differ, the frame count is zero or
    /// exceeds the maximum block size, the resolution is outside 0 to 3, or a control is out of
    /// range (invariant 4).
    pub(crate) fn render(
        &mut self,
        controls: &crate::texture::Controls,
        resolution: i32,
        freeze: &[bool],
        seed: Option<&[u8]>,
        out: &mut [[f32; 2]],
    ) -> Option<()> {
        let size = out.len();
        if freeze.len() != size || seed.is_some_and(|s| s.len() != size) || size == 0 || size > crate::texture::TEXTURE_MAX_BLOCK_SIZE {
            return None;
        }
        if !crate::texture::controls_in_range(controls) {
            return None;
        }
        let c = controls.to_c();
        let freeze: [u8; crate::texture::TEXTURE_MAX_BLOCK_SIZE] = std::array::from_fn(|i| u8::from(freeze.get(i).copied().unwrap_or(false)));
        let seed_ptr = seed.map_or(std::ptr::null(), <[u8]>::as_ptr);
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live synth
        // (invariant 2) used by this thread only; `c` is valid for reads; `freeze` holds at least
        // `size` bytes, `seed` is null or holds exactly `size`, `out` holds exactly `2 * size`
        // floats with `size <= kMaxBlockSize` (invariant 3); the C++ range-checks `resolution`;
        // the controls were range-checked just above (invariant 4).
        let status = unsafe {
            oracle_texture_synth_render(
                self.ptr.as_ptr(),
                std::ptr::from_ref(&c),
                resolution,
                freeze.as_ptr(),
                seed_ptr,
                out.as_mut_ptr().cast::<f32>(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawTextureSynth<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_texture_synth_new` and is freed only here
        // (invariant 2). Freeing touches no shared state.
        unsafe { oracle_texture_synth_free(self.ptr.as_ptr()) }
    }
}

/// The texture processor's input stage (`beads::GainControl`), zero-initialized.
pub struct RawTextureInput<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    /// Ties the handle to its session (invariant 1).
    _session: &'s Session,
}

impl<'s> RawTextureInput<'s> {
    /// `None` when the mode is outside 0 to 2, the sample rate is not one of the codec's, or the
    /// allocation fails.
    pub(crate) fn new(session: &'s Session, sample_rate: f32, mode: i32) -> Option<Self> {
        if !crate::texture::input_sample_rate_supported(sample_rate) {
            return None;
        }
        // SAFETY: the lock is held through `session` (invariant 1); the call checks the mode, then
        // allocates and initializes the stage, or returns null.
        let ptr = unsafe { oracle_texture_input_new(sample_rate, mode) };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, _session: session })
    }

    /// `None` when the mode is outside 0 to 2 or the sample rate is not one of the codec's.
    pub(crate) fn init(&mut self, sample_rate: f32, mode: i32, reset: bool) -> Option<()> {
        if !crate::texture::input_sample_rate_supported(sample_rate) {
            return None;
        }
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live stage
        // (invariant 2) used by this thread only; the C++ range-checks `mode`.
        let status = unsafe { oracle_texture_input_init(self.ptr.as_ptr(), sample_rate, mode, u8::from(reset)) };
        (status == 0).then_some(())
    }

    /// Converts `input.len()` raw frames into `out`. `None` when the lengths differ or the frame
    /// count is zero or exceeds the maximum block size.
    pub(crate) fn process(&mut self, active: bool, gain: u8, normalized: [bool; 2], input: &[[i32; 2]], out: &mut [[f32; 2]]) -> Option<()> {
        let size = input.len();
        if out.len() != size || size == 0 || size > crate::texture::TEXTURE_MAX_BLOCK_SIZE {
            return None;
        }
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live stage
        // (invariant 2) used by this thread only; `input` holds exactly `2 * size` contiguous
        // `i32` and `out` exactly `2 * size` floats, `size <= kMaxBlockSize` (invariant 3); every
        // `i32` and every gain setting is a valid input (no table is indexed, no float is
        // converted to an integer).
        let status = unsafe {
            oracle_texture_input_process(
                self.ptr.as_ptr(),
                u8::from(active),
                gain,
                u8::from(normalized[0]),
                u8::from(normalized[1]),
                input.as_ptr().cast::<i32>(),
                out.as_mut_ptr().cast::<f32>(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawTextureInput<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_texture_input_new` and is freed only here
        // (invariant 2). Freeing touches no shared state.
        unsafe { oracle_texture_input_free(self.ptr.as_ptr()) }
    }
}

/// The meta-modulator (`warps::Modulator`), zero-initialized as on the module.
pub struct RawModulator<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    /// Ties the handle to its session (invariant 1).
    _session: &'s Session,
}

impl<'s> RawModulator<'s> {
    /// `None` when the allocation fails.
    pub(crate) fn new(session: &'s Session) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes a modulator, or returns null.
        let ptr = unsafe { oracle_modulator_new() };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, _session: session })
    }

    pub(crate) fn set_flush(&mut self, flush: bool) {
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live
        // modulator (invariant 2) used by this thread only; the call writes one flag.
        unsafe { oracle_modulator_set_flush(self.ptr.as_ptr(), u8::from(flush)) }
    }

    /// Processes `input.len()` stereo frames into `out`. `None` when the lengths differ, the frame
    /// count is not a multiple of 12 from 12 to 60, or a parameter is out of range (the C++ checks
    /// both, invariants 3 and 4).
    pub(crate) fn process(&mut self, parameters: &CModulatorParameters, input: &[[i16; 2]], out: &mut [[i16; 2]]) -> Option<()> {
        let size = input.len();
        if out.len() != size || size == 0 || size > crate::modulator::MODULATOR_BLOCK_SIZE {
            return None;
        }
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is a live
        // modulator (invariant 2) used by this thread only; `parameters` is valid for reads;
        // `input` and `out` each hold exactly `2 * size` contiguous `i16` (`[i16; 2]` has no
        // padding), `size` is at most the 60 frames the wrapper's copies hold and the wrapper
        // refuses anything but whole groups of 12 (invariant 3); the wrapper range-checks every
        // parameter it indexes a table with, and every input sample is an int16, which keeps every
        // table read of the processing path inside its table (patches/all/0010 clamps the two
        // whose index comes from the processed signal) (invariant 4).
        let status = unsafe {
            oracle_modulator_process(
                self.ptr.as_ptr(),
                std::ptr::from_ref(parameters),
                input.as_ptr().cast::<i16>(),
                out.as_mut_ptr().cast::<i16>(),
                size,
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawModulator<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_modulator_new` and is freed only here
        // (invariant 2). Freeing touches no shared state.
        unsafe { oracle_modulator_free(self.ptr.as_ptr()) }
    }
}

/// The random sequencer's generators (`marbles::TGenerator`, `marbles::XYGenerator` and their own
/// random generator), driven as the firmware's main loop drives them.
pub struct RawSequencer<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    /// Ties the handle to its session (invariant 1).
    _session: &'s Session,
}

impl<'s> RawSequencer<'s> {
    /// `None` when the allocation fails.
    pub(crate) fn new(session: &'s Session, seed: u32) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); the call allocates and
        // initializes the generators, or returns null.
        let ptr = unsafe { oracle_sequencer_new(seed) };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, _session: session })
    }

    pub(crate) fn set_flush(&mut self, flush: bool) {
        // SAFETY: the lock is held through `self._session` (invariant 1); `self.ptr` is live
        // (invariant 2) and used by this thread only; the call writes one flag.
        unsafe { oracle_sequencer_set_flush(self.ptr.as_ptr(), u8::from(flush)) }
    }

    /// Loads a scale into slot `index`. `None` when the C++ refuses it (it checks every argument).
    pub(crate) fn load_scale(&mut self, index: usize, base_interval: f32, voltages: &[f32], weights: &[u8]) -> Option<()> {
        let n = voltages.len();
        if weights.len() != n || n == 0 || n > 16 {
            return None;
        }
        let n = i32::try_from(n).ok()?;
        // SAFETY: the lock is held (invariant 1); `self.ptr` is live (invariant 2); `voltages` and
        // `weights` each hold `n` (1 to 16) readable values (invariant 3); the C++ checks the slot,
        // the interval and every degree before it touches the quantizers (invariant 4).
        let status = unsafe { oracle_sequencer_load_scale(self.ptr.as_ptr(), index, base_interval, n, voltages.as_ptr(), weights.as_ptr()) };
        (status == 0).then_some(())
    }

    /// One block of `gates.len()` frames (1 to 5). `None` when the lengths disagree, a clock the
    /// controls need is missing, or the C++ refuses a control.
    pub(crate) fn process(
        &mut self,
        controls: &CSequencerControls,
        t_clock: Option<&[u8]>,
        xy_clock: Option<&[u8]>,
        gates: &mut [[u8; 3]],
        voltages: &mut [[f32; 4]],
    ) -> Option<()> {
        let size = gates.len();
        if voltages.len() != size || size == 0 || size > crate::sequencer::SEQUENCER_BLOCK_SIZE {
            return None;
        }
        if t_clock.is_some_and(|c| c.len() != size) || xy_clock.is_some_and(|c| c.len() != size) {
            return None;
        }
        let t = t_clock.map_or(std::ptr::null(), <[u8]>::as_ptr);
        let xy = xy_clock.map_or(std::ptr::null(), <[u8]>::as_ptr);
        // SAFETY: the lock is held (invariant 1); `self.ptr` is live (invariant 2) and used by this
        // thread only; `controls` is valid for reads; each clock is null or holds `size` bytes, and
        // `gates` and `voltages` hold `3 * size` bytes and `4 * size` floats (`[u8; 3]` and
        // `[f32; 4]` have no padding), `size` at most the 5 frames the wrapper's buffers hold
        // (invariant 3); the C++ range-checks every control and clock byte (invariant 4).
        let status = unsafe {
            oracle_sequencer_process(
                self.ptr.as_ptr(),
                std::ptr::from_ref(controls),
                t,
                xy,
                size,
                gates.as_mut_ptr().cast::<u8>(),
                voltages.as_mut_ptr().cast::<f32>(),
            )
        };
        (status == 0).then_some(())
    }
}

impl Drop for RawSequencer<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_sequencer_new` and is freed only here (invariant 2).
        unsafe { oracle_sequencer_free(self.ptr.as_ptr()) }
    }
}

/// The random sequencer's scale recorder (`marbles::ScaleRecorder`).
pub struct RawRecorder<'s> {
    ptr: std::ptr::NonNull<std::ffi::c_void>,
    /// Ties the handle to its session (invariant 1).
    _session: &'s Session,
}

impl<'s> RawRecorder<'s> {
    pub(crate) fn new(session: &'s Session) -> Option<Self> {
        // SAFETY: the lock is held through `session` (invariant 1); allocates or returns null.
        let ptr = unsafe { oracle_sequencer_recorder_new() };
        std::ptr::NonNull::new(ptr).map(|ptr| Self { ptr, _session: session })
    }

    /// One event (0 new note, 1 update, 2 accept). `None` when the C++ refuses it.
    pub(crate) fn event(&mut self, kind: i32, voltage: f32) -> Option<()> {
        // SAFETY: the lock is held (invariant 1); `self.ptr` is live (invariant 2); the C++
        // checks the kind and the voltage.
        let status = unsafe { oracle_sequencer_recorder_event(self.ptr.as_ptr(), kind, voltage) };
        (status == 0).then_some(())
    }

    /// The recorded scale, or `None` below two degrees.
    pub(crate) fn extract(&mut self) -> Option<(f32, i32, [f32; 16], [u8; 16])> {
        let (mut base, mut n, mut v, mut w) = (0.0_f32, 0_i32, [0.0_f32; 16], [0_u8; 16]);
        // SAFETY: the lock is held (invariant 1); `self.ptr` is live (invariant 2); every pointer is
        // valid for the writes the C++ makes (one float, one int, 16 floats, 16 bytes).
        let status = unsafe { oracle_sequencer_recorder_extract(self.ptr.as_ptr(), &raw mut base, &raw mut n, v.as_mut_ptr(), w.as_mut_ptr()) };
        (status == 1).then_some((base, n, v, w))
    }
}

impl Drop for RawRecorder<'_> {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` came from `oracle_sequencer_recorder_new` and is freed only here
        // (invariant 2).
        unsafe { oracle_sequencer_recorder_free(self.ptr.as_ptr()) }
    }
}

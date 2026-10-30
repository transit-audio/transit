//! The only module with `unsafe`: the C ABI of `cpp/oracle.cc`, `cpp/resonator_oracle.cc` and
//! `cpp/granular_oracle.cc`, and the session lock that makes it sound to call.
//!
//! Soundness rests on three invariants, each enforced here:
//!
//! 1. **One caller at a time.** The vendored DSP keeps its random state in one process-wide static
//!    (`stmlib::Random`), so two threads rendering at once would race on it (undefined behavior in
//!    C++) and would also make the noise sequence depend on scheduling. Every call goes through a
//!    [`Session`], which holds a process-wide mutex and is neither `Send` nor `Sync`, so all calls of
//!    one session stay on the thread that took the lock. Handles borrow their session.
//! 2. **Valid handles.** [`RawVoice`], [`RawEngine`], [`RawResonator`] and [`RawGranular`] own a non-null pointer
//!    returned by the matching `_new` function and free it exactly once, in `Drop`.
//! 3. **Buffer bounds.** Every render checks the length is between 1 and the firmware's maximum block
//!    size before the call, and passes buffers whose lengths match the size it passes.
//! 4. **Table bounds.** Inputs the C++ indexes tables with are range-checked here before the call:
//!    the resonator's tonic, note, FM, chord, macros and input samples (`crate::resonator::inputs_in_range`, which
//!    derives its bounds from the C++), and the granular processor's parameters
//!    (`crate::granular::parameters_in_range`), so no input from safe Rust can make it read past a
//!    table. The granular processor's memory sizes and quality are checked by the C++ itself (it
//!    returns null for sizes that leave no room for its workspace or its longest grain).

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

// SAFETY: these declarations match the `extern "C"` definitions in cpp/oracle.cc,
// cpp/resonator_oracle.cc and cpp/granular_oracle.cc one for one
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

    /// The bytes of FX workspace the processor carves out of its memory (diffuser, reverb,
    /// correlator).
    #[must_use]
    pub fn granular_workspace_size(&self) -> usize {
        // SAFETY: returns a compile-time constant; no state is touched.
        unsafe { oracle_granular_workspace_size() }
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

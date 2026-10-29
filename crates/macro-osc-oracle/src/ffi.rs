//! The only module with `unsafe`: the C ABI of `cpp/oracle.cc` and the session lock that makes it
//! sound to call.
//!
//! Soundness rests on three invariants, each enforced here:
//!
//! 1. **One caller at a time.** The vendored DSP keeps its random state in one process-wide static
//!    (`stmlib::Random`), so two threads rendering at once would race on it (undefined behavior in
//!    C++) and would also make the noise sequence depend on scheduling. Every call goes through a
//!    [`Session`], which holds a process-wide mutex and is neither `Send` nor `Sync`, so all calls of
//!    one session stay on the thread that took the lock. Handles borrow their session.
//! 2. **Valid handles.** [`RawVoice`] and [`RawEngine`] own a non-null pointer returned by the
//!    matching `_new` function and free it exactly once, in `Drop`.
//! 3. **Buffer bounds.** Every render checks the length against the firmware's maximum block size
//!    before the call, and passes buffers whose lengths match the size it passes.

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

// SAFETY: these declarations match the `extern "C"` definitions in cpp/oracle.cc one for one
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
    fn oracle_fp_enter() -> u64;
    fn oracle_fp_restore(previous: u64);
}

static LOCK: Mutex<()> = Mutex::new(());

/// Exclusive access to the oracle for the current thread (invariant 1).
///
/// While a session is alive the thread runs with flush-to-zero on, as the firmware's desktop test
/// harness does, so a port rendered inside the same session sees the same floating-point mode. The
/// previous mode is restored on drop. Taking a second session on a thread that already holds one
/// blocks forever (or panics).
pub struct Session {
    previous_fp: u64,
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
        // SAFETY: the lock is held (invariant 1); the call only reads and writes this thread's
        // floating-point control register.
        let previous_fp = unsafe { oracle_fp_enter() };
        Self { previous_fp, _guard: guard, _not_send_or_sync: PhantomData }
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
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the lock is still held (the guard drops after this body); the call restores this
        // thread's floating-point control register to the value read in `lock`.
        unsafe { oracle_fp_restore(self.previous_fp) }
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

    /// Renders `frames.len() / 2` interleaved out/aux frames. `None` when the frame count is odd
    /// or above the maximum block size.
    pub(crate) fn render(&mut self, patch: &CPatch, modulations: &CModulations, frames: &mut [i16]) -> Option<()> {
        let size = frames.len() / 2;
        if !frames.len().is_multiple_of(2) || size > self.session.max_block_size() {
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
    /// report. `None` when the lengths differ or exceed the maximum block size.
    pub(crate) fn render(&mut self, parameters: &CEngineParameters, out: &mut [f32], aux: &mut [f32]) -> Option<bool> {
        let size = out.len();
        if aux.len() != size || size > self.session.max_block_size() {
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

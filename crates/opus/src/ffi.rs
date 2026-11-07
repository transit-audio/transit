//! The only module with `unsafe`: the C ABI of the vendored libopus (vendor/include/opus.h and
//! opus_defines.h) and the owned handles that make it sound to call.
//!
//! Soundness rests on four invariants, each enforced here:
//!
//! 1. **Valid handles.** [`RawEncoder`] and [`RawDecoder`] own a non-null pointer returned by
//!    `opus_encoder_create` / `opus_decoder_create` with `OPUS_OK` and free it exactly once, in
//!    `Drop`, with the matching `_destroy` function.
//! 2. **One thread at a time.** opus.h: "A single codec state may only be accessed from a single
//!    thread at a time and any required locking must be performed by the caller." Every call
//!    takes `&mut self`, so the borrow checker gives one caller at a time; the handles are `Send`
//!    (a state may move between threads) and never `Sync`.
//! 3. **Buffer bounds.** Every encode passes exactly `frame_size * channels` input samples and an
//!    output buffer whose length is the `max_data_bytes` it passes; every decode passes an output
//!    buffer of exactly `frame_size * channels` samples for the `frame_size` it passes. The
//!    lengths are checked before the call.
//! 4. **CTL argument types.** Each `opus_encoder_ctl` call passes the argument type its request
//!    macro in opus_defines.h passes: `opus_int32` for the setters (`opus_check_int`), an
//!    `opus_int32*` to a live local for `OPUS_GET_LOOKAHEAD` (`opus_check_int_ptr`).

use std::ffi::{CStr, c_char, c_int, c_uchar};
use std::ptr::NonNull;

/// `OpusEncoder` (opaque).
#[repr(C)]
pub(crate) struct OpusEncoder {
    _private: [u8; 0],
}

/// `OpusDecoder` (opaque).
#[repr(C)]
pub(crate) struct OpusDecoder {
    _private: [u8; 0],
}

/// `OPUS_OK`.
pub(crate) const OPUS_OK: c_int = 0;
/// `OPUS_APPLICATION_AUDIO`.
pub(crate) const OPUS_APPLICATION_AUDIO: c_int = 2049;
/// `OPUS_SET_BITRATE_REQUEST`.
pub(crate) const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
/// `OPUS_SET_VBR_REQUEST`.
pub(crate) const OPUS_SET_VBR_REQUEST: c_int = 4006;
/// `OPUS_SET_COMPLEXITY_REQUEST`.
pub(crate) const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;
/// `OPUS_SET_VBR_CONSTRAINT_REQUEST`.
pub(crate) const OPUS_SET_VBR_CONSTRAINT_REQUEST: c_int = 4020;
/// `OPUS_SET_SIGNAL_REQUEST`.
pub(crate) const OPUS_SET_SIGNAL_REQUEST: c_int = 4024;
/// `OPUS_GET_LOOKAHEAD_REQUEST`.
pub(crate) const OPUS_GET_LOOKAHEAD_REQUEST: c_int = 4027;
/// `OPUS_SIGNAL_MUSIC`.
pub(crate) const OPUS_SIGNAL_MUSIC: c_int = 3002;

// SAFETY: these declarations match vendor/include/opus.h and opus_defines.h one for one (names,
// argument order, and C types: opus_types.h typedefs `opus_int32` as `int32_t` (or `int`), which is
// `c_int` on every target transit builds for; `unsigned char*` is `*mut c_uchar`). The build script
// compiles the matching definitions into this crate.
unsafe extern "C" {
    fn opus_encoder_create(fs: c_int, channels: c_int, application: c_int, error: *mut c_int) -> *mut OpusEncoder;
    fn opus_encode_float(st: *mut OpusEncoder, pcm: *const f32, frame_size: c_int, data: *mut c_uchar, max_data_bytes: c_int) -> c_int;
    fn opus_encoder_destroy(st: *mut OpusEncoder);
    fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    fn opus_decoder_create(fs: c_int, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decode_float(st: *mut OpusDecoder, data: *const c_uchar, len: c_int, pcm: *mut f32, frame_size: c_int, decode_fec: c_int) -> c_int;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
    fn opus_strerror(error: c_int) -> *const c_char;
    fn opus_get_version_string() -> *const c_char;
}

/// A static C string from libopus as an owned `String`.
fn owned(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: opus_defines.h documents `opus_strerror` and `opus_get_version_string` as returning
    // "Error string" / "Version string": pointers to string literals in the library (see
    // celt/celt.c), NUL-terminated and valid for the life of the process; null is excluded above.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// libopus's message for an error code (`opus_strerror`).
pub(crate) fn strerror(code: c_int) -> String {
    // SAFETY: opus_defines.h: "Converts an opus error code into a human readable string": any
    // `int` is accepted (unknown codes map to "unknown error"); no state is touched.
    owned(unsafe { opus_strerror(code) })
}

/// The library's version string (`opus_get_version_string`), e.g. `libopus 1.6.1`.
pub(crate) fn version_string() -> String {
    // SAFETY: opus_defines.h: "Gets the libopus version string." Takes no arguments and touches
    // no state.
    owned(unsafe { opus_get_version_string() })
}

/// An owned `OpusEncoder*` (invariant 1).
pub(crate) struct RawEncoder {
    st: NonNull<OpusEncoder>,
    channels: usize,
}

// SAFETY: opus.h: "The encoder state is always continuous in memory and only a shallow copy is
// sufficient to copy it": the state holds no pointer to thread-local data, so it may be used from
// another thread than the one that created it. Access stays exclusive through `&mut self`
// (invariant 2); `RawEncoder` is not `Sync`.
unsafe impl Send for RawEncoder {}

impl RawEncoder {
    /// `opus_encoder_create(fs, channels, OPUS_APPLICATION_AUDIO, &error)`. `fs` must be one of
    /// 8000, 12000, 16000, 24000 or 48000 and `channels` 1 or 2 (libopus refuses anything else
    /// with `OPUS_BAD_ARG`).
    pub(crate) fn new(fs: i32, channels: usize) -> Result<Self, c_int> {
        let ch = c_int::try_from(channels).map_err(|_| -1)?;
        let mut error: c_int = OPUS_OK;
        // SAFETY: opus.h `opus_encoder_create`: `error` is an `int*` out-parameter, here a live
        // local; the arguments are plain integers that libopus validates itself ("Fs ... must be
        // one of 8000, 12000, 16000, 24000, or 48000", "channels ... (1 or 2)", returning null and
        // an error code otherwise).
        let st = unsafe { opus_encoder_create(fs, ch, OPUS_APPLICATION_AUDIO, &raw mut error) };
        match NonNull::new(st) {
            Some(st) if error == OPUS_OK => Ok(RawEncoder { st, channels }),
            Some(st) => {
                // SAFETY: a non-null state from opus_encoder_create is freed once, here (invariant 1).
                unsafe { opus_encoder_destroy(st.as_ptr()) };
                Err(error)
            }
            None => Err(if error == OPUS_OK { -7 } else { error }),
        }
    }

    /// `opus_encoder_ctl(st, request, value)` for a setter whose macro takes an `opus_int32`.
    pub(crate) fn set(&mut self, request: c_int, value: i32) -> Result<(), c_int> {
        debug_assert!(matches!(
            request,
            OPUS_SET_BITRATE_REQUEST | OPUS_SET_VBR_REQUEST | OPUS_SET_COMPLEXITY_REQUEST | OPUS_SET_VBR_CONSTRAINT_REQUEST | OPUS_SET_SIGNAL_REQUEST
        ));
        if !matches!(
            request,
            OPUS_SET_BITRATE_REQUEST | OPUS_SET_VBR_REQUEST | OPUS_SET_COMPLEXITY_REQUEST | OPUS_SET_VBR_CONSTRAINT_REQUEST | OPUS_SET_SIGNAL_REQUEST
        ) {
            return Err(-1);
        }
        // SAFETY: the state is valid (invariant 1) and exclusively borrowed (invariant 2); each
        // request accepted above is a setter whose opus_defines.h macro passes exactly one
        // `opus_int32` (`opus_check_int(x)`), which `value` is (invariant 4).
        let r = unsafe { opus_encoder_ctl(self.st.as_ptr(), request, value as c_int) };
        if r == OPUS_OK { Ok(()) } else { Err(r) }
    }

    /// `OPUS_GET_LOOKAHEAD`: the samples (at 48 kHz here) the decoder output lags the input by.
    pub(crate) fn lookahead(&mut self) -> Result<i32, c_int> {
        let mut value: c_int = 0;
        // SAFETY: the state is valid and exclusively borrowed (invariants 1, 2);
        // `OPUS_GET_LOOKAHEAD(x)` passes one `opus_int32*` (`opus_check_int_ptr(x)`), here a live
        // local libopus writes once before returning (invariant 4).
        let r = unsafe { opus_encoder_ctl(self.st.as_ptr(), OPUS_GET_LOOKAHEAD_REQUEST, &raw mut value) };
        if r == OPUS_OK { Ok(value) } else { Err(r) }
    }

    /// `opus_encode_float`: one frame of `frame_size` samples per channel (interleaved in `pcm`)
    /// into `out`. Returns the packet length.
    pub(crate) fn encode(&mut self, pcm: &[f32], frame_size: usize, out: &mut [u8]) -> Result<usize, c_int> {
        if frame_size == 0 || pcm.len() != frame_size.saturating_mul(self.channels) || out.is_empty() {
            return Err(-1);
        }
        let fs = c_int::try_from(frame_size).map_err(|_| -1)?;
        let max = c_int::try_from(out.len()).map_err(|_| -1)?;
        // SAFETY: the state is valid and exclusively borrowed (invariants 1, 2). opus.h
        // `opus_encode_float`: "pcm ... length is frame_size*channels*sizeof(float)", checked
        // above; "data ... must contain storage for at least max_data_bytes", which is `out.len()`
        // (invariant 3). libopus checks `frame_size` is an Opus frame size and returns
        // `OPUS_BAD_ARG` otherwise.
        let n = unsafe { opus_encode_float(self.st.as_ptr(), pcm.as_ptr(), fs, out.as_mut_ptr(), max) };
        usize::try_from(n).map_err(|_| n)
    }
}

impl Drop for RawEncoder {
    fn drop(&mut self) {
        // SAFETY: the state came from opus_encoder_create and is freed exactly once (invariant 1).
        unsafe { opus_encoder_destroy(self.st.as_ptr()) }
    }
}

/// An owned `OpusDecoder*` (invariant 1).
pub(crate) struct RawDecoder {
    st: NonNull<OpusDecoder>,
    channels: usize,
}

// SAFETY: opus.h: "The decoder state is always continuous in memory and only a shallow copy is
// sufficient to copy it", and "A single codec state may only be accessed from a single thread at
// a time": it may move between threads, and access stays exclusive through `&mut self`
// (invariant 2). The library is built with VAR_ARRAYS or USE_ALLOCA, never
// NONTHREADSAFE_PSEUDOSTACK (build.rs). `RawDecoder` is not `Sync`.
unsafe impl Send for RawDecoder {}

impl RawDecoder {
    /// `opus_decoder_create(fs, channels, &error)`.
    pub(crate) fn new(fs: i32, channels: usize) -> Result<Self, c_int> {
        let ch = c_int::try_from(channels).map_err(|_| -1)?;
        let mut error: c_int = OPUS_OK;
        // SAFETY: opus.h `opus_decoder_create`: `error` is an `int*` out-parameter, here a live
        // local; `fs` and `channels` are validated by libopus ("must be one of 8000, 12000, 16000,
        // 24000, or 48000", "(1 or 2)"), which returns null and an error code otherwise.
        let st = unsafe { opus_decoder_create(fs, ch, &raw mut error) };
        match NonNull::new(st) {
            Some(st) if error == OPUS_OK => Ok(RawDecoder { st, channels }),
            Some(st) => {
                // SAFETY: a non-null state from opus_decoder_create is freed once, here (invariant 1).
                unsafe { opus_decoder_destroy(st.as_ptr()) };
                Err(error)
            }
            None => Err(if error == OPUS_OK { -7 } else { error }),
        }
    }

    /// `opus_decode_float`: one packet into `pcm` (interleaved), which holds `frame_size` samples
    /// per channel. Returns the samples per channel decoded.
    pub(crate) fn decode(&mut self, packet: &[u8], pcm: &mut [f32], frame_size: usize) -> Result<usize, c_int> {
        if packet.is_empty() || frame_size == 0 || pcm.len() != frame_size.saturating_mul(self.channels) {
            return Err(-1);
        }
        let len = c_int::try_from(packet.len()).map_err(|_| -1)?;
        let fs = c_int::try_from(frame_size).map_err(|_| -1)?;
        // SAFETY: the state is valid and exclusively borrowed (invariants 1, 2). opus.h
        // `opus_decode_float`: `data` is `len` readable bytes (a non-empty slice, so never the
        // null that would request concealment); "pcm ... length is frame_size*channels*
        // sizeof(float)", checked above (invariant 3); `decode_fec` is 0. libopus reads no more
        // than `len` bytes and writes no more than `frame_size` samples per channel, returning
        // `OPUS_BUFFER_TOO_SMALL` for a longer packet.
        let n = unsafe { opus_decode_float(self.st.as_ptr(), packet.as_ptr(), len, pcm.as_mut_ptr(), fs, 0) };
        usize::try_from(n).map_err(|_| n)
    }
}

impl Drop for RawDecoder {
    fn drop(&mut self) {
        // SAFETY: the state came from opus_decoder_create and is freed exactly once (invariant 1).
        unsafe { opus_decoder_destroy(self.st.as_ptr()) }
    }
}

//! State streams (`clap.state`): in-memory read and write cursors.

use super::*;

// ---- state streams -------------------------------------------------------------------------

/// Read side of an in-memory state blob.
pub(super) struct ReadCursor<'a> {
    pub(super) data: &'a [u8],
    pub(super) pos: usize,
}

pub(super) unsafe extern "C" fn istream_read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the `ReadCursor` on `load_state`'s stack frame, alive during the call.
    let Some(c) = (unsafe { ((*stream).ctx as *mut ReadCursor<'_>).as_mut() }) else { return -1 };
    let rest = c.data.get(c.pos..).unwrap_or(&[]);
    let n = rest.len().min(usize::try_from(size).unwrap_or(usize::MAX));
    if let Some(src) = rest.get(..n) {
        // SAFETY: the plugin provides `size` ≥ `n` writable bytes at `buffer`; `src` is `n`
        // readable bytes of our own slice; they cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
    }
    c.pos += n;
    i64::try_from(n).unwrap_or(-1)
}

pub(super) unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the output Vec on `save_state`'s stack frame, alive during the call.
    let Some(v) = (unsafe { ((*stream).ctx as *mut Vec<u8>).as_mut() }) else { return -1 };
    let Ok(n) = usize::try_from(size) else { return -1 };
    if v.len().saturating_add(n) > MAX_STATE_BYTES {
        return -1;
    }
    // SAFETY: the plugin provides `size` readable bytes at `buffer` for the call.
    let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
    v.extend_from_slice(src);
    i64::try_from(n).unwrap_or(-1)
}

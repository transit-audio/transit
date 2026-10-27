//! Host-side SDK objects: the message and attribute list services, and the in-memory stream.

use super::*;

// Test-only, thread-local failure countdown: no global allocator changes or real OOM.
#[cfg(test)]
thread_local! {
    pub(super) static HOST_ALLOC_FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn host_allocation() -> Result<(), sb::tresult> {
    #[cfg(test)]
    HOST_ALLOC_FAIL_AFTER.with(|remaining| match remaining.get() {
        Some(0) => Err(sb::kOutOfMemory),
        Some(n) => {
            remaining.set(Some(n - 1));
            Ok(())
        }
        None => Ok(()),
    })?;
    Ok(())
}

fn try_host_copy<T: Copy>(source: &[T]) -> Result<Vec<T>, sb::tresult> {
    let mut copy = Vec::new();
    if !source.is_empty() {
        host_allocation()?;
        copy.try_reserve_exact(source.len()).map_err(|_| sb::kOutOfMemory)?;
        copy.extend_from_slice(source);
    }
    Ok(copy)
}

/// Allocate a single-interface SDK object with one reference. Only the two matching static
/// vtables below are supplied here; each uses HostObject<C> and offset zero.
pub(super) fn try_host_object<C, I>(data: C, vtbl: &'static I::Vtbl) -> Result<ComPtr<I>, sb::tresult>
where
    C: Class<Interfaces = (I,)> + 'static,
    I: Construct<C, HostObject<C>, 0>,
{
    host_allocation()?;
    let layout = std::alloc::Layout::new::<HostObject<C>>();
    // SAFETY: the header/counter make layout nonzero; null is handled before any write.
    let allocation = unsafe { std::alloc::alloc(layout) }.cast::<HostObject<C>>();
    if allocation.is_null() {
        return Err(sb::kOutOfMemory);
    }
    // SAFETY: allocation has the exact size/alignment of HostObject<C>. Its single-interface
    // header starts at offset zero, with a vtable pointer as required by Interface/MakeHeader.
    // The supplied static vtable retains the generated thunks for C and this wrapper, replacing
    // only QI. ComPtr takes the sole initial reference and release frees this exact allocation.
    unsafe {
        allocation.write(HostObject { header: <(I,) as MakeHeader<C, HostObject<C>>>::HEADER, refs: AtomicUsize::new(1), data });
        allocation.cast::<*const I::Vtbl>().write(vtbl);
        Ok(ComPtr::from_raw_unchecked(allocation.cast::<I>()))
    }
}

/// vst3 0.3.0's FUnknown QI neither clears failed outputs nor checks null arguments. Wrap that
/// third-party helper for the new objects; successful QI retains its ABI, identity and AddRef.
///
/// # Safety
/// `this` must be the live single-interface HostObject<C> header. Non-null `iid` must be
/// readable, and non-null `obj` must be a distinct writable pointer.
unsafe extern "system" fn host_query_interface<C: Class>(this: *mut FUnknown, iid: *const sb::TUID, obj: *mut *mut c_void) -> sb::tresult {
    if obj.is_null() {
        return sb::kInvalidArgument;
    }
    // SAFETY: the caller supplies a writable pointer, and null was rejected.
    unsafe { obj.write(std::ptr::null_mut()) };
    if iid.is_null() {
        return sb::kInvalidArgument;
    }
    // SAFETY: inputs are validated; the generated helper knows C's header and reference counter.
    unsafe { (FUnknown::make_vtbl::<C, HostObject<C>, 0>().queryInterface)(this, iid, obj) }
}

const ATTRIBUTE_VTBL: sv::IAttributeListVtbl = {
    // SAFETY: Construct supplies a static vtable with the SDK ABI for this C/wrapper/offset.
    let mut vtbl = unsafe { *<sv::IAttributeList as Construct<HostAttributeList, HostObject<HostAttributeList>, 0>>::OBJ.vtbl };
    vtbl.base.queryInterface = host_query_interface::<HostAttributeList>;
    vtbl
};
const MESSAGE_VTBL: sv::IMessageVtbl = {
    // SAFETY: Construct supplies a static vtable with the SDK ABI for this C/wrapper/offset.
    let mut vtbl = unsafe { *<sv::IMessage as Construct<HostMessage, HostObject<HostMessage>, 0>>::OBJ.vtbl };
    vtbl.base.queryInterface = host_query_interface::<HostMessage>;
    vtbl
};

/// One `IAttributeList` value.
pub(super) enum Attr {
    Int(i64),
    Float(f64),
    /// UTF-16 without the terminator.
    String(Vec<u16>),
    Binary(Vec<u8>),
}

/// A host `IAttributeList` (created through `IHostApplication::createInstance`, or owned by a
/// [`HostMessage`]). Binary values handed out by `getBinary` stay valid until that attribute is
/// set again or the list is released, as with the SDK's own host classes.
#[derive(Default)]
pub(crate) struct HostAttributeList {
    pub(super) attrs: Mutex<std::collections::HashMap<Vec<u8>, Attr>>,
}

impl Class for HostAttributeList {
    type Interfaces = (sv::IAttributeList,);
}

/// Borrow a terminated SDK string without allocation, rejecting oversized input.
///
/// # Safety
/// Non-null `ptr` must be readable through its first NUL (including that terminator) and
/// remain valid for the returned borrow. `max` must be less than isize::MAX / size_of::<T>().
unsafe fn host_string<'a, T: Copy + Default + PartialEq>(ptr: *const T, max: usize) -> Result<&'a [T], sb::tresult> {
    if ptr.is_null() {
        return Err(sb::kInvalidArgument);
    }
    for len in 0..=max {
        // SAFETY: the SDK supplies terminated input; we stop at its first terminator or cap.
        if unsafe { *ptr.add(len) } == T::default() {
            // SAFETY: these len elements precede the terminator and fit within isize::MAX.
            return Ok(unsafe { std::slice::from_raw_parts(ptr, len) });
        }
    }
    Err(sb::kResultFalse)
}

impl HostAttributeList {
    pub(super) fn try_new() -> Result<ComPtr<sv::IAttributeList>, sb::tresult> {
        try_host_object(Self::default(), &ATTRIBUTE_VTBL)
    }

    /// # Safety
    /// `id` must be null or a readable NUL-terminated SDK attribute ID for this call.
    unsafe fn set(&self, id: sv::IAttributeList_::AttrID, v: Attr) -> sb::tresult {
        // SAFETY: forwarded SDK string contract; keys are only borrowed during this call.
        let key = match unsafe { host_string(id.cast::<u8>(), ATTR_STRING_MAX) } {
            Ok(k) => k,
            Err(e) => return e,
        };
        let mut attrs = self.attrs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(old) = attrs.get_mut(key) {
            *old = v;
            return sb::kResultOk;
        }
        if attrs.len() >= ATTR_COUNT_MAX {
            return sb::kOutOfMemory;
        }
        let key = match try_host_copy(key) {
            Ok(k) => k,
            Err(e) => return e,
        };
        if attrs.len() == attrs.capacity() && (host_allocation().is_err() || attrs.try_reserve(1).is_err()) {
            return sb::kOutOfMemory;
        }
        attrs.insert(key, v);
        sb::kResultOk
    }

    /// # Safety
    /// `id` must be null or a readable NUL-terminated SDK attribute ID for this call.
    unsafe fn get<T>(&self, id: sv::IAttributeList_::AttrID, f: impl FnOnce(&Attr) -> Option<T>) -> Result<T, sb::tresult> {
        // SAFETY: forwarded SDK string contract; the closure cannot retain the key or value borrow.
        let key = unsafe { host_string(id.cast::<u8>(), ATTR_STRING_MAX) }?;
        self.attrs.lock().unwrap_or_else(PoisonError::into_inner).get(key).and_then(f).ok_or(sb::kResultFalse)
    }
}

/// Maximum accepted text length (excluding NUL), binary size, and distinct attribute count.
pub(super) const ATTR_STRING_MAX: usize = 1 << 20;
pub(super) const ATTR_BINARY_MAX: usize = 64 << 20;
const ATTR_COUNT_MAX: usize = 1 << 16;

impl IAttributeListTrait for HostAttributeList {
    /// # Safety
    /// `id` must be null or a readable NUL-terminated SDK attribute ID.
    unsafe fn setInt(&self, id: sv::IAttributeList_::AttrID, value: i64) -> sb::tresult {
        // SAFETY: forwards the SDK attribute ID contract.
        unsafe { self.set(id, Attr::Int(value)) }
    }
    /// # Safety
    /// `id` must be null or terminated; `value` must be null or a distinct writable i64.
    unsafe fn getInt(&self, id: sv::IAttributeList_::AttrID, value: *mut i64) -> sb::tresult {
        // SAFETY: the plugin's out-pointer (or null, handled).
        let Some(out) = (unsafe { value.as_mut() }) else { return sb::kInvalidArgument };
        // SAFETY: forwards the SDK attribute ID contract; only the scalar is copied.
        match unsafe { self.get(id, |a| if let Attr::Int(v) = a { Some(*v) } else { None }) } {
            Ok(v) => {
                *out = v;
                sb::kResultOk
            }
            Err(e) => e,
        }
    }
    /// # Safety
    /// `id` must be null or a readable NUL-terminated SDK attribute ID.
    unsafe fn setFloat(&self, id: sv::IAttributeList_::AttrID, value: f64) -> sb::tresult {
        // SAFETY: forwards the SDK attribute ID contract.
        unsafe { self.set(id, Attr::Float(value)) }
    }
    /// # Safety
    /// `id` must be null or terminated; `value` must be null or a distinct writable f64.
    unsafe fn getFloat(&self, id: sv::IAttributeList_::AttrID, value: *mut f64) -> sb::tresult {
        // SAFETY: the plugin's out-pointer (or null, handled).
        let Some(out) = (unsafe { value.as_mut() }) else { return sb::kInvalidArgument };
        // SAFETY: forwards the SDK attribute ID contract; only the scalar is copied.
        match unsafe { self.get(id, |a| if let Attr::Float(v) = a { Some(*v) } else { None }) } {
            Ok(v) => {
                *out = v;
                sb::kResultOk
            }
            Err(e) => e,
        }
    }
    /// # Safety
    /// `id` and `string` must be null or readable NUL-terminated SDK strings.
    unsafe fn setString(&self, id: sv::IAttributeList_::AttrID, string: *const sv::TChar) -> sb::tresult {
        if id.is_null() {
            return sb::kInvalidArgument;
        }
        // SAFETY: the SDK supplies a terminated UTF-16 string; the helper rejects null/oversize.
        let source = match unsafe { host_string(string, ATTR_STRING_MAX) } {
            Ok(s) => s,
            Err(e) => return e,
        };
        let copy = match try_host_copy(source) {
            Ok(s) => s,
            Err(e) => return e,
        };
        // SAFETY: forwards the SDK attribute ID contract after successfully copying the value.
        unsafe { self.set(id, Attr::String(copy)) }
    }
    /// # Safety
    /// `id` must be null or terminated; `string` must be null or an aligned, distinct writable
    /// buffer of size_in_bytes bytes. It must not alias this list's stored values.
    unsafe fn getString(&self, id: sv::IAttributeList_::AttrID, string: *mut sv::TChar, size_in_bytes: u32) -> sb::tresult {
        let units = size_in_bytes as usize / 2;
        if string.is_null() || units == 0 || units > isize::MAX as usize / 2 {
            return sb::kInvalidArgument;
        }
        // SAFETY: the plugin supplies the writable UTF-16 buffer; its length fits isize::MAX.
        let dst = unsafe { std::slice::from_raw_parts_mut(string, units) };
        // SAFETY: forwards the SDK ID contract; copies while the lock keeps storage stable.
        unsafe {
            self.get(id, |a| {
                if let Attr::String(s) = a {
                    write_c16_units(s, dst);
                    Some(())
                } else {
                    None
                }
            })
        }
        .map_or_else(|e| e, |()| sb::kResultOk)
    }
    /// # Safety
    /// `id` must be null or terminated. Nonempty `data` must have size_in_bytes readable bytes;
    /// rejected oversized buffers are never read.
    unsafe fn setBinary(&self, id: sv::IAttributeList_::AttrID, data: *const c_void, size_in_bytes: u32) -> sb::tresult {
        if id.is_null() {
            return sb::kInvalidArgument;
        }
        let len = size_in_bytes as usize;
        if len > ATTR_BINARY_MAX {
            return sb::kOutOfMemory;
        }
        let source = if len == 0 {
            &[]
        } else if data.is_null() {
            return sb::kInvalidArgument;
        } else {
            // SAFETY: the SDK supplies len readable bytes; the cap also fits isize::MAX on 32-bit.
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }
        };
        let bytes = match try_host_copy(source) {
            Ok(b) => b,
            Err(e) => return e,
        };
        // SAFETY: forwards the SDK attribute ID contract after successfully copying the bytes.
        unsafe { self.set(id, Attr::Binary(bytes)) }
    }
    /// # Safety
    /// `id` must be null or terminated; non-null output pointers must be aligned, writable and
    /// distinct. Borrowed data must not be used concurrently with replacement/final release.
    unsafe fn getBinary(&self, id: sv::IAttributeList_::AttrID, data: *mut *const c_void, size_in_bytes: *mut u32) -> sb::tresult {
        // SAFETY: the plugin's out-pointers (or null, handled).
        let (Some(d), Some(n)) = (unsafe { (data.as_mut(), size_in_bytes.as_mut()) }) else { return sb::kInvalidArgument };
        // The pointer stays valid after the lock is released: the vector is untouched until the
        // attribute is replaced or the list is released (the SDK's contract).
        // SAFETY: forwards the SDK ID contract; the caller synchronizes borrowed pointer use.
        match unsafe { self.get(id, |a| if let Attr::Binary(b) = a { Some((b.as_ptr().cast::<c_void>(), b.len())) } else { None }) } {
            Ok((p, len)) => {
                *d = p;
                *n = u32::try_from(len).unwrap_or(u32::MAX);
                sb::kResultOk
            }
            Err(e) => {
                *d = std::ptr::null();
                *n = 0;
                e
            }
        }
    }
}

/// Copies UTF-16 `src` into `dst` with a terminator, truncating to fit.
fn write_c16_units(src: &[u16], dst: &mut [u16]) {
    let Some(room) = dst.len().checked_sub(1) else { return };
    let n = src.len().min(room);
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

/// A host `IMessage`: an id plus its own attribute list.
pub(crate) struct HostMessage {
    /// NUL-terminated bytes, valid until the next successful setMessageID or final release.
    id: Mutex<Option<Vec<u8>>>,
    attrs: ComPtr<sv::IAttributeList>,
}

impl HostMessage {
    pub(super) fn try_new() -> Result<ComPtr<sv::IMessage>, sb::tresult> {
        let attrs = HostAttributeList::try_new()?;
        try_host_object(HostMessage { id: Mutex::new(None), attrs }, &MESSAGE_VTBL)
    }
}

impl Class for HostMessage {
    type Interfaces = (sv::IMessage,);
}

impl IMessageTrait for HostMessage {
    /// # Safety
    /// The returned borrowed ID must not be used concurrently with replacement/final release.
    unsafe fn getMessageID(&self) -> sb::FIDString {
        self.id.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map_or(std::ptr::null(), |s| s.as_ptr().cast())
    }
    /// # Safety
    /// `id` must be null or a readable NUL-terminated SDK string for this call.
    unsafe fn setMessageID(&self, id: sb::FIDString) {
        let new = if id.is_null() {
            None
        } else {
            // SAFETY: the SDK supplies a terminated ID; the helper checks null and the length cap.
            let Ok(source) = (unsafe { host_string(id.cast::<u8>(), ATTR_STRING_MAX) }) else { return };
            // SAFETY: source excludes the terminator, which the helper read; include it in the copy.
            let source = unsafe { std::slice::from_raw_parts(id.cast::<u8>(), source.len() + 1) };
            // The SDK method returns void: preserve the prior ID on failure. Copy first so
            // assigning our own borrowed ID is also valid.
            let Ok(copy) = try_host_copy(source) else { return };
            Some(copy)
        };
        *self.id.lock().unwrap_or_else(PoisonError::into_inner) = new;
    }
    /// # Safety
    /// The returned pointer is borrowed for the message lifetime; callers must AddRef to retain it.
    unsafe fn getAttributes(&self) -> *mut sv::IAttributeList {
        self.attrs.as_ptr()
    }
}

/// An in-memory `IBStream`, used to hand the component's state to its controller.
pub(crate) struct MemoryStream {
    pub(super) inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemoryStream {
    type Interfaces = (sb::IBStream,);
}

impl MemoryStream {
    pub(super) fn new(data: Vec<u8>) -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream { inner: Mutex::new((data, 0)) })
    }

    pub(super) fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.lock().0)
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<u8>, usize)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: sb::int32, num_read: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let want = usize::try_from(num_bytes).unwrap_or(0);
        let avail = data.len().saturating_sub(*pos);
        let n = want.min(avail);
        if n > 0 && !buffer.is_null() {
            let Some(src) = data.get(*pos..*pos + n) else { return sb::kInternalError };
            // SAFETY: the plugin provides `buffer` with room for `num_bytes` ≥ `n` bytes; `src`
            // is `n` valid bytes; the regions cannot overlap (ours is a private Vec).
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
            *pos += n;
        }
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { num_read.as_mut() } {
            *r = i32::try_from(n).unwrap_or(0);
        }
        sb::kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: sb::int32, num_written: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let n = usize::try_from(num_bytes).unwrap_or(0);
        let end = pos.saturating_add(n);
        if buffer.is_null() || end > MAX_STATE_BYTES {
            return sb::kResultFalse;
        }
        if data.len() < end {
            data.resize(end, 0);
        }
        // SAFETY: the plugin provides `n` readable bytes at `buffer`; the destination range was
        // just made valid.
        let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
        if let Some(dst) = data.get_mut(*pos..end) {
            dst.copy_from_slice(src);
        }
        *pos = end;
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(w) = unsafe { num_written.as_mut() } {
            *w = num_bytes;
        }
        sb::kResultOk
    }
    unsafe fn seek(&self, pos: sb::int64, mode: sb::int32, result: *mut sb::int64) -> sb::tresult {
        let mut g = self.lock();
        let len = i64::try_from(g.0.len()).unwrap_or(i64::MAX);
        let cur = i64::try_from(g.1).unwrap_or(0);
        #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
        let base = match mode as u32 {
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => cur,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => len,
            _ => return sb::kInvalidArgument,
        };
        let new = base.saturating_add(pos).clamp(0, len);
        g.1 = usize::try_from(new).unwrap_or(0);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { result.as_mut() } {
            *r = new;
        }
        sb::kResultOk
    }
    unsafe fn tell(&self, pos: *mut sb::int64) -> sb::tresult {
        let g = self.lock();
        // SAFETY: out-pointer from the plugin (or null, handled).
        match unsafe { pos.as_mut() } {
            Some(p) => {
                *p = i64::try_from(g.1).unwrap_or(0);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
}

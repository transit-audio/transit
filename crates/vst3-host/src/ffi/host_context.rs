//! The host context the plug-in calls back into (`IHostApplication`, `IComponentHandler`) and the COM object wrapper.

use super::*;

// ---- host-side COM objects -----------------------------------------------------------------

/// The host context handed to `initialize` (`IHostApplication`) and to the controller
/// (`IComponentHandler`: parameter edits made in the plugin's editor are queued for the UI, which
/// writes them into the session, from where they reach the processor).
///
/// `restartComponent` (a UI-thread call) only records its flags here; the main-thread service
/// (`plugin::Shared::service`) acts on them (audit F08, ADR 0012 G7).
pub(crate) struct HostContext {
    /// `kLatencyChanged`: deactivate, reactivate, then read the latency (main thread).
    pub latency_changed: AtomicBool,
    pub restart_requested: AtomicBool,
    /// `restartComponent` carried `kParamTitlesChanged` (curve shapes can change): the service
    /// invalidates the conversion curves, which the editor handle re-samples on the UI thread.
    pub titles_changed: AtomicBool,
    /// `restartComponent` carried `kParamValuesChanged` (values changed, curves did not; many
    /// plug-ins send it on every preset load): the host re-reads the values (audit F08), and the
    /// curves stay.
    pub values_changed: AtomicBool,
    /// `kMidiCCAssignmentChanged`: re-read the `IMidiMapping` table (main thread).
    pub midi_cc_changed: AtomicBool,
    /// `kIoChanged`: deactivate, renegotiate the buses, reactivate (main thread).
    pub io_changed: AtomicBool,
    /// `kReloadComponent`: not supported (the component would have to be created again).
    pub reload_requested: AtomicBool,
    /// Any other flags, or-ed (nothing this host needs to do; logged).
    pub other_flags: AtomicU32,
    /// `performEdit` values (normalized), bounded by `EDIT_CAP`.
    pub edits: Mutex<Vec<(u32, f64)>>,
}

impl Default for HostContext {
    fn default() -> Self {
        let edits = Mutex::new(Vec::with_capacity(EDIT_CAP));
        // Plug-ins are known to call `performEdit` from the audio thread despite the spec, and on
        // macOS the first lock boxes the pthread mutex (an allocation). Take it once here.
        drop(edits.lock());
        HostContext {
            latency_changed: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            titles_changed: AtomicBool::new(false),
            values_changed: AtomicBool::new(false),
            midi_cc_changed: AtomicBool::new(false),
            io_changed: AtomicBool::new(false),
            reload_requested: AtomicBool::new(false),
            other_flags: AtomicU32::new(0),
            edits,
        }
    }
}

impl HostContext {
    /// Takes the queued editor edits.
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }
}

impl Class for HostContext {
    type Interfaces = (sv::IHostApplication, sv::IComponentHandler);
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut sv::String128) -> sb::tresult {
        // SAFETY: the plugin passes a pointer to a writable String128 (or null, handled).
        match unsafe { name.as_mut() } {
            Some(n) => {
                write_c16("SoundCraft", n);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }

    /// Host-side `IMessage` and `IAttributeList` objects, which plug-ins whose component and
    /// controller talk through `IConnectionPoint` allocate here (the SDK's `allocateMessage`).
    /// A host must provide them; some plug-ins (iZotope Ozone 11) use the result unchecked.
    ///
    /// # Safety
    /// Non-null IDs must point to readable TUIDs; `obj` must be null or a distinct writable pointer.
    unsafe fn createInstance(&self, cid: *mut sb::TUID, iid: *mut sb::TUID, obj: *mut *mut c_void) -> sb::tresult {
        // SAFETY: `obj` is the plugin's out-pointer (or null, handled).
        let Some(out) = (unsafe { obj.as_mut() }) else { return sb::kInvalidArgument };
        *out = std::ptr::null_mut();
        // SAFETY: the plugin passes two readable 16-byte TUIDs (or null, handled).
        let (Some(cid), Some(iid)) = (unsafe { (cid.as_ref(), iid.as_ref()) }) else { return sb::kInvalidArgument };
        let (cid, iid) = (tuid_bytes(cid), tuid_bytes(iid));
        // Refuse unsupported pairs before allocating, including under memory pressure.
        let raw = if cid == sv::IMessage::IID && (iid == cid || iid == FUnknown::IID) {
            HostMessage::try_new().map(|p| p.into_raw().cast())
        } else if cid == sv::IAttributeList::IID && (iid == cid || iid == FUnknown::IID) {
            HostAttributeList::try_new().map(|p| p.into_raw().cast())
        } else {
            return sb::kResultFalse;
        };
        match raw {
            Ok(p) => {
                *out = p;
                sb::kResultOk
            }
            Err(e) => e,
        }
    }
}

impl IComponentHandlerTrait for HostContext {
    unsafe fn beginEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn performEdit(&self, id: sv::ParamID, value: sv::ParamValue) -> sb::tresult {
        if value.is_finite() {
            let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
            let v = value.clamp(0.0, 1.0);
            if let Some(x) = e.iter_mut().find(|(i, _)| *i == id) {
                x.1 = v;
            } else if e.len() < EDIT_CAP {
                e.push((id, v));
            }
        }
        sb::kResultOk
    }
    unsafe fn endEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn restartComponent(&self, flags: sb::int32) -> sb::tresult {
        if flags as u32 & sv::RestartFlags_::kLatencyChanged as u32 != 0 {
            self.latency_changed.store(true, Ordering::Relaxed);
        }
        if flags as u32 & sv::RestartFlags_::kParamTitlesChanged as u32 != 0 {
            self.titles_changed.store(true, Ordering::Relaxed);
        }
        if flags as u32 & sv::RestartFlags_::kParamValuesChanged as u32 != 0 {
            self.values_changed.store(true, Ordering::Relaxed);
        }
        if flags as u32 & sv::RestartFlags_::kMidiCCAssignmentChanged as u32 != 0 {
            self.midi_cc_changed.store(true, Ordering::Relaxed);
        }
        if flags as u32 & sv::RestartFlags_::kIoChanged as u32 != 0 {
            self.io_changed.store(true, Ordering::Relaxed);
        }
        if flags as u32 & sv::RestartFlags_::kReloadComponent as u32 != 0 {
            self.reload_requested.store(true, Ordering::Relaxed);
        }
        let handled = sv::RestartFlags_::kLatencyChanged as u32
            | sv::RestartFlags_::kParamTitlesChanged as u32
            | sv::RestartFlags_::kParamValuesChanged as u32
            | sv::RestartFlags_::kMidiCCAssignmentChanged as u32
            | sv::RestartFlags_::kIoChanged as u32
            | sv::RestartFlags_::kReloadComponent as u32;
        self.other_flags.fetch_or(flags as u32 & !handled, Ordering::Relaxed);
        self.restart_requested.store(true, Ordering::Release);
        sb::kResultOk
    }
}

// ComWrapper uses infallible Arc allocation. These two SDK objects instead allocate one
// header/data block fallibly, retaining the dependency's generated ABI thunks.
#[repr(C)]
pub(super) struct HostObject<C: Class> {
    pub(super) header: Header<C>,
    pub(super) refs: AtomicUsize,
    pub(super) data: C,
}

// fetch_update keeps this wrapper compatible with the crate's Rust 1.90 MSRV.
#[allow(deprecated)]
impl<C: Class> Wrapper<C> for HostObject<C> {
    /// # Safety
    /// `ptr` must be the header of a live allocation made by `try_host_object`.
    unsafe fn data_from_header(ptr: *mut Header<C>) -> *mut C {
        // SAFETY: repr(C) puts the header first; the allocation contains an initialized C.
        unsafe { std::ptr::addr_of_mut!((*ptr.cast::<Self>()).data) }
    }

    /// # Safety
    /// `ptr` must be the data member of a live allocation made by `try_host_object`.
    unsafe fn header_from_data(ptr: *mut C) -> *mut Header<C> {
        // SAFETY: subtracting the repr(C) member offset recovers the original allocation/header.
        unsafe { ptr.cast::<u8>().sub(std::mem::offset_of!(Self, data)).cast() }
    }

    /// # Safety
    /// `ptr` must name live data and the caller must hold a reference throughout the call.
    unsafe fn add_ref(ptr: *mut C) -> usize {
        // SAFETY: the caller's live reference keeps this header and atomic counter alive.
        let object = unsafe { &*Self::header_from_data(ptr).cast::<Self>() };
        // Saturate instead of wrapping. A saturated object is intentionally never freed.
        object.refs.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_add(1))).unwrap_or(usize::MAX).saturating_add(1)
    }

    /// # Safety
    /// `ptr` must name live data and the caller must surrender exactly one owned reference.
    unsafe fn release(ptr: *mut C) -> usize {
        // SAFETY: the surrendered reference keeps the allocation alive through the decrement.
        let allocation = unsafe { Self::header_from_data(ptr).cast::<Self>() };
        // SAFETY: allocation is live; only the atomic counter is accessed before the last release.
        let previous = unsafe { &(*allocation).refs }
            .fetch_update(Ordering::Release, Ordering::Relaxed, |n| if n == usize::MAX { None } else { n.checked_sub(1) });
        match previous {
            Ok(1) => {
                std::sync::atomic::fence(Ordering::Acquire);
                // SAFETY: this was the last reference. Drop once, then free with the allocation's
                // original layout; no references to the header or data are used afterward.
                unsafe {
                    std::ptr::drop_in_place(allocation);
                    std::alloc::dealloc(allocation.cast(), std::alloc::Layout::new::<Self>());
                }
                0
            }
            Ok(n) => n.saturating_sub(1),
            Err(n) => n,
        }
    }
}

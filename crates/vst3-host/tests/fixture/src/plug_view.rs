use super::*;

/// Issue #101: the view scales on request: it reports its size at the factor and, once attached,
/// asks the host for it through `resizeView` (the SDK's contract). Issue #105, scale modes: see
/// [`ScaleMode`].
impl IPlugViewContentScaleSupportTrait for View {
    unsafe fn setContentScaleFactor(&self, factor: f32) -> tresult {
        if !(factor > 0.0) {
            return kInvalidArgument;
        }
        let attached = self.attached.load(Ordering::Relaxed);
        match self.mode {
            ScaleMode::Refuse => return kResultFalse,
            ScaleMode::AfterAttach if !attached => return kResultTrue,
            _ => {}
        }
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        let frame = self.frame.load(Ordering::Relaxed);
        if attached && let Some(f) = unsafe { ComRef::from_raw(frame) } {
            let mut r = self.rect();
            assert_eq!(unsafe { f.resizeView(std::ptr::null_mut(), &mut r) }, kResultOk, "resizeView");
        }
        kResultTrue
    }
}

impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, t: FIDString) -> tresult {
        let t = unsafe { std::ffi::CStr::from_ptr(t) }.to_bytes();
        if t == b"NSView" || t == b"HWND" || t == b"X11EmbedWindowID" { kResultTrue } else { kResultFalse }
    }
    unsafe fn attached(&self, parent: *mut c_void, _t: FIDString) -> tresult {
        if parent.is_null() {
            return kInvalidArgument;
        }
        self.attached.store(true, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn removed(&self) -> tresult {
        assert!(self.attached.swap(false, Ordering::Relaxed), "removed without attached");
        kResultOk
    }
    unsafe fn onWheel(&self, _d: f32) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyDown(&self, _k: char16, _c: i16, _m: i16) -> tresult {
        kResultFalse
    }
    unsafe fn onKeyUp(&self, _k: char16, _c: i16, _m: i16) -> tresult {
        kResultFalse
    }
    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        let r = unsafe { &mut *size };
        *r = self.rect();
        kResultOk
    }
    unsafe fn onSize(&self, _s: *mut ViewRect) -> tresult {
        kResultOk
    }
    unsafe fn onFocus(&self, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setFrame(&self, f: *mut IPlugFrame) -> tresult {
        // The host keeps its frame alive while it is set (it clears it before releasing it).
        self.frame.store(f, Ordering::Relaxed);
        kResultOk
    }
    unsafe fn canResize(&self) -> tresult {
        kResultFalse
    }
    unsafe fn checkSizeConstraint(&self, _r: *mut ViewRect) -> tresult {
        kResultOk
    }
}


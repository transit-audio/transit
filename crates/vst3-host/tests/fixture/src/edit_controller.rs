use super::*;

impl IEditControllerTrait for GainController {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        ctrl_call("setComponentState");
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            if f64::from_le_bytes(bytes) == -1.0 {
                return kInternalError;
            }
            self.gain.store(f64::from_le_bytes(bytes).to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        ctrl_call("setState");
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            if f64::from_le_bytes(bytes) == -2.0 {
                return kInternalError;
            }
            self.freq.store(f64::from_le_bytes(bytes).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            self.preset.store(f64::from_le_bytes(bytes).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
        // A third f64: after that many ms, a thread of the plug-in's own tells the host its values
        // changed (with nothing changed), outside any host call.
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } == kResultOk && n == 8 {
            let ms = f64::from_le_bytes(bytes).clamp(0.0, 10_000.0) as u64;
            let h = HandlerPtr(self.handler.load(Ordering::SeqCst));
            std::thread::spawn(move || {
                let h = h;
                std::thread::sleep(std::time::Duration::from_millis(ms));
                if let Some(h) = unsafe { ComRef::from_raw(h.0) } {
                    unsafe { h.restartComponent(RestartFlags_::kParamValuesChanged as i32) };
                }
            });
        }
        // Like many plug-ins, it tells the host its values changed while it loads a state.
        if let Some(h) = unsafe { ComRef::from_raw(self.handler.load(Ordering::SeqCst)) } {
            unsafe { h.restartComponent(RestartFlags_::kParamValuesChanged as i32) };
        }
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        ctrl_call("getState");
        let preset = self.preset.load(Ordering::Relaxed);
        if preset != u64::MAX && self.gain.swap(preset, Ordering::Relaxed) != preset {
            if let Some(h) = unsafe { ComRef::from_raw(self.handler.load(Ordering::SeqCst)) } {
                unsafe { h.restartComponent(RestartFlags_::kParamValuesChanged as i32) };
            }
        }
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = f64::from_bits(self.freq.load(Ordering::Relaxed)).to_le_bytes();
        let mut n = 0;
        unsafe { s.write(bytes.as_mut_ptr().cast(), 8, &mut n) }
    }
    unsafe fn getParameterCount(&self) -> i32 {
        ctrl_call("getParameterCount");
        (GAIN_PARAMS.len() + if many() > 0 { many() + 1 } else { 0 }) as i32
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        ctrl_call("getParameterInfo");
        let Some((id, title, units, steps, def, flags)) = gain_param(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.id = id;
        copy_c16(&title, &mut i.title);
        copy_c16(&title, &mut i.shortTitle);
        copy_c16(units, &mut i.units);
        i.stepCount = steps;
        i.defaultNormalizedValue = def;
        i.unitId = 0;
        i.flags = flags;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, id: u32, n: f64, string: *mut String128) -> tresult {
        ctrl_call("getParamStringByValue");
        let s = unsafe { &mut *string };
        let text = match id {
            3 => ["A", "B", "C"][((n * 2.0).round() as usize).min(2)].to_string(),
            _ => format!("{:.2}", unsafe { self.normalizedParamToPlain(id, n) }),
        };
        copy_c16(&text, s);
        kResultOk
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        ctrl_call("getParamValueByString");
        kNotImplemented
    }
    unsafe fn normalizedParamToPlain(&self, id: u32, n: f64) -> f64 {
        ctrl_call("normalizedParamToPlain");
        gain_to_plain(id, n)
    }
    unsafe fn plainParamToNormalized(&self, id: u32, v: f64) -> f64 {
        ctrl_call("plainParamToNormalized");
        gain_to_normalized(id, v)
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        ctrl_call("getParamNormalized");
        match id {
            7 => f64::from_bits(self.gain.load(Ordering::Relaxed)),
            12 => f64::from_bits(self.freq.load(Ordering::Relaxed)),
            FINE => f64::from_bits(FINE_VALUE.load(Ordering::Relaxed)),
            _ => 0.0,
        }
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        ctrl_call("setParamNormalized");
        if probes() && id == 3 {
            reshape_freq(v >= 0.75);
        }
        if id == 7 {
            self.gain.store(v.to_bits(), Ordering::Relaxed);
        } else if id == 12 {
            self.freq.store(v.to_bits(), Ordering::Relaxed);
        } else if id == FINE {
            FINE_VALUE.store(v.to_bits(), Ordering::Relaxed);
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        ctrl_call("setComponentHandler");
        if !self.hello.load(Ordering::SeqCst) {
            violation("the controller got no IMessage from its connected component");
        }
        HANDLER.store(handler, Ordering::SeqCst);
        self.handler.store(handler, Ordering::SeqCst);
        // Exercise the host's handler: an edit from the "plugin GUI" must be accepted.
        if let Some(h) = unsafe { ComRef::from_raw(handler) } {
            assert_eq!(unsafe { h.beginEdit(7) }, kResultOk);
            assert_eq!(unsafe { h.performEdit(7, 0.9) }, kResultOk);
            assert_eq!(unsafe { h.endEdit(7) }, kResultOk);
        }
        kResultOk
    }
    unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
        ctrl_call("createView");
        if name.is_null() || unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes() != b"editor" {
            return std::ptr::null_mut();
        }
        match ComWrapper::new(View::new()).to_com_ptr::<IPlugView>() {
            Some(p) => p.into_raw(),
            None => std::ptr::null_mut(),
        }
    }
}

impl IEditControllerTrait for Synth {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getParameterCount(&self) -> i32 {
        i32::from(self.events)
    }
    unsafe fn getParameterInfo(&self, index: i32, info: *mut ParameterInfo) -> tresult {
        if !self.events || index != 0 {
            return kInvalidArgument;
        }
        // SAFETY: IEditController::getParameterInfo supplies exclusive, aligned, writable
        // ParameterInfo storage valid for this call; the reference does not escape it.
        let i = unsafe { &mut *info };
        i.id = PROBE_PARAM;
        copy_c16("Probe", &mut i.title);
        copy_c16("Probe", &mut i.shortTitle);
        copy_c16("", &mut i.units);
        i.stepCount = 0;
        i.defaultNormalizedValue = 0.0;
        i.unitId = 0;
        i.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;
        kResultOk
    }
    unsafe fn getParamStringByValue(&self, _id: u32, _n: f64, _s: *mut String128) -> tresult {
        kInvalidArgument
    }
    unsafe fn getParamValueByString(&self, _id: u32, _s: *mut TChar, _v: *mut f64) -> tresult {
        kInvalidArgument
    }
    unsafe fn normalizedParamToPlain(&self, _id: u32, n: f64) -> f64 {
        n
    }
    unsafe fn plainParamToNormalized(&self, _id: u32, v: f64) -> f64 {
        v
    }
    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        if self.events && id == PROBE_PARAM { f64::from_bits(self.probe_ctrl.load(Ordering::SeqCst)) } else { 0.0 }
    }
    unsafe fn setParamNormalized(&self, id: u32, v: f64) -> tresult {
        // The editor's side of the probe: a host must tell the controller what it sets.
        if self.events && id == PROBE_PARAM {
            self.probe_ctrl.store(v.to_bits(), Ordering::SeqCst);
        }
        kResultOk
    }
    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        self.handler.store(handler, Ordering::SeqCst);
        kResultOk
    }
    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
}


use super::*;

impl IPluginBaseTrait for GainProcessor {
    unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        assert!(!context.is_null(), "host context");
        self.host.store(context, Ordering::SeqCst);
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for GainProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = GAIN_CTRL_CID };
        kResultOk
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> i32 {
        if media == MediaTypes_::kAudio as MediaType { 1 } else { 0 }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if media != MediaTypes_::kAudio as MediaType || index != 0 {
            return kInvalidArgument;
        }
        audio_bus(bus, dir, "Main");
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 8];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 8, &mut n) } != kResultOk || n != 8 {
            return kResultFalse;
        }
        let g = f64::from_le_bytes(bytes);
        // Negative sentinels exercise host error paths without a third-party plug-in.
        if !(0.0..=1.0).contains(&g) && g != -1.0 && g != -3.0 {
            return kResultFalse;
        }
        self.gain.store(g.to_bits(), Ordering::Relaxed);
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = f64::from_bits(self.gain.load(Ordering::Relaxed)).to_le_bytes();
        let mut n = 0;
        unsafe { s.write(bytes.as_mut_ptr().cast(), 8, &mut n) }
    }
}

impl IPluginBaseTrait for Keyed {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Keyed {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        if media != MediaTypes_::kAudio as MediaType {
            0
        } else if dir == BusDirections_::kInput as BusDirection {
            2
        } else {
            1
        }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        let input = dir == BusDirections_::kInput as BusDirection;
        if media != MediaTypes_::kAudio as MediaType || index < 0 || index > i32::from(input) {
            return kInvalidArgument;
        }
        audio_bus(bus, dir, if index == 0 { "Main" } else { "Sidechain" });
        if index == 1 {
            // SAFETY: the host passes a writable BusInfo; audio_bus just initialized it.
            let b = unsafe { &mut *bus };
            b.busType = BusTypes_::kAux as BusType;
            b.flags = 0;
        }
        kResultOk
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, media: MediaType, dir: BusDirection, index: i32, state: TBool) -> tresult {
        if media == MediaTypes_::kAudio as MediaType && dir == BusDirections_::kInput as BusDirection && index == 1 {
            self.aux_active.store(state != 0, Ordering::SeqCst);
        }
        kResultOk
    }
    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
}

impl IPluginBaseTrait for GainController {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IPluginBaseTrait for Synth {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Synth {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        let out = dir == BusDirections_::kOutput as BusDirection;
        if media == MediaTypes_::kAudio as MediaType && out {
            1
        } else if media == MediaTypes_::kEvent as MediaType && !out {
            1
        } else {
            0
        }
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        if media == MediaTypes_::kAudio as MediaType && dir == BusDirections_::kOutput as BusDirection {
            audio_bus(bus, dir, "Out");
            return kResultOk;
        }
        if media == MediaTypes_::kEvent as MediaType && dir == BusDirections_::kInput as BusDirection {
            let b = unsafe { &mut *bus };
            b.mediaType = media;
            b.direction = dir;
            b.channelCount = 16;
            copy_c16("Notes", &mut b.name);
            b.busType = BusTypes_::kMain as BusType;
            b.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
            return kResultOk;
        }
        kInvalidArgument
    }
    unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }
    unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        self.note_thread("active");
        if state != 0 && self.refuse.load(Ordering::SeqCst) {
            return kResultFalse;
        }
        if state != 0 {
            let next = self.next_latency.swap(-1, Ordering::SeqCst);
            if next >= 0 {
                self.latency.store(next as u32, Ordering::SeqCst);
            }
        }
        kResultOk
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        // A command, or (anything else) an empty state.
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut bytes = [0u8; 64];
        let mut n = 0;
        if unsafe { s.read(bytes.as_mut_ptr().cast(), 64, &mut n) } == kResultOk
            && let Ok(text) = std::str::from_utf8(&bytes[..n.max(0) as usize])
        {
            self.command(text);
        }
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        let armed = self.armed.lock().unwrap().take();
        if let Some(cmd) = armed {
            self.command(&cmd);
        }
        let every = self.every.lock().unwrap().clone();
        if let Some(cmd) = every {
            self.command(&cmd);
        }
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut text = self.report().into_bytes();
        let mut n = 0;
        unsafe { s.write(text.as_mut_ptr().cast(), text.len() as i32, &mut n) }
    }
}

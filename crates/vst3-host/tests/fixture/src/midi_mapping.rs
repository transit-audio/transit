use super::*;

impl IMidiMappingTrait for Synth {
    unsafe fn getMidiControllerAssignment(&self, bus: i32, channel: i16, ctrl: CtrlNumber, id: *mut ParamID) -> tresult {
        self.note_thread("map");
        if bus == 0 && channel == 0 && i64::from(ctrl) == self.cc.load(Ordering::SeqCst) {
            unsafe { *id = VOLUME_PARAM };
            kResultOk
        } else if self.events && bus == 0 && ctrl == 64 && (0..16).contains(&channel) {
            // SAFETY: IMidiMapping's caller supplies a valid, writable ParamID output pointer.
            unsafe { *id = 100 + channel as u32 };
            kResultOk
        } else {
            kResultFalse
        }
    }
}


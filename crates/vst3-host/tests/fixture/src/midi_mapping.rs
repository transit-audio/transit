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
        } else if let (true, Some(base), 0, true) = (self.events, controller_base(ctrl), bus, (0..16).contains(&channel)) {
            // ADR 0059 R1: the other pedals and the safety controllers, one parameter per channel.
            // SAFETY: IMidiMapping's caller supplies a valid, writable ParamID output pointer.
            unsafe { *id = base + channel as u32 };
            kResultOk
        } else {
            kResultFalse
        }
    }
}

/// The events synth's parameter base for CC 66, 67, 69, 120, 121 and 123 (channel `c` maps to
/// `base + c`); CC 64 keeps its parameters `100 + c`.
pub(crate) fn controller_base(ctrl: CtrlNumber) -> Option<u32> {
    match ctrl {
        66 => Some(200),
        67 => Some(300),
        69 => Some(400),
        120 => Some(500),
        121 => Some(600),
        123 => Some(700),
        _ => None,
    }
}

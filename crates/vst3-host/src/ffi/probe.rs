//! The scan's per-class capability probe (ADR 0064, decision 2.4): create one class's component,
//! initialize it with the host context, count its buses, terminate it. Nothing is activated,
//! processed or shown, and no edit controller is made. Runs in a probe child process.

use super::*;

/// The buses one component declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawBuses {
    pub event_inputs: i32,
    pub event_outputs: i32,
    pub audio_outputs: i32,
}

impl Bundle {
    /// Creates class `cid`'s component, initializes it, reads its event bus counts in both
    /// directions and its audio output bus count, and terminates it.
    pub fn probe_buses(&self, cid: &[u8; 16]) -> Result<RawBuses, Vst3Error> {
        let err = |m: &str| Vst3Error::Instantiate(hex(cid), m.to_string());
        let host = ComWrapper::new(HostContext::default());
        let host_unknown = host.as_com_ref::<sv::IHostApplication>().ok_or_else(|| err("host context"))?.upcast::<FUnknown>().as_ptr();
        let component = self.create::<sv::IComponent>(cid).ok_or_else(|| err("factory cannot create the component"))?;
        // SAFETY: a fresh component, initialized once here; the host context it is given outlives
        // it (`host` is dropped after `component`, at the end of this function).
        if !ok(unsafe { component.initialize(host_unknown) }) {
            return Err(err("component initialize failed"));
        }
        let count = |media: sv::MediaTypes, dir: sv::BusDirections| {
            // SAFETY: the component is initialized ("Called after the plug-in is initialized",
            // VST3 `getBusCount`) and not yet terminated.
            unsafe { component.getBusCount(media as sv::MediaType, dir as sv::BusDirection) }.clamp(0, MAX_BUSES)
        };
        let buses = RawBuses {
            event_inputs: count(sv::MediaTypes_::kEvent, sv::BusDirections_::kInput),
            event_outputs: count(sv::MediaTypes_::kEvent, sv::BusDirections_::kOutput),
            audio_outputs: count(sv::MediaTypes_::kAudio, sv::BusDirections_::kOutput),
        };
        // SAFETY: initialized above and never activated; terminated exactly once, before the
        // component's last reference is released (the drop of `component` below).
        unsafe { component.terminate() };
        drop(component);
        drop(host);
        Ok(buses)
    }
}

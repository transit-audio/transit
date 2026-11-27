//! The scan's per-class capability probe (ADR 0064, decision 2.4): create one class's component,
//! initialize it with the host context, count its buses, terminate it. Nothing is activated,
//! processed or shown, and no edit controller is made. Runs in a probe child process, on its main
//! thread: the thread that loaded the module (`Bundle::load` there runs the module entry on the
//! calling thread when it is the main thread), which VST3 calls the UI thread.

use super::*;

/// The buses one component declares, as it reports them (not clamped: a scalar the probe records,
/// not a number of buffers this host provides; the caller rejects a negative count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawBuses {
    pub event_inputs: i32,
    pub event_outputs: i32,
    pub audio_outputs: i32,
}

impl Bundle {
    /// Creates class `cid`'s component, initializes it, reads its event bus counts in both
    /// directions and its audio output bus count, and terminates it. Call it on the main thread.
    pub fn probe_buses(&self, cid: &[u8; 16]) -> Result<RawBuses, Vst3Error> {
        let err = |m: &str| Vst3Error::Instantiate(hex(cid), m.to_string());
        let host = ComWrapper::new(HostContext::default());
        let host_unknown = host.as_com_ref::<sv::IHostApplication>().ok_or_else(|| err("host context"))?.upcast::<FUnknown>().as_ptr();
        let component = self.create::<sv::IComponent>(cid).ok_or_else(|| err("factory cannot create the component"))?;
        // SAFETY: `component` is our reference to a fresh component, initialized once here, on the
        // probe child's main thread (the UI thread VST3 requires for `initialize`); the host
        // context it is given outlives it (`host` is dropped after `component`, on every path).
        if !ok(unsafe { component.initialize(host_unknown) }) {
            return Err(err("component initialize failed"));
        }
        let count = |media: sv::MediaTypes, dir: sv::BusDirections| {
            // SAFETY: our component reference is initialized ("Called after the plug-in is
            // initialized", VST3 `getBusCount`) and not yet terminated; the call runs on the probe
            // child's main thread, the UI thread VST3 requires; media and direction are values of
            // the SDK's own enums.
            unsafe { component.getBusCount(media as sv::MediaType, dir as sv::BusDirection) }
        };
        let buses = RawBuses {
            event_inputs: count(sv::MediaTypes_::kEvent, sv::BusDirections_::kInput),
            event_outputs: count(sv::MediaTypes_::kEvent, sv::BusDirections_::kOutput),
            audio_outputs: count(sv::MediaTypes_::kAudio, sv::BusDirections_::kOutput),
        };
        // SAFETY: our component reference was initialized above and never activated; it is
        // terminated exactly once, on the probe child's main thread (the thread that initialized
        // it), before our reference is released (the drop of `component` below). Whether the
        // plug-in kept other references of its own is its business; ours is the only one we made.
        unsafe { component.terminate() };
        drop(component);
        drop(host);
        Ok(buses)
    }
}

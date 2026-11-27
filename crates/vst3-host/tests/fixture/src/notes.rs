//! The classes ADR 0064's scan probe is tested with (slice HM0), each in a build of its own so the
//! default fixture and every test counting its classes are unchanged:
//!
//! - feature `notes`: "Fixture Notes" (`Instrument|Synth`: one stereo audio output bus, one event
//!   input bus, one event output bus) and "Fixture Notes Silent" (`Instrument|Synth`: one stereo
//!   audio output bus and one event input bus, no event output bus: no readable note output).
//! - feature `note-effects`: three zero-audio note effects, "Fixture Note Effect A", "B" and "C"
//!   (`Instrument|Synth`, one event input bus, one event output bus, no audio bus, no editor).
//! - feature `probe-failures`: "Fixture Notes" first, then a class whose creation returns an error
//!   ("Fixture Probe Error"), one that ends the process on creation ("Fixture Probe Crash"), one
//!   that never returns from creation ("Fixture Probe Block"), then "Fixture Notes Silent".
//!
//! None of them writes events yet; every output buffer is silent. The blocking class first writes
//! `blocking.mark` into the process's working folder, so a test can see that its probe child is
//! inside the creation before it stops it.

// Each build lists one set, so the ids and descriptors of the other sets go unused in it.
#![allow(dead_code)]

use super::*;

pub(crate) const NOTES_CID: TUID = uid(0x5C0A1D10, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const NOTES_SILENT_CID: TUID = uid(0x5C0A1D11, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const EFFECT_A_CID: TUID = uid(0x5C0A1D12, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const EFFECT_B_CID: TUID = uid(0x5C0A1D13, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const EFFECT_C_CID: TUID = uid(0x5C0A1D14, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_ERROR_CID: TUID = uid(0x5C0A1D15, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_CRASH_CID: TUID = uid(0x5C0A1D16, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_BLOCK_CID: TUID = uid(0x5C0A1D17, 0x11112222, 0x33334444, 0x55556666);

const MODULE: &str = "Audio Module Class";

#[cfg(feature = "notes")]
pub(crate) const CLASSES: [(TUID, &str, &str, &str); 2] =
    [(NOTES_CID, MODULE, "Fixture Notes", "Instrument|Synth"), (NOTES_SILENT_CID, MODULE, "Fixture Notes Silent", "Instrument|Synth")];

#[cfg(all(feature = "note-effects", not(feature = "notes")))]
pub(crate) const CLASSES: [(TUID, &str, &str, &str); 3] = [
    (EFFECT_A_CID, MODULE, "Fixture Note Effect A", "Instrument|Synth"),
    (EFFECT_B_CID, MODULE, "Fixture Note Effect B", "Instrument|Synth"),
    (EFFECT_C_CID, MODULE, "Fixture Note Effect C", "Instrument|Synth"),
];

#[cfg(all(feature = "probe-failures", not(any(feature = "notes", feature = "note-effects"))))]
pub(crate) const CLASSES: [(TUID, &str, &str, &str); 5] = [
    (NOTES_CID, MODULE, "Fixture Notes", "Instrument|Synth"),
    (PROBE_ERROR_CID, MODULE, "Fixture Probe Error", "Fx"),
    (PROBE_CRASH_CID, MODULE, "Fixture Probe Crash", "Instrument|Synth"),
    (PROBE_BLOCK_CID, MODULE, "Fixture Probe Block", "Fx"),
    (NOTES_SILENT_CID, MODULE, "Fixture Notes Silent", "Instrument|Synth"),
];

/// The buses a class declares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Stereo audio out, event in, event out.
    Generator,
    /// Stereo audio out, event in.
    Silent,
    /// Event in, event out, no audio.
    Effect,
}

impl Shape {
    fn audio_out(self) -> bool {
        self != Shape::Effect
    }
    fn event_out(self) -> bool {
        self != Shape::Silent
    }
}

pub(crate) struct Notes {
    shape: Shape,
    _live: Live,
}

impl Class for Notes {
    type Interfaces = (IComponent, IAudioProcessor);
}

/// The object for one of this module's classes; `None` for any other class id (and for the class
/// whose creation fails). The crashing and blocking classes never return.
pub(crate) fn create(cid: &TUID) -> Option<vst3::ComPtr<FUnknown>> {
    let shape = if *cid == NOTES_CID {
        Shape::Generator
    } else if *cid == NOTES_SILENT_CID {
        Shape::Silent
    } else if [EFFECT_A_CID, EFFECT_B_CID, EFFECT_C_CID].contains(cid) {
        Shape::Effect
    } else if *cid == PROBE_CRASH_CID {
        crash()
    } else if *cid == PROBE_BLOCK_CID {
        block()
    } else {
        return None;
    };
    ComWrapper::new(Notes { shape, _live: Live::new(&OBJECTS) }).to_com_ptr::<FUnknown>()
}

/// Ends the process at once, as a plug-in that crashes in its constructor does. Windows: an exit
/// with the access-violation status, not an abort, so no error report or dialog is raised.
fn crash() -> ! {
    #[cfg(windows)]
    // SAFETY: the C library's `_exit`; ends the process without running anything else.
    unsafe {
        _exit(0xC000_0005_u32 as c_int)
    }
    #[cfg(not(windows))]
    std::process::abort()
}

/// Never returns, as a plug-in waiting on a dialog or a licence server does; says so first in
/// `blocking.mark` in the working folder.
fn block() -> ! {
    let _ = std::fs::write("blocking.mark", b"blocking in createInstance");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn event_bus(bus: *mut BusInfo, dir: BusDirection, name: &str) {
    // SAFETY: the host passes a writable BusInfo.
    let b = unsafe { &mut *bus };
    b.mediaType = MediaTypes_::kEvent as MediaType;
    b.direction = dir;
    b.channelCount = 16;
    copy_c16(name, &mut b.name);
    b.busType = BusTypes_::kMain as BusType;
    b.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
}

impl IPluginBaseTrait for Notes {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Notes {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        kNotImplemented
    }
    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }
    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> i32 {
        let out = dir == BusDirections_::kOutput as BusDirection;
        let declared = if media == MediaTypes_::kAudio as MediaType {
            out && self.shape.audio_out()
        } else if media == MediaTypes_::kEvent as MediaType {
            !out || self.shape.event_out()
        } else {
            false
        };
        i32::from(declared)
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        // SAFETY: forwarded from the host's own call.
        if index != 0 || unsafe { self.getBusCount(media, dir) } == 0 {
            return kInvalidArgument;
        }
        if media == MediaTypes_::kAudio as MediaType {
            audio_bus(bus, dir, "Out");
        } else {
            event_bus(bus, dir, if dir == BusDirections_::kInput as BusDirection { "Notes In" } else { "Notes Out" });
        }
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
    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
}

impl IAudioProcessorTrait for Notes {
    unsafe fn setBusArrangements(&self, _i: *mut SpeakerArrangement, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
        let want = i32::from(self.shape.audio_out());
        if n_in != 0 || n_out != want {
            return kResultFalse;
        }
        // SAFETY: the host passes n_out (checked above) output arrangements.
        if want == 1 && unsafe { *outputs } != SpeakerArr::kStereo {
            return kResultFalse;
        }
        kResultTrue
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        if dir != BusDirections_::kOutput as BusDirection || index != 0 || !self.shape.audio_out() {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a writable SpeakerArrangement.
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the host passes a valid ProcessData for this call.
        let d = unsafe { &*data };
        let frames = d.numSamples.max(0) as usize;
        for bus in 0..d.numOutputs.max(0) as usize {
            // SAFETY: numOutputs output buses, each with numChannels channel pointers of `frames`
            // valid samples, owned by the host for this call.
            for out in unsafe { channels(&*d.outputs.add(bus), frames) } {
                out.fill(0.0);
            }
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

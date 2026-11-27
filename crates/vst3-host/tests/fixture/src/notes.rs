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
//! "Fixture Notes" and the note effects write scripted note output and record what the host does
//! (`generator.rs`, ADR 0064 slice HM1); the other classes write nothing, and every audio output
//! buffer is silent. The blocking class first writes `blocking.mark` into the process's working
//! folder, so a test can see that its probe child is inside the creation before it stops it.

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
pub(crate) const BUSES_17_CID: TUID = uid(0x5C0A1D18, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const BUSES_NEGATIVE_CID: TUID = uid(0x5C0A1D19, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_HELPER_CID: TUID = uid(0x5C0A1D1A, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_ESCAPE_CID: TUID = uid(0x5C0A1D1B, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_HELPER_CRASH_CID: TUID = uid(0x5C0A1D1C, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_CHATTER_CID: TUID = uid(0x5C0A1D1D, 0x11112222, 0x33334444, 0x55556666);
pub(crate) const PROBE_LONG_LINE_CID: TUID = uid(0x5C0A1D1E, 0x11112222, 0x33334444, 0x55556666);

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

#[cfg(all(feature = "edges", unix, not(any(feature = "notes", feature = "note-effects", feature = "probe-failures"))))]
pub(crate) const CLASSES: [(TUID, &str, &str, &str); 8] = [
    (BUSES_17_CID, MODULE, "Fixture Buses 17", "Instrument|Synth"),
    (BUSES_NEGATIVE_CID, MODULE, "Fixture Buses Negative", "Fx"),
    (PROBE_HELPER_CID, MODULE, "Fixture Probe Helper", "Fx"),
    (PROBE_ESCAPE_CID, MODULE, "Fixture Probe Escape", "Fx"),
    (PROBE_HELPER_CRASH_CID, MODULE, "Fixture Probe Helper Crash", "Fx"),
    (PROBE_CHATTER_CID, MODULE, "Fixture Probe Chatter", "Fx"),
    (PROBE_LONG_LINE_CID, MODULE, "Fixture Probe Long Line", "Fx"),
    (NOTES_CID, MODULE, "Fixture Notes", "Instrument|Synth"),
];

#[cfg(all(feature = "edges", not(unix), not(any(feature = "notes", feature = "note-effects", feature = "probe-failures"))))]
pub(crate) const CLASSES: [(TUID, &str, &str, &str); 7] = [
    (BUSES_17_CID, MODULE, "Fixture Buses 17", "Instrument|Synth"),
    (BUSES_NEGATIVE_CID, MODULE, "Fixture Buses Negative", "Fx"),
    (PROBE_HELPER_CID, MODULE, "Fixture Probe Helper", "Fx"),
    (PROBE_HELPER_CRASH_CID, MODULE, "Fixture Probe Helper Crash", "Fx"),
    (PROBE_CHATTER_CID, MODULE, "Fixture Probe Chatter", "Fx"),
    (PROBE_LONG_LINE_CID, MODULE, "Fixture Probe Long Line", "Fx"),
    (NOTES_CID, MODULE, "Fixture Notes", "Instrument|Synth"),
];

/// The buses a class declares.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Stereo audio out, event in, event out.
    Generator,
    /// Stereo audio out, event in.
    Silent,
    /// Event in, event out, no audio.
    Effect,
    /// Seventeen audio outputs, event in, event out.
    Buses17,
    /// Audio out, event in, and -1 event output buses (a count no component may give).
    NegativeEvents,
}

impl Shape {
    fn audio_outs(self) -> i32 {
        match self {
            Shape::Effect => 0,
            Shape::Buses17 => 17,
            _ => 1,
        }
    }
    fn audio_out(self) -> bool {
        self.audio_outs() > 0
    }
    fn event_outs(self) -> i32 {
        match self {
            Shape::Silent => 0,
            Shape::NegativeEvents => -1,
            _ => 1,
        }
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
        note_created();
        return crate::generator::create(Shape::Generator);
    } else if *cid == NOTES_SILENT_CID {
        Shape::Silent
    } else if [EFFECT_A_CID, EFFECT_B_CID, EFFECT_C_CID].contains(cid) {
        note_created();
        return crate::generator::create(Shape::Effect);
    } else if *cid == PROBE_CRASH_CID {
        crash()
    } else if *cid == PROBE_BLOCK_CID {
        block()
    } else if *cid == BUSES_17_CID {
        Shape::Buses17
    } else if *cid == BUSES_NEGATIVE_CID {
        Shape::NegativeEvents
    } else if *cid == PROBE_HELPER_CID {
        spawn_helper("helper", false);
        block()
    } else if *cid == PROBE_ESCAPE_CID {
        spawn_helper("escape", true);
        block()
    } else if *cid == PROBE_HELPER_CRASH_CID {
        spawn_helper("reaped", false);
        crash()
    } else if *cid == PROBE_CHATTER_CID {
        spawn_chatter("chatter", true);
        block()
    } else if *cid == PROBE_LONG_LINE_CID {
        long_line()
    } else {
        return None;
    };
    note_created();
    ComWrapper::new(Notes { shape, _live: Live::new(&OBJECTS) }).to_com_ptr::<FUnknown>()
}

/// Appends a line to `created.log` in the working folder: a test sees that a class was created
/// (that its child got as far as loading the plug-in).
fn note_created() {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("created.log") {
        let _ = writeln!(f, "{}", std::process::id());
    }
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

/// Does not return for 300 s, as a plug-in waiting on a dialog or a license server does. First it holds an
/// exclusive lock on `blocking.lock` (released only when the process ends, so a test can wait for
/// its end) and says it is blocking in `blocking.mark`, both in the working folder.
fn block() -> ! {
    let lock = std::fs::File::create("blocking.lock").and_then(|f| f.lock().map(|()| f));
    let _ = std::fs::write("blocking.mark", b"blocking in createInstance");
    // Ends by itself after 300 s, saying so in `blocking.done`, so that a test of containment fails
    // (rather than passes) when nothing killed it, and nothing is left for good.
    std::thread::sleep(std::time::Duration::from_secs(300));
    let _ = std::fs::write("blocking.done", b"ended by itself");
    drop(lock);
    std::process::exit(0)
}

/// Starts a helper process that inherits this process's standard output (the scan's pipe), as a
/// plug-in that starts a license or update helper does: this program again with
/// `--hold-output <name>`, which locks `<name>.lock` and writes its process id to `<name>.ready`
/// once it holds the lock. Waits for that file (the helper's own start, not a clock budget: it
/// gives up after 3000 checks). `escape`: the helper starts in a process group of its own (Linux
/// and macOS), as a process that leaves its parent's group on purpose does.
fn spawn_helper(name: &str, escape: bool) {
    spawn_role("--hold-output", name, escape);
}

/// Starts this program with `--chatter <name>`: like a helper from [`spawn_helper`], and it also
/// writes a line to its standard output (inherited: the scan's pipe) every 10 ms until a write
/// fails, which ends it. `escape` as for [`spawn_helper`].
fn spawn_chatter(name: &str, escape: bool) {
    spawn_role("--chatter", name, escape);
}

/// Writes 5 MiB without a line end to standard output (the scan's pipe), then blocks.
fn long_line() -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().write_all(&vec![b'x'; 5 * 1024 * 1024]);
    let _ = std::io::stdout().flush();
    block()
}

fn spawn_role(role: &str, name: &str, escape: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut c = std::process::Command::new(exe);
    c.arg(role).arg(name).stdin(std::process::Stdio::null());
    #[cfg(unix)]
    if escape {
        use std::os::unix::process::CommandExt as _;
        c.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = escape;
    if c.spawn().is_err() {
        return;
    }
    let ready = format!("{name}.ready");
    for _ in 0..3000 {
        if std::fs::metadata(&ready).is_ok_and(|m| m.len() > 0) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The buses of a class of this module, for `IComponent::getBusCount`.
pub(crate) fn bus_count(shape: Shape, media: MediaType, dir: BusDirection) -> i32 {
    let out = dir == BusDirections_::kOutput as BusDirection;
    if media == MediaTypes_::kAudio as MediaType {
        if out { shape.audio_outs() } else { 0 }
    } else if media == MediaTypes_::kEvent as MediaType {
        if out { shape.event_outs() } else { 1 }
    } else {
        0
    }
}

/// `IComponent::getBusInfo` for a class of this module.
pub(crate) fn bus_info(shape: Shape, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
    if index < 0 || index >= bus_count(shape, media, dir) {
        return kInvalidArgument;
    }
    if media == MediaTypes_::kAudio as MediaType {
        audio_bus(bus, dir, "Out");
    } else {
        event_bus(bus, dir, if dir == BusDirections_::kInput as BusDirection { "Notes In" } else { "Notes Out" });
    }
    kResultOk
}

/// `IAudioProcessor::setBusArrangements` for a class of this module: no audio input, and its
/// audio outputs stereo.
///
/// # Safety
/// `outputs` points to `n_out` arrangements.
pub(crate) unsafe fn set_arrangements(shape: Shape, n_in: i32, outputs: *mut SpeakerArrangement, n_out: i32) -> tresult {
    let want = shape.audio_outs();
    if n_in != 0 || n_out != want {
        return kResultFalse;
    }
    // SAFETY: the host passes n_out (checked above) output arrangements.
    if want >= 1 && unsafe { *outputs } != SpeakerArr::kStereo {
        return kResultFalse;
    }
    kResultTrue
}

/// `IAudioProcessor::getBusArrangement` for a class of this module.
pub(crate) fn get_arrangement(shape: Shape, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
    if dir != BusDirections_::kOutput as BusDirection || index != 0 || !shape.audio_out() {
        return kInvalidArgument;
    }
    // SAFETY: the host passes a writable SpeakerArrangement.
    unsafe { *arr = SpeakerArr::kStereo };
    kResultOk
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
        bus_count(self.shape, media, dir)
    }
    unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
        bus_info(self.shape, media, dir, index, bus)
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
        // SAFETY: forwarded from the host's own call.
        unsafe { set_arrangements(self.shape, n_in, outputs, n_out) }
    }
    unsafe fn getBusArrangement(&self, dir: BusDirection, index: i32, arr: *mut SpeakerArrangement) -> tresult {
        get_arrangement(self.shape, dir, index, arr)
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

/// When the working folder holds `record-loads` (only the containment test creates it), appends
/// "<process id> <what>" to `loaded.log` there: the module was entered or its factory fetched, so
/// that process loaded the plug-in file. Elsewhere it does nothing (no stray files from the many
/// in-process loads of other tests).
pub(crate) fn note_loaded(what: &str) {
    use std::io::Write as _;
    if !std::path::Path::new("record-loads").exists() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("loaded.log") {
        let _ = writeln!(f, "{} {what}", std::process::id());
    }
}

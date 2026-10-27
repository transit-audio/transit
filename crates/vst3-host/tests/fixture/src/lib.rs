//! Test-only VST3 plugin for transit-vst3-host (two audio module classes):
//!
//! - "Fixture Gain" (`Fx`): stereo or mono in → out × gain, with a separate edit controller
//!   (connected through IConnectionPoint, synced through the component state). Params: 7 "Gain"
//!   (plain 0..2, linear, units dB), 3 "Mode" (3 steps: A/B/C), 12 "Freq" (20..20000 Hz, log),
//!   9 hidden, 11 bypass. Starts at gain 0.5 (its state) although the declared default is 1.0.
//!   Reports 3 samples of latency. State: the component stores the gain (one LE f64), the
//!   controller the Freq value (one LE f64). Its controller has an editor view (`IPlugView`,
//!   320×240, no real drawing) supporting NSView/HWND/X11, which scales on request
//!   (`IPlugViewContentScaleSupport`: 320×240 times the factor, issue #101).
//! - "Fixture Synth" (`Instrument|Synth`): a single-component instrument (the component is also
//!   its edit controller). No audio input, one stereo output, one event input; every output
//!   sample is the velocity of the held note (sample-accurate), 0 when none. Poly pressure, and
//!   MIDI CC 7 through `IMidiMapping` (hidden parameter 1), set it to their value too.
//!   For the host-request tests (audit F08, ADR 0012 G7) it takes commands as a component state
//!   to load: `LEARN:<cc>` maps that controller to the volume instead (a "MIDI learn") and calls
//!   `restartComponent(kMidiCCAssignmentChanged)`; `LATENCY:<n>` reports `n` samples after the
//!   next `setActive(true)` and calls `restartComponent(kLatencyChanged)`; `RESTART:<flags>`
//!   calls `restartComponent` with those flags; `REFUSE` makes `setActive(true)` fail from then
//!   on; `EVERY:<command>` runs the command at every `getState`; `ARM:<command>` runs the command
//!   at the next `getState` instead (a main-thread call the host makes while the processor runs elsewhere,
//!   like a learn in the plug-in's own window). Its component state is a report: the OS thread of
//!   the `getState` call (`me=`) and the threads that ran `getMidiControllerAssignment`,
//!   `setActive`, `process` and `getLatencySamples`.
//!
//! Gain's component talks to its controller the way iZotope's plug-ins do (and the SDK's
//! `ComponentBase::allocateMessage`): when connected it allocates an `IMessage` through the host
//! context's `IHostApplication::createInstance`, fills its attribute list, and `notify`s the
//! controller, which checks every value; it also allocates a bare `IAttributeList` there. A host
//! that cannot create them, or a controller that never got the message by the time the host sets
//! its component handler, is a host contract violation.
//!
//! Probe mode (`VST3_FIXTURE_PROBES` set; see "probe mode" below) checks VST3's edit-controller
//! threading rule and reports the Freq values Gain's processor receives.
//!
//! Unload mode (`VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER` set; see "a module whose unload waits"
//! below) gives the module a worker thread that its static destructor waits for, like a real
//! instrument whose unload hung the process exit (issue #104).
//!
//! Scale mode (`VST3_FIXTURE_SCALE`; see `ScaleMode` below) changes how Gain's editor view takes
//! the content scale (issue #105): `after-attach` accepts a factor before `attached` but applies it
//! only when one arrives after, then asks for its scaled size through `resizeView`; `refuse`
//! returns kResultFalse and stays at 320×240.
//!
//! The module checks the host's module lifecycle the way real plugins depend on it (see "module
//! lifecycle" below) and aborts the process when it is broken.
//!
//! Test code: panics are fine here, the host must survive anything anyway.

#![allow(non_snake_case, clippy::missing_safety_doc)]

mod audio_processor;
mod component;
mod connection_point;
mod edit_controller;
mod factory;
mod midi_mapping;
mod plug_view;

use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

const GAIN_CID: TUID = uid(0x5C0A1D00, 0x11112222, 0x33334444, 0x55556666);
const GAIN_CTRL_CID: TUID = uid(0x5C0A1D01, 0x11112222, 0x33334444, 0x55556666);
const SYNTH_CID: TUID = uid(0x5C0A1D02, 0x11112222, 0x33334444, 0x55556666);
const KEYED_CID: TUID = uid(0x5C0A1D04, 0x11112222, 0x33334444, 0x55556666);
const PICKY_CID: TUID = uid(0x5C0A1D05, 0x11112222, 0x33334444, 0x55556666);
const EVENTS_CID: TUID = uid(0x5C0A1D03, 0x11112222, 0x33334444, 0x55556666);

fn copy_c8(src: &str, dst: &mut [c_char]) {
    for (d, s) in dst.iter_mut().zip(src.bytes().chain(std::iter::once(0))) {
        *d = s as c_char;
    }
}

fn copy_c16(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn audio_bus(bus: *mut BusInfo, dir: BusDirection, name: &str) {
    let b = unsafe { &mut *bus };
    b.mediaType = MediaTypes_::kAudio as MediaType;
    b.direction = dir;
    b.channelCount = 2;
    copy_c16(name, &mut b.name);
    b.busType = BusTypes_::kMain as BusType;
    b.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
}

unsafe fn channels<'a>(bus: &AudioBusBuffers, frames: usize) -> Vec<&'a mut [f32]> {
    let n = bus.numChannels.max(0) as usize;
    let table = unsafe { std::slice::from_raw_parts(bus.__field0.channelBuffers32, n) };
    table.iter().map(|&p| unsafe { std::slice::from_raw_parts_mut(p, frames) }).collect()
}

// ---- gain ----------------------------------------------------------------------------------

const HELLO_ID: &std::ffi::CStr = c"FixtureHello";
const HELLO_BLOB: [u8; 5] = [0, 1, 2, 0xfe, 0xff];

/// A host object of class (and interface) `I`, as `IHostApplication::createInstance` makes it.
fn host_create<I: vst3::Interface>(host: &ComRef<'_, IHostApplication>) -> Option<vst3::ComPtr<I>> {
    let mut iid = I::IID.map(|b| b as c_char);
    let mut obj: *mut c_void = std::ptr::null_mut();
    // SAFETY: the host is live, both IDs are readable TUIDs, and obj is a distinct writable output.
    let r = unsafe { host.createInstance(&mut iid, &mut iid, &mut obj) };
    if r != kResultOk && r != kResultTrue {
        return None;
    }
    // SAFETY: successful SDK creation returns the requested interface with one owned reference.
    unsafe { vst3::ComPtr::from_raw(obj.cast::<I>()) }
}

/// The component's greeting to its controller (see the module docs).
fn say_hello(host: &vst3::ComPtr<IHostApplication>, peer: &ComRef<'_, IConnectionPoint>) {
    let host = host.as_com_ref();
    let Some(list) = host_create::<IAttributeList>(&host) else { violation("host createInstance made no IAttributeList") };
    // SAFETY: list is owned here; IDs are terminated literals and n is a writable scalar.
    unsafe {
        assert_eq!(list.setInt(c"n".as_ptr(), -7), kResultOk);
        let mut n = 0i64;
        assert_eq!(list.getInt(c"n".as_ptr(), &mut n), kResultOk);
        assert_eq!(n, -7, "host attribute list int");
        assert_ne!(list.getInt(c"missing".as_ptr(), &mut n), kResultOk, "host attribute list: no such int");
    }
    let Some(msg) = host_create::<IMessage>(&host) else { violation("host createInstance made no IMessage for IConnectionPoint::notify") };
    // SAFETY: msg owns the borrowed attributes; all setter inputs remain live through their
    // copying calls. The live peer borrows msg only during synchronous notify (retention needs AddRef).
    unsafe {
        msg.setMessageID(HELLO_ID.as_ptr());
        let Some(attrs) = ComRef::from_raw(msg.getAttributes()) else { violation("host IMessage without attributes") };
        let name: Vec<TChar> = "Fixture".encode_utf16().chain(Some(0)).collect();
        assert_eq!(attrs.setInt(c"int".as_ptr(), 42), kResultOk);
        assert_eq!(attrs.setFloat(c"float".as_ptr(), 0.25), kResultOk);
        assert_eq!(attrs.setString(c"string".as_ptr(), name.as_ptr()), kResultOk);
        assert_eq!(attrs.setBinary(c"binary".as_ptr(), HELLO_BLOB.as_ptr().cast(), HELLO_BLOB.len() as u32), kResultOk);
        peer.notify(msg.as_ptr());
    }
}

/// Checks the component's greeting (see the module docs).
///
/// # Safety
/// message must be null or a live IMessage borrowed for this call, with no concurrent mutation.
unsafe fn hello_ok(message: *mut IMessage) -> bool {
    // SAFETY: notify forwards its live borrowed message; ComRef rejects null and does not release it.
    let Some(msg) = (unsafe { ComRef::from_raw(message) }) else { return false };
    // SAFETY: msg and its ID/attributes remain live and unmodified during notify. The output
    // buffers have the reported byte sizes; successful getBinary supplies size readable bytes.
    unsafe {
        let id = msg.getMessageID();
        if id.is_null() || std::ffi::CStr::from_ptr(id) != HELLO_ID {
            return false;
        }
        let Some(attrs) = ComRef::from_raw(msg.getAttributes()) else { return false };
        let (mut i, mut f) = (0i64, 0f64);
        let mut s = [0 as TChar; 4];
        let (mut data, mut size): (*const c_void, u32) = (std::ptr::null(), 0);
        attrs.getInt(c"int".as_ptr(), &mut i) == kResultOk
            && i == 42
            && attrs.getFloat(c"float".as_ptr(), &mut f) == kResultOk
            && f == 0.25
            // A 4-unit buffer truncates "Fixture" to "Fix" plus the terminator.
            && attrs.getString(c"string".as_ptr(), s.as_mut_ptr(), std::mem::size_of_val(&s) as u32) == kResultOk
            && s == [b'F' as TChar, b'i' as TChar, b'x' as TChar, 0]
            && attrs.getBinary(c"binary".as_ptr(), &mut data, &mut size) == kResultOk
            && size as usize == HELLO_BLOB.len()
            && std::slice::from_raw_parts(data.cast::<u8>(), size as usize) == HELLO_BLOB
            && attrs.getFloat(c"int".as_ptr(), &mut f) != kResultOk
    }
}

struct GainProcessor {
    /// Normalized gain (plain = 2 × normalized).
    gain: AtomicU64,
    arrangement: AtomicU64,
    /// The host context from `initialize` (alive while the component is initialized).
    host: AtomicPtr<FUnknown>,
    _live: Live,
}

impl Class for GainProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IConnectionPoint);
}

// ---- sidechain (feature `sidechain`) -------------------------------------------------------

/// What the sidechain plug-in writes for a channel whose aux silence flag is set.
const KEY_MARK: f32 = 0.125;

/// "Fixture Sidechain": a stereo effect with a second, auxiliary (`kAux`) stereo input bus (the
/// key, inactive by default). When the aux bus is active, each output channel is the main input
/// times the RMS of the key's same channel over the block, or `KEY_MARK` when the host flagged
/// that key channel silent. "Fixture Sidechain Picky" refuses every bus arrangement, so the host
/// cannot activate the aux bus; it passes the main input through.
struct Keyed {
    picky: bool,
    main: AtomicU64,
    aux: AtomicU64,
    aux_active: AtomicBool,
    _live: Live,
}

impl Keyed {
    fn new(picky: bool) -> Keyed {
        Keyed {
            picky,
            main: AtomicU64::new(SpeakerArr::kStereo),
            aux: AtomicU64::new(SpeakerArr::kStereo),
            aux_active: AtomicBool::new(false),
            _live: Live::new(&OBJECTS),
        }
    }
}

impl Class for Keyed {
    type Interfaces = (IComponent, IAudioProcessor);
}

/// The host's component handler, carried to a plug-in thread (the test keeps the plug-in alive).
#[derive(Clone, Copy)]
struct HandlerPtr(*mut IComponentHandler);
unsafe impl Send for HandlerPtr {}

struct GainController {
    gain: AtomicU64,
    freq: AtomicU64,
    /// A gain (normalized, f64 bits) the controller returns to at every `getState` where its gain
    /// differs, as a preset in the plug-in's own window would, telling the host with
    /// `restartComponent(kParamValuesChanged)`; `u64::MAX`: none. Set by a controller state that
    /// carries a second f64 after the Freq value.
    preset: AtomicU64,
    /// The host's component handler for this controller.
    handler: AtomicPtr<IComponentHandler>,
    /// The component's greeting arrived (see the module docs).
    hello: AtomicBool,
    _live: Live,
}

/// How the editor view takes the content scale (`VST3_FIXTURE_SCALE`, issue #105).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScaleMode {
    /// Scales on every request (issue #101).
    Always,
    /// `after-attach`: accepts a factor before `attached` (kResultTrue) but ignores it; one that
    /// arrives once attached is applied and the view asks for its scaled size through
    /// `resizeView`. The suspected behavior of Battery 4 and NI's Solid Mix Series.
    AfterAttach,
    /// `refuse`: returns kResultFalse and keeps its size.
    Refuse,
}

impl ScaleMode {
    /// Read when a view is made (each test process sets it once).
    fn current() -> ScaleMode {
        match std::env::var("VST3_FIXTURE_SCALE").as_deref() {
            Ok("after-attach") => ScaleMode::AfterAttach,
            Ok("refuse") => ScaleMode::Refuse,
            _ => ScaleMode::Always,
        }
    }
}

struct View {
    attached: std::sync::atomic::AtomicBool,
    /// The content scale the host set (`f32` bits; issue #101), 1.0 until then.
    scale: AtomicU32,
    frame: AtomicPtr<IPlugFrame>,
    mode: ScaleMode,
    _live: Live,
}

impl View {
    fn new() -> View {
        View {
            attached: AtomicBool::new(false),
            scale: AtomicU32::new(1.0f32.to_bits()),
            frame: AtomicPtr::new(std::ptr::null_mut()),
            mode: ScaleMode::current(),
            _live: Live::new(&OBJECTS),
        }
    }

    /// 320×240 at the content scale.
    fn rect(&self) -> ViewRect {
        let s = f32::from_bits(self.scale.load(Ordering::Relaxed));
        ViewRect { left: 0, top: 0, right: (320.0 * s).round() as i32, bottom: (240.0 * s).round() as i32 }
    }
}

impl Class for View {
    type Interfaces = (IPlugView, IPlugViewContentScaleSupport);
}

impl Class for GainController {
    type Interfaces = (IEditController, IConnectionPoint);
}

const GAIN_PARAMS: [(u32, &str, &str, i32, f64, i32); 5] = [
    (7, "Gain", "dB", 0, 0.5, ParameterInfo_::ParameterFlags_::kCanAutomate),
    (9, "Secret", "", 0, 0.0, ParameterInfo_::ParameterFlags_::kIsHidden),
    (3, "Mode", "", 2, 0.0, ParameterInfo_::ParameterFlags_::kIsList),
    (11, "Bypass", "", 1, 0.0, ParameterInfo_::ParameterFlags_::kIsBypass),
    (12, "Freq", "Hz", 0, 0.5, 0),
];

/// With `VST3_FIXTURE_PARAMS=N` (N > 0), Gain's controller also exposes N continuous parameters
/// (ids 1000.., plain `1 + 9 n²`) and "Fine" (id 2000, 300 steps, plain = step index), and its
/// processor reports the number of edit-controller calls made so far and the last Fine value it
/// received (normalized) in the first two samples of the second output channel. For the
/// large-plug-in tests of the host's lazy curve sampling.
fn many() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("VST3_FIXTURE_PARAMS").ok().and_then(|v| v.parse().ok()).unwrap_or(0))
}

/// Parameter `index` of Gain's controller: (id, title, units, stepCount, default, flags).
fn gain_param(index: usize) -> Option<(u32, String, &'static str, i32, f64, i32)> {
    if let Some(&(id, title, units, steps, def, flags)) = GAIN_PARAMS.get(index) {
        return Some((id, title.to_string(), units, steps, def, flags));
    }
    let k = index - GAIN_PARAMS.len();
    let automate = ParameterInfo_::ParameterFlags_::kCanAutomate;
    match k.cmp(&many()) {
        std::cmp::Ordering::Less => Some((1000 + k as u32, format!("Extra {k}"), "", 0, 0.5, automate)),
        std::cmp::Ordering::Equal if many() > 0 => Some((FINE, "Fine".to_string(), "", FINE_STEPS, 0.5, automate)),
        _ => None,
    }
}

/// The 300-step parameter.
const FINE: u32 = 2000;
const FINE_STEPS: i32 = 300;
static FINE_VALUE: AtomicU64 = AtomicU64::new(0);
static LAST_FINE: AtomicU64 = AtomicU64::new(0);
/// Edit-controller calls made so far (any thread).
static CTRL_CALLS: AtomicU64 = AtomicU64::new(0);

/// Gain's normalized → plain conversion (Freq follows the current shape).
fn gain_to_plain(id: u32, n: f64) -> f64 {
    match id {
        7 => 2.0 * n,
        3 => (n * 2.0).round(),
        12 if FREQ_SQUARE.load(Ordering::SeqCst) => 20.0 + 19_980.0 * n * n,
        12 => 20.0 * 1000f64.powf(n),
        FINE => (n * f64::from(FINE_STEPS)).round(),
        1000..FINE => 1.0 + 9.0 * n * n,
        _ => n,
    }
}

/// Gain's plain → normalized conversion (Freq follows the current shape).
fn gain_to_normalized(id: u32, v: f64) -> f64 {
    match id {
        7 => v / 2.0,
        3 => v / 2.0,
        12 if FREQ_SQUARE.load(Ordering::SeqCst) => ((v - 20.0) / 19_980.0).max(0.0).sqrt(),
        12 => (v / 20.0).ln() / 1000f64.ln(),
        FINE => v / f64::from(FINE_STEPS),
        1000..FINE => ((v - 1.0) / 9.0).max(0.0).sqrt(),
        _ => v,
    }
}

// ---- synth ---------------------------------------------------------------------------------

struct Synth {
    level: AtomicU32,
    /// The MIDI controller `IMidiMapping` assigns to the volume (7 until a learn).
    cc: AtomicI64,
    latency: AtomicU32,
    /// Latency to report after the next `setActive(true)` (-1: none).
    next_latency: AtomicI64,
    /// The host's component handler (alive as long as this controller).
    handler: AtomicPtr<IComponentHandler>,
    /// A command for the next `getState`.
    armed: std::sync::Mutex<Option<String>>,
    /// A command for every `getState`.
    every: std::sync::Mutex<Option<String>>,
    /// `setActive(true)` fails.
    refuse: AtomicBool,
    /// The events synth (see [`PROBE_PARAM`]).
    events: bool,
    /// The controller's (editor's) value of [`PROBE_PARAM`], `f64` bits: what the host last told
    /// it with `setParamNormalized`. In the report as `probe_ctrl` (millionths).
    probe_ctrl: AtomicU64,
    /// `probe_ctrl` as the processor last reported it: a change is written into the next block as
    /// `[5, 0, PROBE_PARAM, value * 1e6]`, after the parameter points.
    probe_seen: AtomicU64,
    /// (call, OS thread), each pair once.
    threads: std::sync::Mutex<Vec<(&'static str, usize)>>,
    _live: Live,
}

impl Synth {
    fn new() -> Synth {
        Synth {
            level: AtomicU32::new(0),
            cc: AtomicI64::new(7),
            latency: AtomicU32::new(0),
            next_latency: AtomicI64::new(-1),
            handler: AtomicPtr::new(std::ptr::null_mut()),
            armed: std::sync::Mutex::new(None),
            every: std::sync::Mutex::new(None),
            refuse: AtomicBool::new(false),
            events: false,
            probe_ctrl: AtomicU64::new(0f64.to_bits()),
            probe_seen: AtomicU64::new(0f64.to_bits()),
            threads: std::sync::Mutex::new(Vec::new()),
            _live: Live::new(&OBJECTS),
        }
    }

    fn note_thread(&self, call: &'static str) {
        let me = os_thread();
        let mut t = self.threads.lock().unwrap();
        if !t.contains(&(call, me)) {
            t.push((call, me));
        }
    }

    fn restart(&self, flags: i32) {
        if let Some(h) = unsafe { ComRef::from_raw(self.handler.load(Ordering::SeqCst)) } {
            unsafe { h.restartComponent(flags) };
        }
    }

    /// Runs a command (see the crate docs). False for an unknown one.
    fn command(&self, cmd: &str) -> bool {
        let (verb, arg) = cmd.split_once(':').unwrap_or((cmd, ""));
        match verb {
            "LEARN" => {
                self.cc.store(arg.parse().unwrap_or(7), Ordering::SeqCst);
                self.restart(RestartFlags_::kMidiCCAssignmentChanged as i32);
            }
            "LATENCY" => {
                self.next_latency.store(arg.parse().unwrap_or(0), Ordering::SeqCst);
                self.restart(RestartFlags_::kLatencyChanged as i32);
            }
            "RESTART" => self.restart(arg.parse().unwrap_or(0)),
            "ARM" => *self.armed.lock().unwrap() = Some(arg.to_string()),
            "EVERY" => *self.every.lock().unwrap() = Some(arg.to_string()),
            "REFUSE" => self.refuse.store(true, Ordering::SeqCst),
            _ => return false,
        }
        true
    }

    /// The component "state": the report (see the crate docs).
    fn report(&self) -> String {
        let threads = self.threads.lock().unwrap();
        let of = |call: &str| threads.iter().filter(|(c, _)| *c == call).map(|(_, t)| t.to_string()).collect::<Vec<_>>().join(",");
        format!(
            "me={};map={};active={};process={};get_latency={};latency={};probe_ctrl={}",
            os_thread(),
            of("map"),
            of("active"),
            of("process"),
            of("latency"),
            self.latency.load(Ordering::SeqCst),
            (f64::from_bits(self.probe_ctrl.load(Ordering::SeqCst)) * 1e6).round() as i64
        )
    }
}

impl Class for Synth {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IMidiMapping);
}

/// The synth's hidden parameter that MIDI CC 7 drives.
const VOLUME_PARAM: ParamID = 1;

/// The events synth ("Fixture Events", F10) is the synth that exposes [`PROBE_PARAM`], maps CC 64 on channel `c`
/// to parameter `100 + c`, and writes what it receives into its output instead of a level: one
/// record of four samples per event and per parameter point, in the order it reads them —
/// `[1 on | 2 off | 3 poly pressure, offset, channel * 1000 + pitch, noteId]` and
/// `[4, offset, param id, value * 1e6]`, then `[5, 0, param id, value * 1e6]` when the host gave
/// the controller a new value of [`PROBE_PARAM`] (`setParamNormalized`) since the last block.
/// Integers below 2^24 are exact in `f32`.
/// The parameter the events mode exposes (normalized 0..1).
const PROBE_PARAM: ParamID = 5;

// ---- factory -------------------------------------------------------------------------------

struct Factory {
    _live: Live,
}

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

#[cfg(feature = "sidechain")]
const CLASSES: [(TUID, &str, &str, &str); 2] =
    [(KEYED_CID, "Audio Module Class", "Fixture Sidechain", "Fx"), (PICKY_CID, "Audio Module Class", "Fixture Sidechain Picky", "Fx")];
#[cfg(not(feature = "sidechain"))]
const CLASSES: [(TUID, &str, &str, &str); 4] = [
    (GAIN_CID, "Audio Module Class", "Fixture Gain", "Fx"),
    (GAIN_CTRL_CID, "Component Controller Class", "Fixture Gain", ""),
    (SYNTH_CID, "Audio Module Class", "Fixture Synth", "Instrument|Synth"),
    (EVENTS_CID, "Audio Module Class", "Fixture Events", "Instrument|Synth"),
];

// ---- probe mode ----------------------------------------------------------------------------
//
// With `VST3_FIXTURE_PROBES` set: Gain's controller ends the process (status 3) when it is called
// on a thread that has run Gain's `process` (VST3 allows edit-controller calls on the UI thread
// only); Gain's processor writes the last Freq value it received (normalized) to its second output
// channel instead of the gained input; and setting Mode from the host reshapes Freq's curve to
// `20 + 19980 n²` at C (same endpoints; `20 · 1000ⁿ` at A and B) and tells the host with
// `restartComponent(kParamTitlesChanged)`, as a plug-in whose mode changes a range would.

/// 0: not read yet, 1: off, 2: on.
static PROBES: AtomicU32 = AtomicU32::new(0);
/// OS threads that have run Gain's `process` (0: free slot).
static PROCESS_THREADS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
/// The last Freq point Gain's processor received (f64 bits).
static LAST_FREQ: AtomicU64 = AtomicU64::new(0);
/// Freq's curve shape: `20 · 1000ⁿ` (false) or `20 + 19980 n²` (true).
static FREQ_SQUARE: AtomicBool = AtomicBool::new(false);
/// The component handler the last Gain controller was given (the host's, alive with it).
static HANDLER: AtomicPtr<IComponentHandler> = AtomicPtr::new(std::ptr::null_mut());

fn probes() -> bool {
    match PROBES.load(Ordering::Relaxed) {
        0 => {
            let on = std::env::var_os("VST3_FIXTURE_PROBES").is_some();
            PROBES.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        n => n == 2,
    }
}

fn note_process_thread() {
    let me = os_thread();
    for slot in &PROCESS_THREADS {
        match slot.compare_exchange(0, me, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(t) if t == me => return,
            Err(_) => {}
        }
    }
}

fn ctrl_call(what: &str) {
    CTRL_CALLS.fetch_add(1, Ordering::Relaxed);
    if probes() {
        let me = os_thread();
        if PROCESS_THREADS.iter().any(|t| t.load(Ordering::SeqCst) == me) {
            violation(&format!("edit controller {what} called on the render thread"));
        }
    }
}

fn reshape_freq(square: bool) {
    if FREQ_SQUARE.swap(square, Ordering::SeqCst) == square {
        return;
    }
    if let Some(h) = unsafe { ComRef::from_raw(HANDLER.load(Ordering::SeqCst)) } {
        unsafe { h.restartComponent(RestartFlags_::kParamTitlesChanged as i32) };
    }
}

// ---- module lifecycle ----------------------------------------------------------------------
//
// The contract real plugins rely on: the host runs the module exit once the factory and every
// plugin object are released, on the thread that ran the module entry, and before the process
// tears down the module's statics. Breaking it ends the process with status 3, much as real
// plugins crash (HALion Sonic segfaults in its static destructors when the module exit never ran,
// and in its module exit when that runs from an exit handler after it was used; Massive X in its
// module exit when that runs on another thread than the entry).

/// Module entries minus module exits (they nest, as in the VST3 SDK).
static ENTERED: AtomicI64 = AtomicI64::new(0);
/// The OS thread that ran the outermost module entry.
static ENTRY_THREAD: AtomicUsize = AtomicUsize::new(0);
/// Factories handed out and not yet released.
static FACTORIES: AtomicI64 = AtomicI64::new(0);
/// Plugin objects alive: components, controllers, views.
static OBJECTS: AtomicI64 = AtomicI64::new(0);
static TEARDOWN_REGISTERED: AtomicBool = AtomicBool::new(false);
/// With `VST3_FIXTURE_LATE_STATE` set, the first plugin made sets up state that the process exit
/// tears down, like a static a real plugin initialises lazily; the module exit needs it.
static LATE_STATE_REGISTERED: AtomicBool = AtomicBool::new(false);
static LATE_STATE_GONE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> c_int;
    fn _exit(status: c_int) -> !;
}

/// Counts itself in one of the counters above for as long as it exists.
struct Live(&'static AtomicI64);

impl Live {
    fn new(counter: &'static AtomicI64) -> Live {
        counter.fetch_add(1, Ordering::SeqCst);
        Live(counter)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The first violation any thread reported: later ones (a second thread, a drop-time call) must
/// not hide it.
static FIRST_VIOLATION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Ends the process at once with status 3 (no crash report: one test expects this), naming the
/// first violation.
fn violation(what: &str) -> ! {
    let first = FIRST_VIOLATION.get_or_init(|| what.to_string());
    eprintln!("vst3-fixture: host contract violation: {first}");
    unsafe { _exit(3) }
}

/// The OS id of the calling thread (`std::thread::current` is gone in exit handlers).
fn os_thread() -> usize {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn pthread_self() -> usize;
        }
        unsafe { pthread_self() }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        unsafe { GetCurrentThreadId() as usize }
    }
}

fn module_entry() -> bool {
    // A module entry that hangs (K38): with `VST3_FIXTURE_ENTRY_GATE` set to a folder, the first
    // entry in the process to claim it writes `entered` there and waits while `hold` exists.
    if let Some(gate) = std::env::var_os("VST3_FIXTURE_ENTRY_GATE").map(std::path::PathBuf::from)
        && std::fs::OpenOptions::new().write(true).create_new(true).open(gate.join("claimed")).is_ok()
    {
        let _ = std::fs::write(gate.join("entered"), b"");
        let start = std::time::Instant::now();
        while gate.join("hold").exists() && start.elapsed() < std::time::Duration::from_secs(60) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    if ENTERED.fetch_add(1, Ordering::SeqCst) == 0 {
        ENTRY_THREAD.store(os_thread(), Ordering::SeqCst);
        if std::env::var_os("VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER").is_some() {
            start_worker();
        }
    }
    if !TEARDOWN_REGISTERED.swap(true, Ordering::SeqCst) {
        unsafe { atexit(static_teardown) };
    }
    true
}

/// Called whenever the factory makes a plugin object.
fn set_up_late_state() {
    if std::env::var_os("VST3_FIXTURE_LATE_STATE").is_some() && !LATE_STATE_REGISTERED.swap(true, Ordering::SeqCst) {
        unsafe { atexit(late_state_teardown) };
    }
}

extern "C" fn late_state_teardown() {
    LATE_STATE_GONE.store(true, Ordering::SeqCst);
}

fn module_exit() -> bool {
    let left = ENTERED.fetch_sub(1, Ordering::SeqCst) - 1;
    if left < 0 {
        violation("module exit without a module entry");
    }
    let (factories, objects) = (FACTORIES.load(Ordering::SeqCst), OBJECTS.load(Ordering::SeqCst));
    if left == 0 && (factories != 0 || objects != 0) {
        violation(&format!("module exit with {factories} factories and {objects} plugin objects alive"));
    }
    if left == 0 && os_thread() != ENTRY_THREAD.load(Ordering::SeqCst) {
        violation("module exit on another thread than the module entry");
    }
    if LATE_STATE_GONE.load(Ordering::SeqCst) {
        violation("module exit after the process exit tore down state the module set up while in use");
    }
    if left == 0 {
        stop_worker();
    }
    true
}

// A module whose unload waits for its own worker thread (issue #104), on with
// `VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER`: the outermost module entry starts a worker thread, the
// module exit stops it, and a static destructor (an `atexit` handler of this binary, which runs
// when the binary is unloaded or the process exits) stops it too and waits, with no time limit,
// until the worker says it has stopped. Seen in a real instrument: on Windows the process exit
// kills every other thread before it unloads the binaries, so when the module exit never ran
// the worker is gone, nobody clears the flag and the exit hangs forever.

/// True while the worker thread runs; only the worker clears it.
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static WORKER_STOP: AtomicBool = AtomicBool::new(false);
static WORKER_TEARDOWN_REGISTERED: AtomicBool = AtomicBool::new(false);

fn start_worker() {
    if WORKER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    WORKER_STOP.store(false, Ordering::SeqCst);
    std::thread::spawn(|| {
        while !WORKER_STOP.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        WORKER_RUNNING.store(false, Ordering::SeqCst);
    });
    if !WORKER_TEARDOWN_REGISTERED.swap(true, Ordering::SeqCst) {
        // SAFETY: the C library's `atexit` with its declared signature; `worker_teardown` is a plain
        // function of this binary, which is never unloaded before its exit handlers run.
        unsafe { atexit(worker_teardown) };
    }
}

/// Stops the worker and waits until it has stopped: forever, as the real one does.
fn stop_worker() {
    WORKER_STOP.store(true, Ordering::SeqCst);
    while WORKER_RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

extern "C" fn worker_teardown() {
    stop_worker();
}

/// Stands in for the C++ static destructors a real module registers while it loads.
extern "C" fn static_teardown() {
    if ENTERED.load(Ordering::SeqCst) > 0 && OBJECTS.load(Ordering::SeqCst) == 0 {
        violation("static teardown of a module that is not in use but was never exited");
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    module_entry()
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    module_exit()
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(bundle: *mut c_void) -> bool {
    !bundle.is_null() && module_entry()
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    module_exit()
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleEntry(handle: *mut c_void) -> bool {
    !handle.is_null() && module_entry()
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleExit() -> bool {
    module_exit()
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    match ComWrapper::new(Factory { _live: Live::new(&FACTORIES) }).to_com_ptr::<IPluginFactory>() {
        Some(p) => p.into_raw(),
        None => std::ptr::null_mut(),
    }
}

//! "Fixture Notes" and the zero-audio "Fixture Note Effect" classes (ADR 0064 slice HM1): a
//! single-component class (its own edit controller, no parameters, no view) that writes the note
//! output a test scripts and records what the host does to it.
//!
//! Commands arrive as a component state (text, `;` between commands; the call index counts this
//! instance's `process` calls since the last `CLEAR`):
//!
//! - `CLEAR`: forget the log, the script and the counters.
//! - `EMIT <type> <call|*> <offset> <bus> <a> <b> <c> <d>`: at that call (or every call), write one
//!   output event at `offset` on bus `bus`. Types: `on`, `off` (a = channel, b = pitch, c = note
//!   id, d = velocity), `pp` (d = pressure), `cc` (a legacy controller output event: a = controller
//!   number, b = channel, c = `value`, d = `value2`), `data`, `expr` (a note expression value,
//!   c = note id), `exprint`, `exprtext`, `chord`, `scale`.
//! - `HOLD <call> <offset> <channel> <pitch> <id> <len>`: a note-on there, and its note-off `len`
//!   frames later from the class's own schedule (kept or forgotten at `setProcessing(false)`).
//! - `RETAIN` / `FORGET`: keep the schedule at `setProcessing(false)`, or forget it (the default).
//! - `FLOOD <call> <n>`: write `n` note-ons at offset 0 in that call.
//! - `FAIL <call>`: that call returns `kResultFalse`.
//! - `SETPROC_ERR`: `setProcessing` returns `kInternalError` from now on.
//! - `LATENCY <n>`: report `n` samples after the next `setActive(true)`, and ask for a restart
//!   (`kLatencyChanged`); `RESTART`: ask for one with the latency unchanged.
//! - `REFUSE`: `setActive(true)` fails from now on.
//! - `AT <frame> <type> <bus> <a> <b> <c> <d>`: as `EMIT`, written in the call that holds frame
//!   `frame` of this instance's own time (the frames of its calls since the last `CLEAR`), at that
//!   frame's offset (ADR 0064 HM2: a script independent of how the host splits its blocks).
//! - `ECHO <semitones>`: every input note-on and note-off is written again on bus 0 at its offset,
//!   `semitones` up, with its channel, note id and velocity (a note effect, ADR 0064 HM2).
//!
//! Its component state is the log since the last read, one line per record:
//! `activateBus <media> <direction> <index> <state>`, `setActive <s> -> <0|1>`,
//! `setProcessing <s> -> <result>`,
//! `process <call> n=<frames> in=<numInputs> out=<numOutputs> inptr=<0|1> outptr=<0|1>
//! pts=<projectTimeSamples> ptm=<projectTimeMusic>`, `in <on|off|pp> <offset> <channel> <pitch>
//! <id>`, `point <param> <offset> <value x 1e6>`, `wrote <what> <offset> -> <result>`,
//! `flood <accepted> <refused>`, `fail <call>`. Controller CC 64 on channel c maps to parameter
//! 100 + c (`IMidiMapping`).

use super::notes::{Shape, bus_count, bus_info, get_arrangement, set_arrangements};
use super::*;
use std::sync::Mutex;

#[derive(Clone, Copy)]
struct Emit {
    call: Option<u64>,
    ty: u16,
    offset: i32,
    bus: i32,
    a: i32,
    b: i32,
    c: i32,
    d: f64,
}

#[derive(Default)]
struct State {
    log: Vec<String>,
    emits: Vec<Emit>,
    /// HOLD lines: (call, offset, channel, pitch, id, len).
    holds: Vec<(u64, i32, i16, i16, i32, u64)>,
    /// Scheduled note-offs: (frame, channel, pitch, id).
    due: Vec<(u64, i16, i16, i32)>,
    floods: Vec<(u64, u32)>,
    calls: u64,
    frame: u64,
    fail: Option<u64>,
    /// AT lines: (frame, event).
    ats: Vec<(u64, Emit)>,
    /// ECHO: the transposition, when set.
    echo: Option<i32>,
    setproc_err: bool,
    retain: bool,
    latency: u32,
    next_latency: Option<u32>,
    refuse: bool,
}

pub(crate) struct Gen {
    shape: Shape,
    state: Mutex<State>,
    handler: AtomicPtr<IComponentHandler>,
    _live: Live,
}

impl Class for Gen {
    type Interfaces = (IComponent, IAudioProcessor, IEditController, IMidiMapping);
}

pub(crate) fn create(shape: Shape) -> Option<vst3::ComPtr<FUnknown>> {
    let g = Gen { shape, state: Mutex::new(State::default()), handler: AtomicPtr::new(std::ptr::null_mut()), _live: Live::new(&OBJECTS) };
    ComWrapper::new(g).to_com_ptr::<FUnknown>()
}

fn type_of(name: &str) -> Option<u16> {
    use Event_::EventTypes_ as T;
    Some(match name {
        "on" => T::kNoteOnEvent as u16,
        "off" => T::kNoteOffEvent as u16,
        "pp" => T::kPolyPressureEvent as u16,
        "cc" => T::kLegacyMIDICCOutEvent as u16,
        "data" => T::kDataEvent as u16,
        "expr" => T::kNoteExpressionValueEvent as u16,
        "exprint" => T::kNoteExpressionIntValueEvent as u16,
        "exprtext" => T::kNoteExpressionTextEvent as u16,
        "chord" => T::kChordEvent as u16,
        "scale" => T::kScaleEvent as u16,
        _ => return None,
    })
}

/// The SDK event for an `EMIT` line.
fn event_of(e: &Emit) -> Event {
    use Event_::EventTypes_ as T;
    // SAFETY: `Event` is plain data (numbers and a union of plain structs); all-zero is valid.
    let mut ev: Event = unsafe { std::mem::zeroed() };
    (ev.busIndex, ev.sampleOffset, ev.r#type) = (e.bus, e.offset, e.ty);
    let (ch, pitch) = (e.a as i16, e.b as i16);
    if e.ty == T::kNoteOnEvent as u16 {
        ev.__field0.noteOn = NoteOnEvent { channel: ch, pitch, tuning: 0.0, velocity: e.d as f32, length: 0, noteId: e.c };
    } else if e.ty == T::kNoteOffEvent as u16 {
        ev.__field0.noteOff = NoteOffEvent { channel: ch, pitch, velocity: e.d as f32, noteId: e.c, tuning: 0.0 };
    } else if e.ty == T::kPolyPressureEvent as u16 {
        ev.__field0.polyPressure = PolyPressureEvent { channel: ch, pitch, pressure: e.d as f32, noteId: e.c };
    } else if e.ty == T::kLegacyMIDICCOutEvent as u16 {
        ev.__field0.midiCCOut = LegacyMIDICCOutEvent { controlNumber: e.a as u8, channel: e.b as i8, value: e.c as i8, value2: e.d as i8 };
    } else if e.ty == T::kNoteExpressionValueEvent as u16 {
        ev.__field0.noteExpressionValue = NoteExpressionValueEvent { typeId: 0, noteId: e.c, value: e.d };
    } else if e.ty == T::kNoteExpressionIntValueEvent as u16 {
        ev.__field0.noteExpressionIntValue = NoteExpressionIntValueEvent { typeId: 0, noteId: e.c, value: 0 };
    } else if e.ty == T::kNoteExpressionTextEvent as u16 {
        ev.__field0.noteExpressionText = NoteExpressionTextEvent { typeId: 0, noteId: e.c, textLen: 0, text: std::ptr::null() };
    }
    ev
}

fn note(ty: u16, offset: i32, ch: i16, pitch: i16, id: i32) -> Emit {
    Emit { call: None, ty, offset, bus: 0, a: i32::from(ch), b: i32::from(pitch), c: id, d: 0.5 }
}

impl Gen {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn restart(&self) {
        // SAFETY: the host's component handler, alive as long as this controller.
        if let Some(h) = unsafe { ComRef::from_raw(self.handler.load(Ordering::SeqCst)) } {
            // SAFETY: a live handler; a main-thread call, as the host's state load is.
            unsafe { h.restartComponent(RestartFlags_::kLatencyChanged as i32) };
        }
    }

    /// Runs one command line.
    fn command(&self, line: &str) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let int = |i: usize| w.get(i).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
        let mut st = self.st();
        match w.first().copied() {
            Some("CLEAR") => {
                let (latency, refuse, setproc_err, retain) = (st.latency, st.refuse, st.setproc_err, st.retain);
                *st = State { latency, refuse, setproc_err, retain, ..State::default() };
            }
            Some("EMIT") => {
                let Some(ty) = w.get(1).and_then(|n| type_of(n)) else { return };
                let call = if w.get(2) == Some(&"*") { None } else { Some(int(2) as u64) };
                let d = w.get(8).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                st.emits.push(Emit { call, ty, offset: int(3) as i32, bus: int(4) as i32, a: int(5) as i32, b: int(6) as i32, c: int(7) as i32, d });
            }
            Some("AT") => {
                let Some(ty) = w.get(2).and_then(|n| type_of(n)) else { return };
                let d = w.get(7).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
                let e = Emit { call: None, ty, offset: 0, bus: int(3) as i32, a: int(4) as i32, b: int(5) as i32, c: int(6) as i32, d };
                st.ats.push((int(1) as u64, e));
            }
            Some("ECHO") => st.echo = Some(int(1) as i32),
            Some("HOLD") => st.holds.push((int(1) as u64, int(2) as i32, int(3) as i16, int(4) as i16, int(5) as i32, int(6) as u64)),
            Some("FLOOD") => st.floods.push((int(1) as u64, int(2) as u32)),
            Some("RETAIN") => st.retain = true,
            Some("FORGET") => st.retain = false,
            Some("FAIL") => st.fail = Some(int(1) as u64),
            Some("SETPROC_ERR") => st.setproc_err = true,
            Some("REFUSE") => st.refuse = true,
            Some("LATENCY") => {
                st.next_latency = Some(int(1) as u32);
                drop(st);
                self.restart();
            }
            Some("RESTART") => {
                drop(st);
                self.restart();
            }
            _ => {}
        }
    }

    /// Writes `e` to the output list and logs it with the list's answer.
    fn write(st: &mut State, list: Option<&ComRef<'_, IEventList>>, e: &Emit, what: &str) -> bool {
        let mut ev = event_of(e);
        // SAFETY: a live output list for this process call; `ev` is a valid event on this frame.
        let r = list.map_or(kResultFalse, |l| unsafe { l.addEvent(&mut ev) });
        st.log.push(format!("wrote {what} {} -> {r}", e.offset));
        r == kResultOk
    }
}

impl IPluginBaseTrait for Gen {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }
    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Gen {
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
    unsafe fn activateBus(&self, m: MediaType, d: BusDirection, i: i32, state: TBool) -> tresult {
        self.st().log.push(format!("activateBus {m} {d} {i} {state}"));
        kResultOk
    }
    unsafe fn setActive(&self, state: TBool) -> tresult {
        let mut st = self.st();
        let ok = state == 0 || !st.refuse;
        if state != 0 && ok
            && let Some(l) = st.next_latency.take()
        {
            st.latency = l;
        }
        st.log.push(format!("setActive {state} -> {}", u8::from(ok)));
        if ok { kResultOk } else { kResultFalse }
    }
    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        // SAFETY: the host's stream, live for this call.
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut text = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let mut n = 0;
            // SAFETY: `buf` is writable for its length; `n` is a local.
            if unsafe { s.read(buf.as_mut_ptr().cast(), buf.len() as i32, &mut n) } != kResultOk || n <= 0 {
                break;
            }
            text.extend_from_slice(&buf[..n as usize]);
        }
        for line in String::from_utf8_lossy(&text).split(';') {
            self.command(line.trim());
        }
        kResultOk
    }
    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        // SAFETY: the host's stream, live for this call.
        let Some(s) = (unsafe { ComRef::from_raw(state) }) else { return kInvalidArgument };
        let mut text = std::mem::take(&mut self.st().log).join("\n").into_bytes();
        let mut n = 0;
        // SAFETY: `text` is readable for its length; `n` is a local.
        unsafe { s.write(text.as_mut_ptr().cast(), text.len() as i32, &mut n) }
    }
}

impl IAudioProcessorTrait for Gen {
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
        self.st().latency
    }
    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }
    unsafe fn setProcessing(&self, state: TBool) -> tresult {
        let mut st = self.st();
        if state == 0 && !st.retain {
            st.due.clear();
        }
        let r = if st.setproc_err { kInternalError } else { kResultOk };
        st.log.push(format!("setProcessing {state} -> {r}"));
        r
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the host passes a valid ProcessData for this call.
        let d = unsafe { &*data };
        let frames = d.numSamples.max(0) as usize;
        let mut st = self.st();
        let call = st.calls;
        st.calls += 1;
        // SAFETY: processContext is null or valid for this call.
        let (pts, ptm) = unsafe { d.processContext.as_ref() }.map_or((0, 0.0), |c| (c.projectTimeSamples, c.projectTimeMusic));
        st.log.push(format!(
            "process {call} n={frames} in={} out={} inptr={} outptr={} pts={pts} ptm={ptm:?}",
            d.numInputs,
            d.numOutputs,
            u8::from(!d.inputs.is_null()),
            u8::from(!d.outputs.is_null())
        ));
        let mut echoes: Vec<Emit> = Vec::new();
        // SAFETY: inputEvents is null or a borrowed IEventList valid for this process call.
        if let Some(list) = unsafe { ComRef::from_raw(d.inputEvents) } {
            // SAFETY: the live list borrowed above.
            for i in 0..unsafe { list.getEventCount() } {
                // SAFETY: plain data; getEvent fills it.
                let mut e: Event = unsafe { std::mem::zeroed() };
                // SAFETY: index in the list; `e` is writable.
                if unsafe { list.getEvent(i, &mut e) } != kResultOk {
                    continue;
                }
                use Event_::EventTypes_ as T;
                // SAFETY: each union read matches the event's tag.
                let rec = unsafe {
                    if e.r#type == T::kNoteOnEvent as u16 {
                        Some(("on", e.__field0.noteOn.channel, e.__field0.noteOn.pitch, e.__field0.noteOn.noteId))
                    } else if e.r#type == T::kNoteOffEvent as u16 {
                        Some(("off", e.__field0.noteOff.channel, e.__field0.noteOff.pitch, e.__field0.noteOff.noteId))
                    } else if e.r#type == T::kPolyPressureEvent as u16 {
                        Some(("pp", e.__field0.polyPressure.channel, e.__field0.polyPressure.pitch, e.__field0.polyPressure.noteId))
                    } else {
                        None
                    }
                };
                if let Some((what, ch, pitch, id)) = rec {
                    st.log.push(format!("in {what} {} {ch} {pitch} {id}", e.sampleOffset));
                    if let (Some(shift), "on" | "off") = (st.echo, what) {
                        // SAFETY: each union read matches the event's tag (checked above).
                        let velocity = unsafe { if what == "on" { e.__field0.noteOn.velocity } else { e.__field0.noteOff.velocity } };
                        let ty = if what == "on" { T::kNoteOnEvent as u16 } else { T::kNoteOffEvent as u16 };
                        let a = i32::from(ch);
                        echoes.push(Emit { call: None, ty, offset: e.sampleOffset, bus: 0, a, b: i32::from(pitch) + shift, c: id, d: f64::from(velocity) });
                    }
                }
            }
        }
        // SAFETY: inputParameterChanges is null or valid for this call.
        if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            // SAFETY: the live interface borrowed above.
            for i in 0..unsafe { changes.getParameterCount() } {
                // SAFETY: index in the list; its queue stays valid for this call.
                let Some(q) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else { continue };
                // SAFETY: a live queue.
                let pid = unsafe { q.getParameterId() };
                // SAFETY: a live queue.
                for k in 0..unsafe { q.getPointCount() } {
                    let (mut at, mut v) = (0i32, 0f64);
                    // SAFETY: index in the queue; `at` and `v` are writable.
                    if unsafe { q.getPoint(k, &mut at, &mut v) } == kResultOk {
                        st.log.push(format!("point {pid} {at} {}", (v * 1e6).round() as i64));
                    }
                }
            }
        }
        for bus in 0..d.numOutputs.max(0) as usize {
            // SAFETY: numOutputs output buses, each with numChannels channel pointers of `frames`
            // valid samples, owned by the host for this call.
            for out in unsafe { channels(&*d.outputs.add(bus), frames) } {
                out.fill(0.0);
            }
        }
        // SAFETY: outputEvents is null or a borrowed IEventList valid for this process call.
        let out = unsafe { ComRef::from_raw(d.outputEvents) };
        let emits: Vec<Emit> = st.emits.iter().filter(|e| e.call.is_none_or(|c| c == call)).copied().collect();
        for e in &emits {
            Self::write(&mut st, out.as_ref(), e, "emit");
        }
        for e in &echoes {
            Self::write(&mut st, out.as_ref(), e, "echo");
        }
        let (start, end) = (st.frame, st.frame + frames as u64);
        let ats: Vec<Emit> = st.ats.iter().filter(|(f, _)| *f >= start && *f < end).map(|(f, e)| Emit { offset: (f - start) as i32, ..*e }).collect();
        for e in &ats {
            Self::write(&mut st, out.as_ref(), e, "at");
        }
        use Event_::EventTypes_ as T;
        let holds: Vec<_> = st.holds.iter().filter(|h| h.0 == call).copied().collect();
        for (_, offset, ch, pitch, id, len) in holds {
            if Self::write(&mut st, out.as_ref(), &note(T::kNoteOnEvent as u16, offset, ch, pitch, id), "hold-on") {
                let at = st.frame + offset.max(0) as u64 + len;
                st.due.push((at, ch, pitch, id));
            }
        }
        let (from, to) = (st.frame, st.frame + frames as u64);
        let due: Vec<_> = st.due.iter().filter(|x| x.0 >= from && x.0 < to).copied().collect();
        st.due.retain(|x| !(x.0 >= from && x.0 < to));
        for (at, ch, pitch, id) in due {
            Self::write(&mut st, out.as_ref(), &note(T::kNoteOffEvent as u16, (at - from) as i32, ch, pitch, id), "hold-off");
        }
        let floods: Vec<_> = st.floods.iter().filter(|f| f.0 == call).copied().collect();
        for (_, n) in floods {
            let (mut yes, mut no) = (0, 0);
            for k in 0..n {
                let mut ev = event_of(&note(T::kNoteOnEvent as u16, 0, 0, (k % 128) as i16, k as i32));
                // SAFETY: a live output list; `ev` lives on this frame.
                if out.as_ref().is_some_and(|l| unsafe { l.addEvent(&mut ev) } == kResultOk) {
                    yes += 1;
                } else {
                    no += 1;
                }
            }
            st.log.push(format!("flood {yes} {no}"));
        }
        st.frame = to;
        if st.fail == Some(call) {
            st.log.push(format!("fail {call}"));
            return kResultFalse;
        }
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IEditControllerTrait for Gen {
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
        0
    }
    unsafe fn getParameterInfo(&self, _index: i32, _info: *mut ParameterInfo) -> tresult {
        kInvalidArgument
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
    unsafe fn getParamNormalized(&self, _id: u32) -> f64 {
        0.0
    }
    unsafe fn setParamNormalized(&self, _id: u32, _v: f64) -> tresult {
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

impl IMidiMappingTrait for Gen {
    unsafe fn getMidiControllerAssignment(&self, bus: i32, channel: i16, ctrl: CtrlNumber, id: *mut ParamID) -> tresult {
        if bus == 0 && ctrl == 64 && (0..16).contains(&channel) {
            // SAFETY: the caller supplies a valid, writable ParamID output pointer.
            unsafe { *id = 100 + channel as u32 };
            kResultOk
        } else {
            kResultFalse
        }
    }
}

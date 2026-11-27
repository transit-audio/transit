//! "Fixture Notes" and the zero-audio "Fixture Note Effect" plug-ins (ADR 0064 slice HM1): they
//! write the note output a test scripts and record what the host does to them, through
//! `clap.state` (and report a latency through `clap.latency`).
//!
//! Commands arrive as a state to load (text, `;` between commands; the call index counts this
//! instance's `process` calls since the last `CLEAR`):
//!
//! - `CLEAR`: forget the log, the script and the counters.
//! - `EMIT <type> <call|*> <time> <port> <channel> <key> <id> <value>`: at that call (or every
//!   call), push one output event at `time` (written as `u32`, so -1 is 2^32 - 1). Types: `on`,
//!   `off`, `choke`, `end` (note events, value = velocity), `pexpr` (a pressure note expression),
//!   `texpr` (a tuning one), `midi` (channel, key and id are the three bytes), `sysex`, `midi2`,
//!   `param` (a parameter value: channel is the parameter id).
//! - `FLOOD <call> <n>`: push `n` note-ons on the read port at time 0 in that call.
//! - `FAIL <call>`: that call returns `CLAP_PROCESS_ERROR`.
//! - `LATENCY <n>`: report `n` samples from the next activation and ask for a restart;
//!   `RESTART`: ask for one with the latency unchanged; `REFUSE`: `activate` fails from now on.
//! - `DIALECT_MIDI_ONLY`: from the next activation, input note port 0 offers only the MIDI
//!   dialect (it offers CLAP and MIDI before).
//!
//! Its state is the log since the last read, one line per record: `activate -> <0|1>`,
//! `deactivate`, `start`, `stop`, `reset`, `process <call> n=<frames> ain=<count> aout=<count>
//! beats=<song_pos_beats> secs=<song_pos_seconds>`, `in <on|off|choke|midi|expr|param> <time> ...`,
//! `wrote <what> <time> -> <true|false>`, `flood <accepted> <refused>`, `fail <call>`.

use super::notes::{AUDIO, NOTE_PORTS, Note, note};
use super::*;

#[derive(Clone, Copy)]
struct Emit {
    call: Option<u64>,
    what: u8,
    time: u32,
    port: i16,
    channel: i16,
    key: i16,
    id: i32,
    value: f64,
}

const ON: u8 = 0;
const OFF: u8 = 1;
const CHOKE: u8 = 2;
const END: u8 = 3;
const PEXPR: u8 = 4;
const TEXPR: u8 = 5;
const MIDI: u8 = 6;
const SYSEX: u8 = 7;
const MIDI2: u8 = 8;
const PARAM: u8 = 9;

#[derive(Default)]
pub(crate) struct State {
    log: Vec<String>,
    emits: Vec<Emit>,
    floods: Vec<(u64, u32)>,
    calls: u64,
    fail: Option<u64>,
    latency: u32,
    next_latency: Option<u32>,
    refuse: bool,
    midi_only_next: bool,
    midi_only: bool,
}

fn st(n: &Note) -> std::sync::MutexGuard<'_, State> {
    n.gen_state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Gives a generator's vtable its own lifecycle and process calls.
pub(crate) fn install(p: &mut clap_plugin) {
    p.activate = Some(activate);
    p.deactivate = Some(deactivate);
    p.start_processing = Some(start_processing);
    p.stop_processing = Some(stop_processing);
    p.reset = Some(reset);
    p.process = Some(process);
    p.get_extension = Some(get_extension);
}

/// Input note port 0 offers only MIDI in the current activation.
pub(crate) fn input_midi_only(n: &Note) -> bool {
    st(n).midi_only
}

fn what_of(name: &str) -> Option<u8> {
    Some(match name {
        "on" => ON,
        "off" => OFF,
        "choke" => CHOKE,
        "end" => END,
        "pexpr" => PEXPR,
        "texpr" => TEXPR,
        "midi" => MIDI,
        "sysex" => SYSEX,
        "midi2" => MIDI2,
        "param" => PARAM,
        _ => return None,
    })
}

fn request_restart(n: &Note) {
    // SAFETY: the host this plug-in was created with, alive as long as it; `request_restart` is
    // thread-safe.
    unsafe {
        if let Some(f) = (*n.host).request_restart {
            f(n.host);
        }
    }
}

fn command(n: &Note, line: &str) {
    let w: Vec<&str> = line.split_whitespace().collect();
    let int = |i: usize| w.get(i).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let mut s = st(n);
    match w.first().copied() {
        Some("CLEAR") => {
            let (latency, refuse, midi_only, midi_only_next) = (s.latency, s.refuse, s.midi_only, s.midi_only_next);
            *s = State { latency, refuse, midi_only, midi_only_next, ..State::default() };
        }
        Some("EMIT") => {
            let Some(what) = w.get(1).and_then(|x| what_of(x)) else { return };
            let call = if w.get(2) == Some(&"*") { None } else { Some(int(2) as u64) };
            let value = w.get(8).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
            s.emits.push(Emit {
                call,
                what,
                time: int(3) as u32,
                port: int(4) as i16,
                channel: int(5) as i16,
                key: int(6) as i16,
                id: int(7) as i32,
                value,
            });
        }
        Some("FLOOD") => s.floods.push((int(1) as u64, int(2) as u32)),
        Some("FAIL") => s.fail = Some(int(1) as u64),
        Some("REFUSE") => s.refuse = true,
        Some("DIALECT_MIDI_ONLY") => s.midi_only_next = true,
        Some("LATENCY") => {
            s.next_latency = Some(int(1) as u32);
            drop(s);
            request_restart(n);
        }
        Some("RESTART") => {
            drop(s);
            request_restart(n);
        }
        _ => {}
    }
}

unsafe extern "C" fn activate(p: *const clap_plugin, _sr: f64, _min: u32, _max: u32) -> bool {
    // SAFETY: the host passes a live plug-in of this module.
    let n = unsafe { note(p) };
    let mut s = st(n);
    let ok = !s.refuse;
    if ok {
        if let Some(l) = s.next_latency.take() {
            s.latency = l;
        }
        if std::mem::take(&mut s.midi_only_next) {
            s.midi_only = true;
        }
    }
    s.log.push(format!("activate -> {}", u8::from(ok)));
    ok
}
unsafe extern "C" fn deactivate(p: *const clap_plugin) {
    // SAFETY: as above.
    st(unsafe { note(p) }).log.push("deactivate".into());
}
unsafe extern "C" fn start_processing(p: *const clap_plugin) -> bool {
    // SAFETY: as above.
    st(unsafe { note(p) }).log.push("start".into());
    true
}
unsafe extern "C" fn stop_processing(p: *const clap_plugin) {
    // SAFETY: as above.
    st(unsafe { note(p) }).log.push("stop".into());
}
unsafe extern "C" fn reset(p: *const clap_plugin) {
    // SAFETY: as above.
    st(unsafe { note(p) }).log.push("reset".into());
}

fn header(size: usize, type_: u16, time: u32) -> clap_event_header {
    clap_event_header { size: size as u32, time, space_id: CLAP_CORE_EVENT_SPACE_ID, type_, flags: 0 }
}

/// Pushes `e`; false when the host refused it.
fn push(out: &clap_output_events, e: &Emit) -> bool {
    let Some(try_push) = out.try_push else { return false };
    let note = |t: u16| clap_event_note {
        header: header(size_of::<clap_event_note>(), t, e.time),
        note_id: e.id,
        port_index: e.port,
        channel: e.channel,
        key: e.key,
        velocity: e.value,
    };
    let expr = |x: i32| clap_event_note_expression {
        header: header(size_of::<clap_event_note_expression>(), CLAP_EVENT_NOTE_EXPRESSION, e.time),
        expression_id: x,
        note_id: e.id,
        port_index: e.port,
        channel: e.channel,
        key: e.key,
        value: e.value,
    };
    // SAFETY (every call below): the host's output list for this process call, and an event
    // that lives on this frame for the call.
    unsafe {
        match e.what {
            ON | OFF | CHOKE | END => {
                let t = [CLAP_EVENT_NOTE_ON, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_END][e.what as usize];
                let ev = note(t);
                try_push(out, &ev.header)
            }
            PEXPR | TEXPR => {
                let ev = expr(if e.what == PEXPR { CLAP_NOTE_EXPRESSION_PRESSURE } else { CLAP_NOTE_EXPRESSION_TUNING });
                try_push(out, &ev.header)
            }
            MIDI => {
                let ev = clap_event_midi {
                    header: header(size_of::<clap_event_midi>(), CLAP_EVENT_MIDI, e.time),
                    port_index: e.port as u16,
                    data: [e.channel as u8, e.key as u8, e.id as u8],
                };
                try_push(out, &ev.header)
            }
            SYSEX => {
                let bytes = [0xF0u8, 0x7D, 0xF7];
                let ev = clap_event_midi_sysex {
                    header: header(size_of::<clap_event_midi_sysex>(), CLAP_EVENT_MIDI_SYSEX, e.time),
                    port_index: e.port as u16,
                    buffer: bytes.as_ptr(),
                    size: 3,
                };
                try_push(out, &ev.header)
            }
            MIDI2 => {
                let ev = clap_event_midi2 { header: header(size_of::<clap_event_midi2>(), CLAP_EVENT_MIDI2, e.time), port_index: e.port as u16, data: [0; 4] };
                try_push(out, &ev.header)
            }
            _ => {
                let ev = clap_event_param_value {
                    header: header(size_of::<clap_event_param_value>(), CLAP_EVENT_PARAM_VALUE, e.time),
                    param_id: e.channel as u32,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value: e.value,
                };
                try_push(out, &ev.header)
            }
        }
    }
}

const WHAT: [&str; 10] = ["on", "off", "choke", "end", "pexpr", "texpr", "midi", "sysex", "midi2", "param"];

/// Logs one input event the host gave.
fn log_input(s: &mut State, h: &clap_event_header) {
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return;
    }
    // SAFETY (each cast): the header's type says which complete event it heads.
    let line = unsafe {
        match h.type_ {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
                let e = &*(h as *const clap_event_header).cast::<clap_event_note>();
                let w = if h.type_ == CLAP_EVENT_NOTE_ON { "on" } else { "off" };
                format!("in {w} {} {} {} {} {}", h.time, e.port_index, e.channel, e.key, e.note_id)
            }
            CLAP_EVENT_MIDI => {
                let e = &*(h as *const clap_event_header).cast::<clap_event_midi>();
                format!("in midi {} {} {} {}", h.time, e.data[0], e.data[1], e.data[2])
            }
            CLAP_EVENT_NOTE_EXPRESSION => format!("in expr {}", h.time),
            CLAP_EVENT_PARAM_VALUE => format!("in param {}", h.time),
            _ => return,
        }
    };
    s.log.push(line);
}

unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> clap_process_status {
    // SAFETY: the host passes a live plug-in of this module and a valid process struct.
    let (n, pr) = unsafe { (note(p), &*pr) };
    let mut s = st(n);
    let call = s.calls;
    s.calls += 1;
    // SAFETY: the transport is null or valid for this call.
    let (beats, secs) = unsafe { pr.transport.as_ref() }.map_or((0, 0), |t| (t.song_pos_beats, t.song_pos_seconds));
    s.log.push(format!("process {call} n={} ain={} aout={} beats={beats} secs={secs}", pr.frames_count, pr.audio_inputs_count, pr.audio_outputs_count));
    // SAFETY: the host's input list, valid for this call.
    if let Some(list) = unsafe { pr.in_events.as_ref() } {
        // SAFETY: the list's own functions, called with the list.
        let count = list.size.map_or(0, |f| unsafe { f(list) });
        for i in 0..count {
            // SAFETY: index below the size; the header stays valid for this call.
            let h = list.get.map_or(std::ptr::null(), |f| unsafe { f(list, i) });
            // SAFETY: null or a valid header.
            if let Some(h) = unsafe { h.as_ref() } {
                log_input(&mut s, h);
            }
        }
    }
    for i in 0..pr.audio_outputs_count as usize {
        // SAFETY: `audio_outputs_count` buffers, each with `channel_count` channels of
        // `frames_count` samples, owned by the host for this call.
        unsafe {
            let out = &*pr.audio_outputs.add(i);
            for c in 0..out.channel_count as usize {
                std::slice::from_raw_parts_mut(*out.data32.add(c), pr.frames_count as usize).fill(0.0);
            }
        }
    }
    // SAFETY: the host's output list, valid for this call.
    if let Some(out) = unsafe { pr.out_events.as_ref() } {
        let emits: Vec<Emit> = s.emits.iter().filter(|e| e.call.is_none_or(|c| c == call)).copied().collect();
        for e in &emits {
            let ok = push(out, e);
            s.log.push(format!("wrote {} {} -> {ok}", WHAT[e.what as usize], e.time));
        }
        let floods: Vec<_> = s.floods.iter().filter(|f| f.0 == call).copied().collect();
        for (_, count) in floods {
            let (mut yes, mut no) = (0, 0);
            for k in 0..count {
                let e = Emit { call: None, what: ON, time: 0, port: if n.shape == Shape::Generator { 1 } else { 0 }, channel: 0, key: (k % 128) as i16, id: k as i32, value: 0.5 };
                if push(out, &e) {
                    yes += 1;
                } else {
                    no += 1;
                }
            }
            s.log.push(format!("flood {yes} {no}"));
        }
    }
    if s.fail == Some(call) {
        s.log.push(format!("fail {call}"));
        return CLAP_PROCESS_ERROR;
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    // SAFETY: the host passes a live plug-in of this module and a valid stream.
    let (n, s) = unsafe { (note(p), &*stream) };
    let text = std::mem::take(&mut st(n).log).join("\n");
    if text.is_empty() {
        return true;
    }
    // SAFETY: the stream's own write, with a buffer of `len` readable bytes.
    let w = s.write.map_or(-1, |f| unsafe { f(stream, text.as_ptr().cast(), text.len() as u64) });
    w == text.len() as i64
}

unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    // SAFETY: the host passes a live plug-in of this module and a valid stream.
    let (n, s) = unsafe { (note(p), &*stream) };
    let mut text = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: the stream's own read, into a writable buffer of its length.
        let r = s.read.map_or(-1, |f| unsafe { f(stream, buf.as_mut_ptr().cast(), buf.len() as u64) });
        if r <= 0 {
            break;
        }
        text.extend_from_slice(&buf[..r as usize]);
    }
    for line in String::from_utf8_lossy(&text).split(';') {
        command(n, line.trim());
    }
    true
}

static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn latency_get(p: *const clap_plugin) -> u32 {
    // SAFETY: the host passes a live plug-in of this module.
    st(unsafe { note(p) }).latency
}

static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn get_extension(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    // SAFETY: the host passes a NUL-terminated extension id.
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

use super::notes::Shape;

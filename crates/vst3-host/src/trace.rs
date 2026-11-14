//! The host call trace (#651): every call between the host and a plug-in that can bear on the
//! plug-in's own "modified" state, written to the log while debug logging is on (Help > Debug
//! Logging, `--verbose`, or a `RUST_LOG` that reaches debug).
//!
//! Off, a call site costs one relaxed atomic load ([`on`]: the log crate's maximum level). On,
//! each call is one fixed-size record pushed into a lock-free, allocation-free ring ([`record`]),
//! from any thread, the audio thread included; the instance's main thread drains the ring and
//! writes the lines ([`drain`], from `service_requests` and each editor handle's idle). Every
//! record carries a sequence number taken when the call happened, so the lines keep the true
//! order of the calls even across threads. A run of more than [`BURST`] records of one call for
//! one parameter of one instance within a drain (automation, a knob drag) is written as its first
//! [`BURST`] records and one line with the count and the last value. A full ring drops records and
//! the next drain says how many.
//!
//! Line format (target `transit_vst3_host::trace`):
//! `vst3 #<seq> inst=<instance> thread=<ui|service|audio|other> <call> ...`. Values are written as
//! the shortest decimal that reads back to the same `f64` and as the raw bits.

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// The log target of the trace lines (a `transit` target: Help > Debug Logging raises it).
pub(crate) const TARGET: &str = "transit_vst3_host::trace";
/// Records the ring holds between two drains.
const CAPACITY: usize = 4096;
/// Records of one call for one parameter of one instance written per drain before collapsing.
pub(crate) const BURST: usize = 8;

/// Debug logging is on. One relaxed atomic load, so it is the whole cost of a call site while
/// the trace is off.
#[inline]
pub(crate) fn on() -> bool {
    log::max_level() >= log::LevelFilter::Debug
}

/// Where a call happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Thread {
    /// The instance's main thread, outside the request service (editor handle, state reader).
    Ui = 0,
    /// The instance's main thread, servicing the plug-in's requests (`Shared::service`).
    Service = 1,
    /// The owner side: the `Plugin` calls (`set_param`, `process`, `prepare`, `load_state`),
    /// normally the audio thread.
    Audio = 2,
    /// Any other thread (a plug-in calling back from a thread of its own).
    Other = 3,
}

impl Thread {
    fn name(self) -> &'static str {
        match self {
            Thread::Ui => "ui",
            Thread::Service => "service",
            Thread::Audio => "audio",
            Thread::Other => "other",
        }
    }

    fn from_u8(v: u8) -> Thread {
        match v {
            0 => Thread::Ui,
            1 => Thread::Service,
            2 => Thread::Audio,
            _ => Thread::Other,
        }
    }
}

thread_local! {
    /// The context a [`scope`] set on this thread (`u8::MAX`: none).
    static CONTEXT: Cell<u8> = const { Cell::new(u8::MAX) };
    /// This thread created an instance (its main thread).
    static MAIN: Cell<bool> = const { Cell::new(false) };
}

/// Restores the previous context when dropped.
pub(crate) struct Scope(u8);

impl Drop for Scope {
    fn drop(&mut self) {
        CONTEXT.with(|c| c.set(self.0));
    }
}

/// Marks the rest of the caller's block as running in context `t` (only while the trace is on;
/// `None` otherwise, which costs nothing more than [`on`]).
#[inline]
pub(crate) fn scope(t: Thread) -> Option<Scope> {
    on().then(|| Scope(CONTEXT.with(|c| c.replace(t as u8))))
}

/// The calling thread creates an instance: it is that instance's main thread. Also makes the ring
/// here, so the audio side never allocates it.
pub(crate) fn mark_main() {
    MAIN.with(|m| m.set(true));
    ring();
}

fn current() -> Thread {
    match CONTEXT.with(Cell::get) {
        u8::MAX if MAIN.with(Cell::get) => Thread::Ui,
        u8::MAX => Thread::Other,
        v => Thread::from_u8(v),
    }
}

/// A new instance number for the trace (1, 2, …; 0 is "no instance").
pub(crate) fn next_instance() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The calls the trace knows. `id` is a parameter id where the call has one; `value` an `f64`
/// value; `aux` a size, flags, an offset or a plain value (see [`line`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Call {
    // Plug-in to host.
    BeginEdit = 0,
    PerformEdit,
    EndEdit,
    /// `aux`: the flags.
    RestartComponent,
    /// `aux`: the state (0 or 1).
    SetDirty,
    RequestOpenEditor,
    StartGroupEdit,
    FinishGroupEdit,
    /// `id`: the unit.
    NotifyUnitSelection,
    /// `id`: the list, `aux`: the program index.
    NotifyProgramListChange,
    // Host to plug-in.
    /// `aux`: 0 the host's own value (automation, a mapped control, before a save), 1 to show it.
    SetParamNormalized,
    /// `aux`: the sample offset in its queue.
    ProcessorPoint,
    /// `aux`: bytes; `id`: 1 when the plug-in accepted it.
    ComponentSetState,
    ComponentGetState,
    ControllerSetComponentState,
    ControllerSetState,
    ControllerGetState,
    /// `aux`: the state (0 or 1).
    SetActive,
    /// `aux`: the state (0 or 1).
    SetProcessing,
    // What the host made of it.
    /// The host handed the session the plug-in's value: `value` normalized, `aux` the plain `f32`
    /// bits the session got.
    Report,
    /// `set_param` recognized the session's echo of a report: `value` goes to the processor,
    /// nothing to the controller. `aux`: the plain `f32` bits.
    Echo,
    /// `set_param` took a value of the host's own: `value` (normalized) goes to the processor and
    /// to the controller. `aux`: the plain `f32` bits.
    HostValue,
    /// A (re)activation or a curve re-sample re-sends a value the host set (the `set_param` line
    /// that follows says how).
    Resend,
    /// `set_param` repeated the echo the host already holds: nothing is sent. `aux`: plain bits.
    EchoRepeat,
    /// A re-send left out: the plug-in reported a newer value the session has not echoed yet.
    ResendSkipped,
}

impl Call {
    fn from_u8(v: u8) -> Option<Call> {
        use Call::*;
        const ALL: [Call; 25] = [
            BeginEdit,
            PerformEdit,
            EndEdit,
            RestartComponent,
            SetDirty,
            RequestOpenEditor,
            StartGroupEdit,
            FinishGroupEdit,
            NotifyUnitSelection,
            NotifyProgramListChange,
            SetParamNormalized,
            ProcessorPoint,
            ComponentSetState,
            ComponentGetState,
            ControllerSetComponentState,
            ControllerSetState,
            ControllerGetState,
            SetActive,
            SetProcessing,
            Report,
            Echo,
            HostValue,
            Resend,
            EchoRepeat,
            ResendSkipped,
        ];
        ALL.get(usize::from(v)).copied()
    }
}

/// One drained record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Record {
    pub seq: u64,
    pub inst: u32,
    pub thread: Thread,
    pub call: Call,
    pub id: u32,
    pub value: f64,
    pub aux: u64,
}

/// A bounded multi-producer queue of fixed-size records (each cell's sequence number says whose
/// turn it is), all atomics: no lock, no allocation after creation.
struct Ring {
    cells: Box<[Cell5]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicU64,
    /// One drainer at a time.
    drain: Mutex<()>,
}

struct Cell5 {
    turn: AtomicUsize,
    words: [AtomicU64; 5],
}

impl Ring {
    fn new(n: usize) -> Ring {
        Ring {
            cells: (0..n).map(|i| Cell5 { turn: AtomicUsize::new(i), words: std::array::from_fn(|_| AtomicU64::new(0)) }).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            drain: Mutex::new(()),
        }
    }

    /// Any thread. False (and counted) when the ring is full.
    fn push(&self, words: [u64; 5]) -> bool {
        let n = self.cells.len();
        let mut pos = self.head.load(Ordering::Relaxed);
        loop {
            let Some(cell) = self.cells.get(pos % n) else { return false };
            let turn = cell.turn.load(Ordering::Acquire);
            let diff = turn.wrapping_sub(pos) as isize;
            if diff == 0 {
                match self.head.compare_exchange_weak(pos, pos.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        for (w, v) in cell.words.iter().zip(words) {
                            w.store(v, Ordering::Relaxed);
                        }
                        cell.turn.store(pos.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(p) => pos = p,
                }
            } else if diff < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                pos = self.head.load(Ordering::Relaxed);
            }
        }
    }

    /// The single consumer (callers hold `drain`).
    fn pop(&self) -> Option<[u64; 5]> {
        let n = self.cells.len();
        let pos = self.tail.load(Ordering::Relaxed);
        let cell = self.cells.get(pos % n)?;
        if cell.turn.load(Ordering::Acquire) != pos.wrapping_add(1) {
            return None;
        }
        let words = std::array::from_fn(|k| cell.words.get(k).map_or(0, |w| w.load(Ordering::Relaxed)));
        cell.turn.store(pos.wrapping_add(n), Ordering::Release);
        self.tail.store(pos.wrapping_add(1), Ordering::Relaxed);
        Some(words)
    }
}

static RING: OnceLock<Ring> = OnceLock::new();

fn ring() -> &'static Ring {
    RING.get_or_init(|| Ring::new(CAPACITY))
}

/// The ring once [`mark_main`] made it: the audio side never makes it (that would allocate).
fn ring_if_made() -> Option<&'static Ring> {
    RING.get()
}

/// Records a call (any thread; allocation-free and lock-free). Nothing while the trace is off.
#[inline]
pub(crate) fn record(inst: u32, call: Call, id: u32, value: f64, aux: u64) {
    if on() {
        push(inst, call, id, value, aux);
    }
}

fn push(inst: u32, call: Call, id: u32, value: f64, aux: u64) {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let Some(r) = ring_if_made() else { return };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    r.push([seq, u64::from(inst) << 32 | u64::from(id), (call as u64) << 8 | current() as u64, value.to_bits(), aux]);
}

/// Takes every record so far, oldest first, and the number dropped since the last call. Main
/// thread (any thread is safe; one drainer at a time, a second gets nothing).
pub(crate) fn take() -> (Vec<Record>, u64) {
    let Some(r) = ring_if_made() else { return (Vec::new(), 0) };
    let Ok(_g) = r.drain.try_lock() else { return (Vec::new(), 0) };
    let mut out = Vec::new();
    while let Some(w) = r.pop() {
        let [seq, a, b, v, aux] = w;
        let Some(call) = Call::from_u8((b >> 8) as u8) else { continue };
        out.push(Record { seq, inst: (a >> 32) as u32, id: a as u32, call, thread: Thread::from_u8(b as u8), value: f64::from_bits(v), aux });
    }
    out.sort_by_key(|r| r.seq);
    (out, r.dropped.swap(0, Ordering::Relaxed))
}

/// The trace lines of `records` with bursts collapsed (see the module docs), and a line for
/// `dropped` records.
pub(crate) fn lines(records: &[Record], dropped: u64) -> Vec<String> {
    use std::collections::HashMap;
    let mut out = Vec::new();
    if dropped > 0 {
        out.push(format!("vst3 trace: {dropped} records dropped (the ring was full)"));
    }
    // (inst, call, id) -> (records seen, the last one beyond the burst)
    let mut runs: HashMap<(u32, u8, u32), (usize, Option<Record>)> = HashMap::new();
    let mut order: Vec<(u32, u8, u32)> = Vec::new();
    for r in records {
        let key = (r.inst, r.call as u8, r.id);
        let run = runs.entry(key).or_insert_with(|| {
            order.push(key);
            (0, None)
        });
        run.0 += 1;
        if run.0 <= BURST || !collapsible(r.call) {
            out.push(line(r));
        } else {
            run.1 = Some(*r);
        }
    }
    for key in order {
        if let Some(&(n, Some(last))) = runs.get(&key) {
            out.push(format!("{} (collapsed: {} more like it in this drain; this is the last)", line(&last), n - BURST));
        }
    }
    out
}

fn collapsible(c: Call) -> bool {
    matches!(c, Call::PerformEdit | Call::SetParamNormalized | Call::ProcessorPoint | Call::Echo | Call::HostValue | Call::EchoRepeat | Call::Report)
}

fn value(v: f64) -> String {
    format!("value={v:?} bits={:#018x}", v.to_bits())
}

fn plain(aux: u64) -> String {
    let p = f32::from_bits(aux as u32);
    format!("plain={p:?} plain_bits={:#010x}", p.to_bits())
}

/// The restart flags by name.
pub(crate) fn restart_flags(flags: u32) -> String {
    const NAMES: [&str; 12] = [
        "kReloadComponent",
        "kIoChanged",
        "kParamValuesChanged",
        "kLatencyChanged",
        "kParamTitlesChanged",
        "kMidiCCAssignmentChanged",
        "kNoteExpressionChanged",
        "kIoTitlesChanged",
        "kPrefetchableSupportChanged",
        "kRoutingInfoChanged",
        "kKeyswitchChanged",
        "kParamIDMappingChanged",
    ];
    let mut parts: Vec<String> = NAMES.iter().enumerate().filter(|(i, _)| flags & (1 << i) != 0).map(|(_, n)| (*n).to_owned()).collect();
    let rest = flags & !((1 << NAMES.len()) - 1);
    if rest != 0 {
        parts.push(format!("unknown {rest:#x}"));
    }
    if parts.is_empty() { "none".to_owned() } else { parts.join("|") }
}

/// One trace line.
pub(crate) fn line(r: &Record) -> String {
    let head = format!("vst3 #{} inst={} thread={}", r.seq, r.inst, r.thread.name());
    let size = |what: &str| format!("{head} {what} bytes={} {}", r.aux, if r.id == 1 { "ok" } else { "refused or failed" });
    match r.call {
        Call::BeginEdit => format!("{head} beginEdit id={}", r.id),
        Call::PerformEdit => format!("{head} performEdit id={} {}", r.id, value(r.value)),
        Call::EndEdit => format!("{head} endEdit id={}", r.id),
        Call::RestartComponent => format!("{head} restartComponent flags={:#x} ({})", r.aux, restart_flags(r.aux as u32)),
        Call::SetDirty => format!("{head} IComponentHandler2::setDirty state={}", r.aux),
        Call::RequestOpenEditor => format!("{head} IComponentHandler2::requestOpenEditor (not supported)"),
        Call::StartGroupEdit => format!("{head} IComponentHandler2::startGroupEdit"),
        Call::FinishGroupEdit => format!("{head} IComponentHandler2::finishGroupEdit"),
        Call::NotifyUnitSelection => format!("{head} IUnitHandler::notifyUnitSelection unit={}", r.id as i32),
        Call::NotifyProgramListChange => format!("{head} IUnitHandler::notifyProgramListChange list={} program={}", r.id as i32, r.aux as u32 as i32),
        Call::SetParamNormalized => {
            format!(
                "{head} controller.setParamNormalized id={} {} ({})",
                r.id,
                value(r.value),
                if r.aux == 1 { "editor display" } else { "host value" }
            )
        }
        Call::ProcessorPoint => format!("{head} processor point id={} offset={} {}", r.id, r.aux, value(r.value)),
        Call::ComponentSetState => size("component.setState"),
        Call::ComponentGetState => size("component.getState"),
        Call::ControllerSetComponentState => size("controller.setComponentState"),
        Call::ControllerSetState => size("controller.setState"),
        Call::ControllerGetState => size("controller.getState"),
        Call::SetActive => format!("{head} component.setActive({})", r.aux),
        Call::SetProcessing => format!("{head} processor.setProcessing({})", r.aux),
        Call::Report => format!("{head} report to the session id={} {} {}", r.id, value(r.value), plain(r.aux)),
        Call::Echo => format!("{head} set_param id={} {}: the session's echo, processor gets {}", r.id, plain(r.aux), value(r.value)),
        Call::HostValue => format!("{head} set_param id={} {}: a host value, processor and controller get {}", r.id, plain(r.aux), value(r.value)),
        Call::ResendSkipped => format!("{head} re-send left out id={}: the plug-in reported a newer value, its echo is on the way", r.id),
        Call::EchoRepeat => format!("{head} set_param id={} {}: repeats the echo the host holds, nothing sent", r.id, plain(r.aux)),
        Call::Resend => format!("{head} re-send of a value the host set (after a (re)activation or a curve re-sample) id={}", r.id),
    }
}

/// Writes the lines of every record so far to the log. The instance's main thread.
pub(crate) fn drain() {
    let (records, dropped) = take();
    if records.is_empty() && dropped == 0 {
        return;
    }
    for l in lines(&records, dropped) {
        log::debug!(target: TARGET, "{l}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seq: u64, call: Call, id: u32, value: f64) -> Record {
        Record { seq, inst: 1, thread: Thread::Audio, call, id, value, aux: 0 }
    }

    #[test]
    fn a_burst_keeps_its_first_records_and_one_line_with_the_count_and_the_last_value() {
        let mut rs: Vec<Record> = (0..20).map(|k| rec(k, Call::ProcessorPoint, 7, k as f64 / 100.0)).collect();
        rs.push(rec(20, Call::ProcessorPoint, 3, 0.5));
        let out = lines(&rs, 2);
        assert_eq!(out.first().map(String::as_str), Some("vst3 trace: 2 records dropped (the ring was full)"));
        assert_eq!(out.iter().filter(|l| l.contains("id=7 ")).count(), BURST + 1, "{out:#?}");
        let last = out.last().unwrap();
        assert!(last.contains("collapsed: 12 more") && last.contains("value=0.19 "), "{last}");
        assert!(out.iter().any(|l| l.contains("id=3 ")), "another parameter is not collapsed with it");
    }

    #[test]
    fn the_ring_keeps_order_drops_when_full_and_frees_its_cells() {
        let r = Ring::new(4);
        for k in 0..6u64 {
            let kept = r.push([k, 0, 0, 0, 0]);
            assert_eq!(kept, k < 4, "push {k}");
        }
        assert_eq!(r.dropped.load(Ordering::Relaxed), 2);
        assert_eq!((0..4).map(|_| r.pop().unwrap()[0]).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert!(r.pop().is_none());
        assert!(r.push([9, 0, 0, 0, 0]), "a freed cell takes the next record");
        assert_eq!(r.pop().map(|w| w[0]), Some(9));
    }

    #[test]
    fn restart_flags_are_named() {
        assert_eq!(restart_flags(0x14), "kParamValuesChanged|kParamTitlesChanged");
        assert_eq!(restart_flags(1 << 16), "unknown 0x10000");
        assert_eq!(restart_flags(0), "none");
    }
}

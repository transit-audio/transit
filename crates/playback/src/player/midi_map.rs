//! MIDI Learn matching on the ingress worker (ADR 0028, `docs/contributing/knowledge/midi-learn.md`).
//!
//! Main builds an immutable [`Snapshot`] and swaps an `Arc` of it into the worker. For each
//! message the worker scans the entries, with no allocation and no lock, and publishes a
//! [`MapEvent`] (a `Copy` value in a preallocated ring) for Main to apply. Pickup, jump and scaled
//! are decided here, against the value cell Main keeps current. Learn is Main-side: the worker only
//! forwards the next candidate when its shared flag is armed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use transit_mix::Message;
use transit_model::{MapMode, SourceKind};

/// An entry whose port is not open matches nothing.
pub const NO_PORT: u64 = u64::MAX;
const ANY: u8 = 0xFF;
const NO_LAST: u8 = 0xFF;
/// How far a control may drift from what the knob last set before pickup lets go (1 %): a mouse
/// move of the fader, an undo, automation.
const DRIFT: f32 = 0.01;

/// One mapping as the host asks for it: the port by name, the control by what kind it is.
#[derive(Debug, Clone, PartialEq)]
pub struct MapSpec {
    pub port: String,
    /// Zero-based; `None` accepts every channel.
    pub channel: Option<u8>,
    pub kind: SourceKind,
    pub number: u8,
    pub mode: MapMode,
    pub min: f32,
    pub max: f32,
    pub toggle: bool,
    /// Mute, solo or arm: an on/off control.
    pub button: bool,
    /// Also deliver the message to the track's routes (default off: a mapped message is consumed).
    pub pass_through: bool,
}

impl MapSpec {
    /// The spec of a resolved mapping (callers pass only those not overridden).
    pub fn from_resolved(r: &transit_model::Resolved) -> Self {
        let m = &r.mapping;
        MapSpec {
            port: m.source.port.clone(),
            channel: m.source.channel,
            kind: m.source.kind,
            number: m.source.number,
            mode: m.mode,
            min: m.min,
            max: m.max,
            toggle: m.toggle,
            button: r.control.is_button(),
            pass_through: m.pass_through,
        }
    }
}

/// What the worker and Main share for one entry. Main writes `current` and `applied` once a frame;
/// the worker writes the rest.
#[derive(Debug)]
pub struct Cell {
    /// The control's value (0..1, in the control's own travel) as Main last saw it.
    current: AtomicU32,
    /// How many events of this entry Main has applied.
    applied: AtomicU32,
    /// What the worker last published, and how many it has published.
    published: AtomicU32,
    published_n: AtomicU32,
    /// Pickup: the knob has caught the control. Buttons: the last pressed state.
    engaged: AtomicBool,
    /// The last knob value (0..127) seen, or `NO_LAST`.
    last: AtomicU8,
    /// The newest value the worker published, packed with its publish count (count in the high
    /// word) so a reader never pairs a value with another publish's count. A sweep is coalesced
    /// here, last value wins, so a UI that is slow to apply loses no final value.
    slot: AtomicU64,
    /// The publish count Main last took.
    taken: AtomicU32,
    /// How many continuous-control messages matched this entry, published or not (pickup
    /// holds them back until the knob reaches the control): Main's diagnosis of a knob that moves
    /// nothing.
    heard: AtomicU32,
}

impl Cell {
    pub fn new(current: f32) -> Self {
        Cell {
            current: AtomicU32::new(current.to_bits()),
            applied: AtomicU32::new(0),
            published: AtomicU32::new(current.to_bits()),
            published_n: AtomicU32::new(0),
            engaged: AtomicBool::new(false),
            last: AtomicU8::new(NO_LAST),
            slot: AtomicU64::new(0),
            taken: AtomicU32::new(0),
            heard: AtomicU32::new(0),
        }
    }

    /// The worker: this is the control's newest value.
    fn publish(&self, value: f32) {
        self.published.store(value.to_bits(), Ordering::Relaxed);
        self.current.store(value.to_bits(), Ordering::Relaxed);
        let n = self.published_n.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        self.slot.store((u64::from(n) << 32) | u64::from(value.to_bits()), Ordering::Release);
    }

    /// How many values and presses this entry has published, ever (the UI compares it between
    /// frames to see whether the controller did anything).
    pub fn published(&self) -> u32 {
        self.published_n.load(Ordering::Relaxed)
    }

    /// Main: the newest published value since the last call, with the publish count it belongs to
    /// (what `set_value` is told as `applied`).
    pub fn take(&self) -> Option<(f32, u32)> {
        let packed = self.slot.load(Ordering::Acquire);
        let n = (packed >> 32) as u32;
        if n == 0 || n == self.taken.load(Ordering::Relaxed) {
            return None;
        }
        self.taken.store(n, Ordering::Relaxed);
        Some((f32::from_bits(packed as u32), n))
    }

    /// Main, once a frame: the control's value now, and how many events of this entry it has applied.
    pub fn set_value(&self, current: f32, applied: u32) {
        self.current.store(current.to_bits(), Ordering::Relaxed);
        self.applied.store(applied, Ordering::Relaxed);
    }

    /// Whether pickup has caught the control (tests, `ui.inspect`).
    pub fn engaged(&self) -> bool {
        self.engaged.load(Ordering::Relaxed)
    }

    /// What the worker heard of this entry: how many messages matched it, the last knob value
    /// (0..127, `None` before the first), and whether pickup has caught the control.
    pub fn heard(&self) -> Heard {
        let last = self.last.load(Ordering::Relaxed);
        Heard { count: self.heard.load(Ordering::Relaxed), last: (last != NO_LAST).then_some(last), engaged: self.engaged() }
    }
}

/// [`Cell::heard`], for Main's diagnosis of a mapped knob that moves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Heard {
    pub count: u32,
    pub last: Option<u8>,
    pub engaged: bool,
}

#[derive(Debug, Clone)]
pub struct Entry {
    port: u64,
    channel: u8,
    kind: SourceKind,
    number: u8,
    mode: MapMode,
    min: f32,
    max: f32,
    toggle: bool,
    button: bool,
    pass_through: bool,
    cell: Arc<Cell>,
}

impl Entry {
    /// `port` is the ingress input id the spec's port name resolved to, or `None` while it is closed.
    pub fn new(spec: &MapSpec, port: Option<u64>, cell: Arc<Cell>) -> Self {
        Entry {
            port: port.unwrap_or(NO_PORT),
            channel: spec.channel.unwrap_or(ANY),
            kind: spec.kind,
            number: spec.number,
            mode: spec.mode,
            min: spec.min,
            max: spec.max,
            toggle: spec.toggle,
            button: spec.button,
            pass_through: spec.pass_through,
            cell,
        }
    }
}

/// The mappings the worker matches against. Built on Main, never changed after.
#[derive(Debug)]
pub struct Snapshot {
    /// Which list of specs the entries came from; events carry it so a stale index is ignored.
    pub epoch: u32,
    entries: Box<[Entry]>,
}

impl Snapshot {
    pub fn new(epoch: u32, entries: Vec<Entry>) -> Self {
        Snapshot { epoch, entries: entries.into_boxed_slice() }
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapEvent {
    /// A button: `Some(on)` sets it, `None` flips it.
    Press { epoch: u32, index: u32, set: Option<bool> },
    /// Learn: the message that was armed for.
    Candidate { port: u64, channel: u8, kind: SourceKind, number: u8 },
}

/// A message the matcher cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Msg {
    Cc(u8, u8),
    NoteOn(u8, u8),
    NoteOff(u8),
}

fn msg_of(message: Message) -> Option<Msg> {
    match message {
        Message::Control { cc, value } => Some(Msg::Cc(cc, value)),
        Message::NoteOn { key, velocity, .. } => Some(Msg::NoteOn(key, velocity)),
        Message::NoteOff { key, .. } => Some(Msg::NoteOff(key)),
        _ => None,
    }
}

/// The learn candidate a message makes: a CC (not a channel-mode message) or a note-on.
pub fn candidate(port: u64, channel: u8, message: Message) -> Option<MapEvent> {
    match msg_of(message)? {
        Msg::Cc(cc, _) if cc < 120 => Some(MapEvent::Candidate { port, channel, kind: SourceKind::Cc, number: cc }),
        Msg::NoteOn(key, _) => Some(MapEvent::Candidate { port, channel, kind: SourceKind::Note, number: key }),
        _ => None,
    }
}

/// Whether a CC or note-on is mapped by an entry that does not pass it through, so it must not
/// reach the track's routes. Pure; note-offs are paired with their note-on by the caller.
pub fn consumes(snap: &Snapshot, port: u64, channel: u8, message: Message) -> bool {
    let Some(msg) = msg_of(message) else { return false };
    snap.entries.iter().any(|e| {
        !e.pass_through
            && e.port == port
            && (e.channel == ANY || e.channel == channel)
            && match (msg, e.kind) {
                (Msg::Cc(n, _), SourceKind::Cc) | (Msg::NoteOn(n, _), SourceKind::Note) | (Msg::NoteOff(n), SourceKind::Note) => n == e.number,
                _ => false,
            }
    })
}

/// Match one message against the snapshot and publish what it does: a continuous control's value
/// goes to its cell, a button press to `push` (which returns whether it was queued). Returns
/// whether a mapping without pass-through matched, i.e. the message is consumed. No allocation,
/// no lock.
pub fn handle(snap: &Snapshot, port: u64, channel: u8, message: Message, push: &mut impl FnMut(MapEvent) -> bool) -> bool {
    let Some(msg) = msg_of(message) else { return false };
    let mut consumed = false;
    for (index, e) in snap.entries.iter().enumerate() {
        if e.port != port || (e.channel != ANY && e.channel != channel) {
            continue;
        }
        let (number, value) = match (msg, e.kind) {
            (Msg::Cc(n, v), SourceKind::Cc) => (n, Some(v)),
            (Msg::NoteOn(n, v), SourceKind::Note) => (n, Some(v)),
            (Msg::NoteOff(n), SourceKind::Note) => (n, None),
            _ => continue,
        };
        if number != e.number {
            continue;
        }
        consumed |= !e.pass_through;
        let (epoch, index) = (snap.epoch, index as u32);
        if e.button {
            let pressed = match (msg, value) {
                (Msg::Cc(..), Some(v)) => v >= 64,
                (_, Some(_)) => true,
                (_, None) => false,
            };
            let was = e.cell.engaged.swap(pressed, Ordering::Relaxed);
            let set = if e.toggle {
                // Flip on the press only; a release (note-off, CC back below 64) does nothing.
                if !pressed || (was && matches!(msg, Msg::Cc(..))) {
                    continue;
                }
                None
            } else {
                if was == pressed {
                    continue;
                }
                Some(pressed)
            };
            if push(MapEvent::Press { epoch, index, set }) {
                e.cell.published_n.fetch_add(1, Ordering::Relaxed);
            }
            continue;
        }
        let Some(v) = value else { continue };
        e.cell.heard.fetch_add(1, Ordering::Relaxed);
        let x = f32::from(v) / 127.0;
        let span = e.max - e.min;
        let mut target = e.min + span * x;
        let c = &e.cell;
        let mut send = e.mode == MapMode::Jump || matches!(msg, Msg::NoteOn(..));
        if !send && e.mode == MapMode::Scaled {
            // Not pickup: nothing waits for the knob, so nothing is reported as held back.
            c.engaged.store(true, Ordering::Relaxed);
            let last = c.last.load(Ordering::Relaxed);
            if last != NO_LAST {
                let published = f32::from_bits(c.published.load(Ordering::Relaxed));
                // The control as the worker last left it while Main is behind, else as Main has it
                // (so a mouse drag or automation is taken into account).
                let now = if c.applied.load(Ordering::Relaxed) == c.published_n.load(Ordering::Relaxed) {
                    f32::from_bits(c.current.load(Ordering::Relaxed))
                } else {
                    published
                };
                let at = ((now - e.min) / span).clamp(0.0, 1.0);
                target = e.min + span * scaled_step(at, f32::from(last) / 127.0, x);
                send = true;
            }
        } else if !send {
            let current = f32::from_bits(c.current.load(Ordering::Relaxed));
            let published = f32::from_bits(c.published.load(Ordering::Relaxed));
            // Let go when the control was moved by something else (and Main has caught up with
            // everything this entry published, so the difference is not just lag).
            if c.engaged.load(Ordering::Relaxed)
                && c.applied.load(Ordering::Relaxed) == c.published_n.load(Ordering::Relaxed)
                && (current - published).abs() > DRIFT
            {
                c.engaged.store(false, Ordering::Relaxed);
            }
            if !c.engaged.load(Ordering::Relaxed) {
                let at = ((current - e.min) / span).clamp(0.0, 1.0);
                let last = c.last.load(Ordering::Relaxed);
                let near = (x - at).abs() <= 1.0 / 127.0;
                let crossed = last != NO_LAST && (f32::from(last) / 127.0 - at) * (x - at) <= 0.0;
                if near || crossed {
                    c.engaged.store(true, Ordering::Relaxed);
                }
            }
            send = c.engaged.load(Ordering::Relaxed);
        }
        c.last.store(v, Ordering::Relaxed);
        if send {
            c.publish(target);
        }
    }
    consumed
}

/// Value scaling (Logic's rule, Ableton's Value Scaling): the control at `at` and the knob moving
/// from `from` to `to` (all 0..1 of their travel). The control moves by the same fraction of ITS
/// remaining distance to the end as the knob moves of ITS remaining distance to that end, so the
/// two meet exactly when the knob reaches the end of its travel; once they coincide they track
/// directly. The gap never changes sign, so there is no jump.
pub fn scaled_step(at: f32, from: f32, to: f32) -> f32 {
    let (at, from, to) = (at.clamp(0.0, 1.0), from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
    // A repeated value (the knob did not move) moves nothing.
    if to == from {
        return at;
    }
    let next = if to >= from {
        let room = 1.0 - from;
        if room <= f32::EPSILON { 1.0 } else { at + (to - from) / room * (1.0 - at) }
    } else if from <= f32::EPSILON {
        0.0
    } else {
        at - (from - to) / from * at
    };
    // Within one knob step the two have met: from here the control follows the knob exactly.
    let next = if (next - to).abs() <= 0.5 / 127.0 { to } else { next };
    // ...but never against the knob's direction.
    let next = if to >= from { next.max(at) } else { next.min(at) };
    next.clamp(0.0, 1.0)
}

/// What Main wants matched, and what the worker was last given. Main side only.
#[derive(Default)]
pub(super) struct Wanted {
    specs: Vec<MapSpec>,
    cells: Vec<Arc<Cell>>,
    epoch: u32,
    /// The epoch and port ids of the snapshot last handed to the worker.
    sent: Option<(u32, Vec<Option<u64>>)>,
    /// Snapshots the worker may still hold a reference to; dropped here once it lets go, so the
    /// worker never frees one.
    retired: Vec<Arc<Snapshot>>,
    current: Option<Arc<Snapshot>>,
    pub learning: bool,
    /// Tells this Native from the one a reconnect replaces; epochs start from it, so a new
    /// Native's epochs never equal an old one's.
    generation: u32,
}

static GENERATIONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

impl Wanted {
    pub fn new() -> Self {
        let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        Wanted { epoch: generation.wrapping_mul(1 << 16), generation, ..Default::default() }
    }

    /// Total publishes of every mapping so far; a change since the last look means the controller
    /// did something.
    pub fn activity(&self) -> u64 {
        self.cells.iter().map(|c| u64::from(c.published())).sum()
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Each continuous control's newest published value since the last call (last wins), as the UI
    /// applies it. Never loses the final value of a sweep, however slowly it is called.
    pub fn take_moves(&self) -> Vec<(u32, f32, u32)> {
        self.cells.iter().enumerate().filter_map(|(i, c)| c.take().map(|(v, n)| (i as u32, v, n))).collect()
    }

    /// Replace the wanted mappings. Equal lists change nothing (pickup state survives).
    pub fn set(&mut self, specs: Vec<MapSpec>) {
        if specs == self.specs {
            return;
        }
        self.cells = specs.iter().map(|_| Arc::new(Cell::new(0.0))).collect();
        self.specs = specs;
        self.epoch = self.epoch.wrapping_add(1);
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Whether a port with this name should be open for MIDI Learn: any while a learn is armed.
    pub fn wants_port(&self, name: &str) -> bool {
        self.learning || self.specs.iter().any(|spec| spec.port == name)
    }

    pub fn wants_any(&self) -> bool {
        self.learning || !self.specs.is_empty()
    }

    /// What the worker heard of each wanted mapping, in order.
    pub fn heard(&self) -> Vec<Heard> {
        self.cells.iter().map(|c| c.heard()).collect()
    }

    /// Whether each wanted mapping's port was open in the snapshot the worker was last handed;
    /// `None` until it has been handed one of this list.
    pub fn ports_open(&self) -> Option<Vec<bool>> {
        match &self.sent {
            Some((epoch, ids)) if *epoch == self.epoch => Some(ids.iter().map(Option::is_some).collect()),
            _ => None,
        }
    }

    pub fn port_of(&self, spec: usize) -> Option<&str> {
        self.specs.get(spec).map(|s| s.port.as_str())
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// The control values Main sees now, one `(value, events applied)` per wanted mapping, for the
    /// worker's pickup.
    pub fn set_values(&self, values: &[(f32, u32)]) {
        for (cell, (value, applied)) in self.cells.iter().zip(values) {
            cell.set_value(*value, *applied);
        }
    }

    /// The snapshot to hand the worker, when what it holds is out of date: the list changed, or a
    /// port opened or closed (`ids` are the ingress input ids, `None` for a closed port).
    pub fn rebuild(&mut self, ids: Vec<Option<u64>>) -> Option<Arc<Snapshot>> {
        self.retired.retain(|old| Arc::strong_count(old) > 1);
        if self.sent.is_none() && self.specs.is_empty() {
            return None;
        }
        if self.sent.as_ref().is_some_and(|(epoch, sent)| *epoch == self.epoch && *sent == ids) {
            return None;
        }
        let entries = self.specs.iter().zip(&ids).zip(&self.cells).map(|((spec, id), cell)| Entry::new(spec, *id, Arc::clone(cell))).collect();
        let snapshot = Arc::new(Snapshot::new(self.epoch, entries));
        if let Some(old) = self.current.replace(Arc::clone(&snapshot)) {
            self.retired.push(old);
        }
        self.sent = Some((self.epoch, ids));
        Some(snapshot)
    }

    /// The worker did not take the snapshot: build it again next time.
    pub fn unsent(&mut self) {
        self.sent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transit_mix::NoteId;

    fn spec(kind: SourceKind, number: u8, mode: MapMode, button: bool) -> MapSpec {
        MapSpec { port: "KeyStep".into(), channel: Some(0), kind, number, mode, min: 0.0, max: 1.0, toggle: true, button, pass_through: false }
    }
    fn snap(spec: &MapSpec, current: f32) -> (Snapshot, Arc<Cell>) {
        let cell = Arc::new(Cell::new(current));
        (Snapshot::new(3, vec![Entry::new(spec, Some(1), Arc::clone(&cell))]), cell)
    }
    fn cc(snap: &Snapshot, value: u8) -> Vec<MapEvent> {
        let mut out = Vec::new();
        handle(snap, 1, 0, Message::Control { cc: 7, value }, &mut |e| {
            out.push(e);
            true
        });
        out
    }
    /// The continuous values `handle` published for a CC 7 of `value`.
    fn mv(snap: &Snapshot, value: u8) -> Vec<f32> {
        let _ = cc(snap, value);
        snap.entries[0].cell.take().map(|(v, _)| v).into_iter().collect()
    }

    #[test]
    fn pickup_waits_for_the_knob_to_cross_the_control() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 0.5);
        assert!(mv(&s, 0).is_empty(), "knob far below the fader: nothing moves");
        assert!(mv(&s, 30).is_empty());
        assert!(!cell.engaged());
        let crossing = mv(&s, 80);
        assert_eq!(crossing.len(), 1, "passing the fader picks it up");
        assert!((crossing[0] - 80.0 / 127.0).abs() < 1e-6);
        assert!(cell.engaged());
        assert_eq!(mv(&s, 90).len(), 1, "then it follows");
        assert_eq!(mv(&s, 10).len(), 1, "in both directions");
    }

    #[test]
    fn pickup_catches_a_knob_that_starts_on_the_value_and_lets_go_after_an_outside_move() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 64.0 / 127.0);
        assert_eq!(mv(&s, 64).len(), 1, "a knob already there engages at once");
        // Main applied the one event and the control sits where the knob put it.
        cell.set_value(64.0 / 127.0, 1);
        assert_eq!(mv(&s, 70).len(), 1);
        cell.set_value(70.0 / 127.0, 2);
        // The user drags the fader with the mouse to the bottom.
        cell.set_value(0.1, 2);
        assert!(mv(&s, 75).is_empty(), "the knob no longer matches the fader: pickup again");
        assert!(!cell.engaged());
        assert_eq!(mv(&s, 5).len(), 1, "crossing the new value picks it up");
    }

    #[test]
    fn lag_behind_main_does_not_release_the_pickup() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 0.0);
        assert_eq!(mv(&s, 0).len(), 1);
        assert_eq!(mv(&s, 40).len(), 1);
        assert_eq!(mv(&s, 80).len(), 1);
        // Main has applied only the first event and still reports the old value.
        cell.set_value(0.0, 1);
        assert_eq!(mv(&s, 90).len(), 1, "still engaged: Main has not caught up");
    }

    /// Plays a knob sweep (0..127 values) at a scaled mapping, Main keeping up after each message;
    /// returns the control's value after each.
    fn sweep(start: f32, knob: &[u8]) -> Vec<f32> {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Scaled, false), start);
        let mut value = start;
        let mut n = 0;
        let mut out = Vec::new();
        for k in knob {
            if let Some(v) = mv(&s, *k).first() {
                value = *v;
                n += 1;
            }
            cell.set_value(value, n);
            out.push(value);
        }
        out
    }

    #[test]
    fn scaled_up_sweep_from_10_to_100_percent_on_a_fader_at_70_does_not_jump_and_ends_at_100() {
        let knob: Vec<u8> = (13..=127).collect(); // 10 % .. 100 %
        let v = sweep(0.7, &knob);
        assert_eq!(v[0], 0.7, "the first message only records where the knob is");
        assert!(v.windows(2).all(|w| w[1] >= w[0]), "monotonic");
        assert!(v.windows(2).all(|w| w[1] - w[0] < 0.01), "no jump");
        assert_eq!(*v.last().unwrap(), 1.0);
        assert!(v.iter().all(|x| (0.7..=1.0).contains(x)), "the control does not dip below where it was");
        // Deterministic: the same sweep gives the same values.
        assert_eq!(v, sweep(0.7, &knob));
    }

    #[test]
    fn scaled_reverse_sweep_converges_downward_and_ends_at_zero() {
        let knob: Vec<u8> = (0..=127).rev().collect(); // 100 % .. 0 %
        let v = sweep(0.7, &knob);
        assert_eq!(v[0], 0.7);
        assert!(v.windows(2).all(|w| w[1] <= w[0]), "monotonic");
        assert!(v.windows(2).all(|w| w[0] - w[1] < 0.02), "no jump");
        assert_eq!(*v.last().unwrap(), 0.0);
        // The fader closes on the knob: the gap (control minus knob) shrinks to nothing.
        let gap = |i: usize| v[i] - knob[i] as f32 / 127.0;
        assert!(gap(60).abs() < gap(1).abs(), "closing");
        assert_eq!(gap(127), 0.0);
    }

    #[test]
    fn scaled_tracks_directly_once_the_knob_and_control_coincide() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Scaled, false), 64.0 / 127.0);
        let _ = mv(&s, 64);
        for k in [90u8, 70, 20, 100] {
            assert!((mv(&s, k)[0] - f32::from(k) / 127.0).abs() < 1e-6);
            cell.set_value(f32::from(k) / 127.0, 1000);
        }
    }

    #[test]
    fn scaled_scaling_formula_meets_at_the_end_of_travel() {
        assert!((scaled_step(0.7, 0.1, 0.1 + 0.9 * 0.5) - (0.7 + 0.5 * 0.3)).abs() < 1e-6);
        assert_eq!(scaled_step(0.7, 0.1, 1.0), 1.0);
        assert_eq!(scaled_step(0.2, 0.9, 0.0), 0.0);
        assert_eq!(scaled_step(0.3, 0.3, 0.8), 0.8, "no gap: direct");
    }

    #[test]
    fn a_repeated_top_value_does_not_move_the_control() {
        assert_eq!(scaled_step(0.3, 1.0, 1.0), 0.3);
        assert_eq!(scaled_step(0.3, 0.0, 0.0), 0.3);
        let v = sweep(0.3, &[127, 127, 127]);
        assert_eq!(v, vec![0.3, 0.3, 0.3]);
    }

    #[test]
    fn scaled_is_never_reported_as_waiting() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Scaled, false), 0.7);
        let _ = mv(&s, 10);
        assert!(cell.engaged());
    }

    #[test]
    fn jump_follows_the_knob_with_the_range() {
        let mut sp = spec(SourceKind::Cc, 7, MapMode::Jump, false);
        sp.min = 0.2;
        sp.max = 0.6;
        let (s, _) = snap(&sp, 0.9);
        let v = mv(&s, 127);
        assert!((v[0] - 0.6).abs() < 1e-6);
        let v = mv(&s, 0);
        assert!((v[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn a_note_toggles_a_button_on_note_on_only() {
        let (s, _) = snap(&spec(SourceKind::Note, 60, MapMode::Pickup, true), 0.0);
        let mut out = Vec::new();
        let mut push = |e| {
            out.push(e);
            true
        };
        handle(&s, 1, 0, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE }, &mut push);
        handle(&s, 1, 0, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE }, &mut push);
        handle(&s, 1, 0, Message::NoteOn { key: 61, velocity: 100, id: NoteId::NONE }, &mut push);
        handle(&s, 2, 0, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE }, &mut push);
        handle(&s, 1, 5, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE }, &mut push);
        assert_eq!(out, vec![MapEvent::Press { epoch: 3, index: 0, set: None }], "other key, port and channel are ignored");
    }

    #[test]
    fn a_cc_button_toggles_on_the_rising_edge_and_a_momentary_one_follows() {
        let (s, _) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, true), 0.0);
        let presses = |events: Vec<MapEvent>| events.len();
        assert_eq!(presses(cc(&s, 127)), 1);
        assert_eq!(presses(cc(&s, 127)), 0, "held: no repeat");
        assert_eq!(presses(cc(&s, 0)), 0, "release does nothing");
        assert_eq!(presses(cc(&s, 100)), 1);
        let mut sp = spec(SourceKind::Cc, 7, MapMode::Pickup, true);
        sp.toggle = false;
        let (s, _) = snap(&sp, 0.0);
        assert_eq!(cc(&s, 127), vec![MapEvent::Press { epoch: 3, index: 0, set: Some(true) }]);
        assert!(cc(&s, 100).is_empty());
        assert_eq!(cc(&s, 0), vec![MapEvent::Press { epoch: 3, index: 0, set: Some(false) }]);
    }

    #[test]
    fn candidates_are_ccs_and_note_ons_only() {
        let cc = |cc| candidate(2, 4, Message::Control { cc, value: 1 });
        assert_eq!(cc(74), Some(MapEvent::Candidate { port: 2, channel: 4, kind: SourceKind::Cc, number: 74 }));
        assert_eq!(cc(123), None, "all notes off is not a knob");
        assert!(candidate(2, 0, Message::NoteOn { key: 36, velocity: 90, id: NoteId::NONE }).is_some());
        assert!(candidate(2, 0, Message::NoteOff { key: 36, velocity: 0, id: NoteId::NONE }).is_none());
        assert!(candidate(2, 0, Message::PitchBend { value: 8192 }).is_none());
    }

    #[test]
    fn a_sweep_is_coalesced_last_value_wins_with_the_publish_count() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Jump, false), 0.0);
        for v in 0..=100u8 {
            let _ = cc(&s, v);
        }
        let (value, n) = cell.take().unwrap();
        assert!((value - 100.0 / 127.0).abs() < 1e-6, "the final value survives: {value}");
        assert_eq!(n, 101);
        assert!(cell.take().is_none(), "taken once");
    }

    #[test]
    fn a_mapped_message_is_consumed_unless_the_mapping_passes_it_through() {
        let msg = Message::Control { cc: 7, value: 9 };
        let (s, _) = snap(&spec(SourceKind::Cc, 7, MapMode::Jump, false), 0.0);
        assert!(consumes(&s, 1, 0, msg));
        assert!(handle(&s, 1, 0, msg, &mut |_| true));
        assert!(!consumes(&s, 1, 0, Message::Control { cc: 8, value: 9 }), "another CC is not ours");
        assert!(!consumes(&s, 2, 0, msg) && !consumes(&s, 1, 3, msg), "another port or channel");
        let mut sp = spec(SourceKind::Cc, 7, MapMode::Jump, false);
        sp.pass_through = true;
        let (s, cell) = snap(&sp, 0.0);
        assert!(!consumes(&s, 1, 0, msg));
        assert!(!handle(&s, 1, 0, msg, &mut |_| true), "pass-through still drives the control");
        assert!(cell.take().is_some());
        // An engaged-or-not pickup mapping consumes too: the knob is the mapping's, not the track's.
        let (s, _) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 0.9);
        assert!(handle(&s, 1, 0, msg, &mut |_| true));
    }

    #[test]
    fn a_reader_never_pairs_a_value_with_another_publishes_count() {
        // The writer publishes the value `n` as the n-th publish (n wraps at 2^24 so the f32 stays
        // exact); any (value, count) a reader gets must agree. With two atomics a reader paired an
        // old value with a newer count. The writer runs until the reader has checked a fixed number
        // of pairs, so the test does not depend on which thread the scheduler runs first.
        const PAIRS: u32 = 20_000;
        let exact = |n: u32| (n & 0x00ff_ffff) as f32;
        let cell = Arc::new(Cell::new(0.0));
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (cell, stop) = (Arc::clone(&cell), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    cell.publish(exact(n));
                }
            })
        };
        let mut seen = 0u32;
        while seen < PAIRS {
            if let Some((value, n)) = cell.take() {
                assert_eq!(value, exact(n), "a torn pair");
                seen += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        assert_eq!(seen, PAIRS);
    }
}

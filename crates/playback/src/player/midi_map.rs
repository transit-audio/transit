//! MIDI Learn matching on the ingress worker (ADR 0028, `docs/contributing/knowledge/midi-learn.md`).
//!
//! Main builds an immutable [`Snapshot`] and swaps an `Arc` of it into the worker. For each
//! message the worker scans the entries, with no allocation and no lock, and publishes a
//! [`MapEvent`] (a `Copy` value in a preallocated ring) for Main to apply. Pickup and jump are
//! decided here, against the value cell Main keeps current. Learn is Main-side: the worker only
//! forwards the next candidate when its shared flag is armed.

use soundcraft_mix::Message;
use soundcraft_model::{MapMode, SourceKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

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
}

impl MapSpec {
    /// The spec of a resolved mapping (callers pass only those not overridden).
    pub fn from_resolved(r: &soundcraft_model::Resolved) -> Self {
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
        }
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
    /// Set a continuous control to `value` (0..1 of its travel).
    Move { epoch: u32, index: u32, value: f32 },
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

/// Match one admitted message against the snapshot and publish what it does. `push` returns
/// whether the event was queued. No allocation, no lock.
pub fn handle(snap: &Snapshot, port: u64, channel: u8, message: Message, push: &mut impl FnMut(MapEvent) -> bool) {
    let Some(msg) = msg_of(message) else { return };
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
        let x = f32::from(v) / 127.0;
        let span = e.max - e.min;
        let target = e.min + span * x;
        let c = &e.cell;
        let mut send = e.mode == MapMode::Jump || matches!(msg, Msg::NoteOn(..));
        if !send {
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
        if send && push(MapEvent::Move { epoch, index, value: target }) {
            c.published.store(target.to_bits(), Ordering::Relaxed);
            c.current.store(target.to_bits(), Ordering::Relaxed);
            c.published_n.fetch_add(1, Ordering::Relaxed);
        }
    }
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
}

impl Wanted {
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
    use soundcraft_mix::NoteId;

    fn spec(kind: SourceKind, number: u8, mode: MapMode, button: bool) -> MapSpec {
        MapSpec { port: "KeyStep".into(), channel: Some(0), kind, number, mode, min: 0.0, max: 1.0, toggle: true, button }
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
    fn moved(events: &[MapEvent]) -> Vec<f32> {
        events.iter().filter_map(|e| if let MapEvent::Move { value, .. } = e { Some(*value) } else { None }).collect()
    }

    #[test]
    fn pickup_waits_for_the_knob_to_cross_the_control() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 0.5);
        assert!(cc(&s, 0).is_empty(), "knob far below the fader: nothing moves");
        assert!(cc(&s, 30).is_empty());
        assert!(!cell.engaged());
        let crossing = cc(&s, 80);
        assert_eq!(moved(&crossing).len(), 1, "passing the fader picks it up");
        assert!((moved(&crossing)[0] - 80.0 / 127.0).abs() < 1e-6);
        assert!(cell.engaged());
        assert_eq!(moved(&cc(&s, 90)).len(), 1, "then it follows");
        assert_eq!(moved(&cc(&s, 10)).len(), 1, "in both directions");
    }

    #[test]
    fn pickup_catches_a_knob_that_starts_on_the_value_and_lets_go_after_an_outside_move() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 64.0 / 127.0);
        assert_eq!(moved(&cc(&s, 64)).len(), 1, "a knob already there engages at once");
        // Main applied the one event and the control sits where the knob put it.
        cell.set_value(64.0 / 127.0, 1);
        assert_eq!(moved(&cc(&s, 70)).len(), 1);
        cell.set_value(70.0 / 127.0, 2);
        // The user drags the fader with the mouse to the bottom.
        cell.set_value(0.1, 2);
        assert!(cc(&s, 75).is_empty(), "the knob no longer matches the fader: pickup again");
        assert!(!cell.engaged());
        assert_eq!(moved(&cc(&s, 5)).len(), 1, "crossing the new value picks it up");
    }

    #[test]
    fn lag_behind_main_does_not_release_the_pickup() {
        let (s, cell) = snap(&spec(SourceKind::Cc, 7, MapMode::Pickup, false), 0.0);
        assert_eq!(moved(&cc(&s, 0)).len(), 1);
        assert_eq!(moved(&cc(&s, 40)).len(), 1);
        assert_eq!(moved(&cc(&s, 80)).len(), 1);
        // Main has applied only the first event and still reports the old value.
        cell.set_value(0.0, 1);
        assert_eq!(moved(&cc(&s, 90)).len(), 1, "still engaged: Main has not caught up");
    }

    #[test]
    fn jump_follows_the_knob_with_the_range() {
        let mut sp = spec(SourceKind::Cc, 7, MapMode::Jump, false);
        sp.min = 0.2;
        sp.max = 0.6;
        let (s, _) = snap(&sp, 0.9);
        let v = moved(&cc(&s, 127));
        assert!((v[0] - 0.6).abs() < 1e-6);
        let v = moved(&cc(&s, 0));
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
}

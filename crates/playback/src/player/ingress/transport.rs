//! Dedicated bounded worker-to-control transport publication. Never used by audio.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use transit_midi_io::{TRANSPORT_CAPACITY, TransportEnvelope};

/// Exactly-once learn consideration across independent copies and delayed musical routing.
pub(super) struct CaptureHistory {
    pub next: u64,
    consumed: std::collections::VecDeque<(u64, bool)>,
}

impl Default for CaptureHistory {
    fn default() -> Self {
        Self { next: 0, consumed: std::collections::VecDeque::with_capacity(TRANSPORT_CAPACITY * 2) }
    }
}

impl CaptureHistory {
    pub fn pending(&self) -> bool {
        !self.consumed.is_empty()
    }

    pub fn remember(&mut self, seq: u64, consume_route: bool) -> bool {
        if self.consumed.len() == self.consumed.capacity() {
            return false;
        }
        self.consumed.push_back((seq, consume_route));
        true
    }

    pub fn disposition(&self, seq: u64) -> Option<bool> {
        self.consumed.iter().find(|(id, _)| *id == seq).map(|(_, consume)| *consume)
    }

    pub fn consume(&mut self, seq: u64) -> Option<bool> {
        while self.consumed.front().is_some_and(|(old, _)| *old < seq) {
            self.consumed.pop_front();
        }
        if self.consumed.front().is_some_and(|(id, _)| *id == seq) { self.consumed.pop_front().map(|(_, consume)| consume) } else { None }
    }
}

pub(super) fn mapped(sources: &[transit_model::MidiSource], raw: &transit_midi_io::InputEvent) -> bool {
    let kind = match raw.bytes[0] & 0xf0 {
        0x80 | 0x90 => transit_model::SourceKind::Note,
        0xb0 => transit_model::SourceKind::Cc,
        _ => return false,
    };
    sources.iter().any(|s| s.kind == kind && s.number == raw.bytes[1] && s.channel.is_none_or(|c| c == raw.bytes[0] & 15))
}

/// Capturing owns the edge until release. Both the musical scan and dedicated lane share this
/// latch, so a fast press/release completes even before the new source mask is installed.
pub(super) fn capture(
    mon: &mut super::Mon,
    id: u64,
    raw: transit_midi_io::InputEvent,
    out: &mut rtrb::Producer<super::MapEvent>,
    shared: &super::Shared,
) -> Option<bool> {
    let cc = raw.bytes[0] & 0xf0 == 0xb0;
    if !matches!(raw.bytes[0] & 0xf0, 0x80 | 0x90 | 0xb0) {
        return None;
    }
    let word = usize::from(raw.bytes[0] & 15) + usize::from(cc) * 16;
    let bit = 1 << (raw.bytes[1] & 127);
    let high = if cc { raw.bytes[2] >= 64 } else { raw.bytes[0] & 0xf0 == 0x90 && raw.bytes[2] > 0 };
    if mon.captured[word] & bit != 0 {
        if !high {
            mon.captured[word] &= !bit;
        }
        return Some(true);
    }
    if shared.learn.load(Ordering::Acquire)
        && let Some((channel, message)) = super::decode(raw)
        && let Some(candidate) = super::midi_map::candidate(id, channel, message)
        && shared.learn.swap(false, Ordering::AcqRel)
    {
        let consume = shared.learn_transport.load(Ordering::Acquire);
        if out.push(candidate).is_ok() {
            shared.map_pushed.fetch_add(1, Ordering::Relaxed);
        } else {
            shared.map_dropped.fetch_add(1, Ordering::Relaxed);
        }
        if high && consume {
            mon.captured[word] |= bit;
        }
        return Some(consume);
    }
    None
}

pub(super) struct Sender {
    ring: rtrb::Producer<(u64, TransportEnvelope)>,
    lost: Arc<AtomicBool>,
}

pub(super) struct Receiver {
    ring: RefCell<rtrb::Consumer<(u64, TransportEnvelope)>>,
    lost: Arc<AtomicBool>,
}

pub(super) fn pair() -> (Sender, Receiver) {
    let (tx, rx) = rtrb::RingBuffer::new(TRANSPORT_CAPACITY);
    let lost = Arc::new(AtomicBool::new(false));
    (Sender { ring: tx, lost: Arc::clone(&lost) }, Receiver { ring: RefCell::new(rx), lost })
}

impl Sender {
    pub(super) fn fail(&self) {
        self.lost.store(true, Ordering::Release);
    }
    pub(super) fn push(&mut self, port: u64, event: TransportEnvelope) {
        if event.bytes[0] == 0xf8 {
            return;
        }
        if !self.lost.load(Ordering::Acquire) && self.ring.push((port, event)).is_err() {
            self.lost.store(true, Ordering::Release);
        }
    }
}

impl Receiver {
    pub(super) fn take(&self) -> (bool, Vec<(u64, TransportEnvelope)>) {
        let Ok(mut ring) = self.ring.try_borrow_mut() else { return (false, Vec::new()) };
        let mut out = Vec::new();
        for _ in 0..TRANSPORT_CAPACITY {
            let Ok(event) = ring.pop() else { break };
            out.push(event);
        }
        let lost = self.lost.swap(false, Ordering::AcqRel);
        if lost {
            out.clear();
        }
        (lost, out)
    }
}

//! A stage's output buffer (ADR 0057, slice P3b): acceptance is final. `push` admits a non-release
//! event while the buffer holds fewer than [`MIDI_FX_NON_RELEASE`] non-release events and fewer
//! than [`MIDI_FX_BLOCK`] events in all, a release while it holds fewer than [`MIDI_FX_BLOCK`], so
//! releases already admitted never use the non-release quota; nothing admitted is evicted later.
//! A release it refuses marks the block for the host's reset barrier (`release_refused`).
use super::{MIDI_FX_BLOCK, MIDI_FX_NON_RELEASE, Message, MidiFxCounts, TimedEvent};

pub(super) struct Output {
    events: [TimedEvent; MIDI_FX_BLOCK],
    len: usize,
    /// Non-release events among the first `len`.
    held: usize,
    pub(super) refused: u32,
    /// A release found the buffer full.
    pub(super) release_refused: bool,
    pub(super) counts: MidiFxCounts,
}

impl Output {
    pub(super) fn new() -> Self {
        Self { events: [TimedEvent::EMPTY; MIDI_FX_BLOCK], len: 0, held: 0, refused: 0, release_refused: false, counts: MidiFxCounts::default() }
    }

    /// Test seam (feature `testing`, #742): every entry's event becomes `junk(k)`, length
    /// unchanged. Returns the entries changed.
    #[cfg(feature = "testing")]
    pub(super) fn poison(&mut self, junk: impl Fn(usize) -> TimedEvent) -> usize {
        let mut n = 0;
        for (k, e) in self.events.iter_mut().enumerate() {
            n += usize::from(*e != junk(k));
            *e = junk(k);
        }
        n
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
        self.held = 0;
        self.refused = 0;
        self.release_refused = false;
        self.counts = MidiFxCounts::default();
    }

    /// Admits `event` or refuses it (counted), finally.
    pub(super) fn push(&mut self, event: TimedEvent) -> bool {
        let release = event.event.is_release();
        let room = self.len < MIDI_FX_BLOCK && (release || self.held < MIDI_FX_NON_RELEASE);
        let Some(slot) = self.events.get_mut(self.len).filter(|_| room) else {
            self.refused = self.refused.saturating_add(1);
            self.counts.overflow = self.counts.overflow.saturating_add(1);
            if matches!(event.event.message, Message::NoteOn { .. }) {
                self.counts.refused_ons = self.counts.refused_ons.saturating_add(1);
            }
            self.release_refused |= release;
            return false;
        };
        *slot = event;
        self.len += 1;
        self.held += usize::from(!release);
        true
    }

    /// The admitted events, in write order.
    pub(super) fn events(&self) -> impl Iterator<Item = TimedEvent> + '_ {
        self.events.iter().take(self.len).copied()
    }
}

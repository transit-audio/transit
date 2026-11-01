//! Fixed output storage. Eviction uses an index stack; links preserve write order at equal offsets.
use super::{MIDI_FX_BLOCK, Message, MidiFxCounts, PluginEvent, TimedEvent};

const END: usize = MIDI_FX_BLOCK;

#[derive(Clone, Copy)]
struct Entry {
    event: TimedEvent,
    previous: usize,
    next: usize,
}

const EMPTY: Entry = Entry { event: TimedEvent::EMPTY, previous: END, next: END };

pub(super) struct Output {
    entries: [Entry; MIDI_FX_BLOCK],
    len: usize,
    first: usize,
    last: usize,
    // After filling, these cursors only move backward. Each slot is examined at most once
    // by each cursor, so finding victims costs O(1) amortized per event.
    on_cursor: usize,
    other_cursor: usize,
    safety_channels: u16,
    safety_offset: u32,
    pub(super) refused: u32,
    pub(super) counts: MidiFxCounts,
}

impl Output {
    pub(super) fn new() -> Self {
        Self {
            entries: [EMPTY; MIDI_FX_BLOCK],
            len: 0,
            first: END,
            last: END,
            on_cursor: 0,
            other_cursor: 0,
            safety_channels: 0,
            safety_offset: 0,
            refused: 0,
            counts: MidiFxCounts::default(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
        self.first = END;
        self.last = END;
        self.on_cursor = 0;
        self.other_cursor = 0;
        self.safety_channels = 0;
        self.safety_offset = 0;
        self.refused = 0;
        self.counts = MidiFxCounts::default();
    }

    fn victim(&mut self) -> Option<usize> {
        while self.on_cursor > 0 {
            self.on_cursor -= 1;
            if matches!(self.entries[self.on_cursor].event.event.message, Message::NoteOn { .. }) {
                self.counts.evicted_ons = self.counts.evicted_ons.saturating_add(1);
                return Some(self.on_cursor);
            }
        }
        while self.other_cursor > 0 {
            self.other_cursor -= 1;
            if !self.entries[self.other_cursor].event.event.is_release() {
                return Some(self.other_cursor);
            }
        }
        None
    }

    fn unlink(&mut self, i: usize) {
        let e = self.entries[i];
        if e.previous < END {
            self.entries[e.previous].next = e.next;
        } else {
            self.first = e.next;
        }
        if e.next < END {
            self.entries[e.next].previous = e.previous;
        } else {
            self.last = e.previous;
        }
    }

    pub(super) fn push(&mut self, event: TimedEvent) -> bool {
        if self.safety_channels != 0 {
            if event.event.is_release() {
                self.coalesce(event);
                return true;
            }
            return self.refuse(event);
        }
        let i = if self.len < MIDI_FX_BLOCK {
            let i = self.len;
            self.len += 1;
            self.on_cursor = self.len;
            self.other_cursor = self.len;
            i
        } else if event.event.is_release() {
            let Some(i) = self.victim() else {
                return self.release_only_overflow(event);
            };
            self.counts.overflow = self.counts.overflow.saturating_add(1);
            self.unlink(i);
            i
        } else {
            return self.refuse(event);
        };
        self.entries[i] = Entry { event, previous: self.last, next: END };
        if self.last < END {
            self.entries[self.last].next = i;
        } else {
            self.first = i;
        }
        self.last = i;
        true
    }

    fn refuse(&mut self, event: TimedEvent) -> bool {
        self.refused = self.refused.saturating_add(1);
        self.counts.overflow = self.counts.overflow.saturating_add(1);
        if matches!(event.event.message, Message::NoteOn { .. }) {
            self.counts.refused_ons = self.counts.refused_ons.saturating_add(1);
        }
        false
    }

    fn coalesce(&mut self, event: TimedEvent) {
        self.safety_channels |= 1 << (event.event.channel & 15);
        self.safety_offset = self.safety_offset.max(event.offset);
        self.counts.coalesced_releases = self.counts.coalesced_releases.saturating_add(1);
    }

    fn release_only_overflow(&mut self, event: TimedEvent) -> bool {
        // All pending ons (and then other non-releases) have already been removed. No on
        // can follow these safety releases in this buffer. Bound even an all-off flood to
        // seven channel releases per affected channel, at the last release's offset.
        for i in 0..self.len {
            self.coalesce(self.entries[i].event);
        }
        self.len = 0;
        self.coalesce(event);
        true
    }

    pub(super) fn events(&self) -> impl Iterator<Item = TimedEvent> + '_ {
        let mut next = self.first;
        let normal = (0..self.len).map(move |_| {
            let entry = self.entries[next];
            next = entry.next;
            entry.event
        });
        let safety = (0u8..16).filter(move |ch| self.safety_channels & (1 << ch) != 0).flat_map(move |channel| {
            [64, 66, 67, 69, 120, 121, 123].map(|cc| TimedEvent {
                offset: self.safety_offset,
                event: PluginEvent { channel, note_id: PluginEvent::NO_ID, message: Message::Control { cc, value: 0 } },
            })
        });
        normal.chain(safety)
    }
}

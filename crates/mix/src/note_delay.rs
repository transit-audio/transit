//! Per-reader note timing, stored with the insert so plan moves preserve pending releases.
use crate::note_view::{ALL_OFF, NOTE_VIEW_CAP, RELEASE_RESERVE, is_release};
use transit_dsp::event::Message;
use transit_dsp::{Plugin, midi_fx::TimedEvent};
use transit_rt::midi_defer::past_its_note_on;

/// One pending event per 64 frames of the plan's maximum latency (2^20 frames): 16,384 events.
/// Density, rather than elapsed time, bounds storage; even larger reported delays need no more
/// storage per event. The last 64 entries are reserved for releases.
pub(crate) const CARRY_CAP: usize = crate::MAX_LATENCY / 64;

#[derive(Clone, Copy)]
struct Pending {
    source: u64,
    order: u64,
    event: transit_dsp::event::PluginEvent,
}

pub(crate) struct NoteDelay {
    /// The instrument instance whose stream this reader follows, not the plan generation.
    source: Option<crate::SlotGen>,
    carry: Box<[Pending]>,
    len: usize,
    clock: u64,
    order: u64,
    /// While an overflow's all-notes-off stand-in has not left the carry: its `order`. Non-releases
    /// are refused for exactly that long.
    stand_in: Option<u64>,
    /// A bypassed, inactive or failed reader has already released this run of skipped blocks.
    skipped: bool,
}

impl NoteDelay {
    pub(crate) const BYTES: usize = CARRY_CAP * size_of::<Pending>() + size_of::<Self>();

    pub(crate) fn new() -> Self {
        Self {
            source: None,
            carry: vec![Pending { source: 0, order: 0, event: ALL_OFF }; CARRY_CAP].into_boxed_slice(),
            len: 0,
            clock: 0,
            order: 0,
            stand_in: None,
            skipped: false,
        }
    }

    /// Before the reader's first block from a different instrument, release the old stream and
    /// discard its delayed events. The reader contract already requires `all_notes_off` to release
    /// everything; the Filter keeps its envelope tail. Pedals are lifted through the fixed note
    /// view. Intentional discards, like reset/bypass, are not overflow losses.
    pub(crate) fn follow_source(&mut self, source: Option<crate::SlotGen>, plugin: &mut dyn Plugin) {
        self.skipped = false;
        if self.source != source {
            if self.source.is_some() {
                crate::note_view::NoteView::release_reader(plugin);
                self.clear();
            }
            self.source = source;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.clock = 0;
        self.order = 0;
        self.stand_in = None;
        self.skipped = false;
    }

    /// Release once per run of skipped insert blocks. Discard the carry: a pending pedal-down
    /// would undo the release, and a pending start may have its note-off missed during bypass.
    /// Whole-strip pool skips do not enter here; their undelivered stream still arrives late.
    pub(crate) fn skip(&mut self, plugin: &mut dyn Plugin) {
        if !self.skipped {
            crate::note_view::NoteView::release_reader(plugin);
            self.clear();
            self.skipped = true;
        }
    }

    fn record(&mut self, e: &TimedEvent) -> u64 {
        let release = is_release(&e.event.message);
        let full = self.stand_in.is_some();
        if full && !release || self.len == CARRY_CAP {
            return 1;
        }
        let mut event = e.event;
        let mut source = self.clock.saturating_add(u64::from(e.offset));
        let lost = !full && (self.len + 1 >= CARRY_CAP || !release && self.len + 1 >= CARRY_CAP - RELEASE_RESERVE);
        if lost {
            // The stand-in follows every queued start, including out-of-order source offsets.
            source = self.carry[..self.len].iter().fold(source, |at, p| at.max(p.source));
            event = ALL_OFF;
            self.stand_in = Some(self.order);
        }
        self.carry[self.len] = Pending { source, order: self.order, event };
        self.order = self.order.saturating_add(1);
        self.len += 1;
        u64::from(lost)
    }

    /// Puts the events `record` appended after `old` into (source, order) place in the sorted
    /// carry. The new events are sorted among themselves (at most `NOTE_VIEW_CAP` of them), then
    /// merged from the back, which moves only the entries later than the earliest new event: a
    /// block's events normally come after everything pending, so nothing moves. Result and cost
    /// bound: identical to sorting the whole carry (the key is unique), in O(k log k + n) with
    /// k <= 512 new and n <= 16,384 pending, one pass of at most `CARRY_CAP` 32-byte moves.
    /// Measured in release on the dev box (#247 minors): with 16,250 pending and 64 new events
    /// sorting before all of them 22 us worst, interleaved 53 us worst; the full re-sort it
    /// replaces took 607 us worst.
    fn merge_new(&mut self, old: usize) {
        let k = self.len - old;
        if k == 0 {
            return;
        }
        let key = |p: &Pending| (p.source, p.order);
        if k > NOTE_VIEW_CAP {
            // Unreachable (a block's view holds at most NOTE_VIEW_CAP); never allocate for it.
            self.carry[..self.len].sort_unstable_by_key(key);
            return;
        }
        self.carry[old..self.len].sort_unstable_by_key(key);
        if old == 0 || key(&self.carry[old - 1]) < key(&self.carry[old]) {
            return;
        }
        let mut new = [self.carry[old]; NOTE_VIEW_CAP];
        new[..k].copy_from_slice(&self.carry[old..self.len]);
        let (mut i, mut j, mut w) = (old, k, self.len);
        while j > 0 {
            w -= 1;
            if i > 0 && key(&self.carry[i - 1]) > key(&new[j - 1]) {
                i -= 1;
                self.carry[w] = self.carry[i];
            } else {
                j -= 1;
                self.carry[w] = new[j];
            }
        }
    }

    /// A reader sees the instrument's accepted stream delayed by its accumulated upstream audio
    /// latency, read from the same running sum as PDC, including the delay before a keyed slot.
    /// Every block recomputes pending due frames from their source frames and the current latency;
    /// a reduction makes overdue events arrive at offset zero, in source order. Matched note-offs
    /// stay at least one sample after their note-ons (#257); if that would exceed this block, the
    /// release and everything behind it stay in the carry. Subsequent offsets never decrease.
    /// Zero delay with
    /// no carry forwards the original view unchanged. The carry holds CARRY_CAP events, reserving
    /// 64 for releases and its final open entry for an all-notes-off on overflow. Only actual
    /// drops are counted. At most 512 events are delivered per block, with 64 release slots;
    /// excess due events stay queued and arrive late, never early. Delays beyond MAX_LATENCY are
    /// represented exactly (no time-based truncation). Reset and bypass discard the carry when
    /// the reader itself is reset or released. All storage is prepared off the audio thread.
    pub(crate) fn deliver(&mut self, notes: &[TimedEvent], latency: usize, frames: usize, plugin: &mut dyn Plugin) -> u64 {
        if latency == 0 && self.len == 0 {
            for e in notes {
                plugin.event(e.offset, &e.event);
            }
            self.clock = self.clock.saturating_add(frames as u64);
            return 0;
        }
        let old = self.len;
        let mut dropped = 0;
        for e in notes {
            dropped += self.record(e);
        }
        self.merge_new(old);
        let end = self.clock.saturating_add(frames as u64);
        let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
        let mut offsets = [0u32; NOTE_VIEW_CAP];
        let mut floor = 0;
        let mut sent = 0;
        let mut stand_in_sent = false;
        for p in &self.carry[..self.len] {
            let due = p.source.saturating_add(latency as u64);
            if due >= end || sent >= NOTE_VIEW_CAP || !is_release(&p.event.message) && sent >= NOTE_VIEW_CAP - RELEASE_RESERVE - 1 {
                break;
            }
            let mut offset = u32::try_from(due.saturating_sub(self.clock)).unwrap_or(u32::MAX).max(floor);
            if let Message::NoteOff { key, .. } = p.event.message {
                let on = self.carry[..sent].iter().rposition(|q| {
                    matches!(q.event.message, Message::NoteOn { key: k, .. } if k == key)
                        && q.event.channel == p.event.channel
                        && (p.event.note_id < 0 || q.event.note_id == p.event.note_id)
                });
                let Some(at) = past_its_note_on(offset, on.map(|i| offsets[i]), last) else { break };
                offset = at;
            }
            plugin.event(offset, &p.event);
            stand_in_sent |= self.stand_in == Some(p.order);
            offsets[sent] = offset;
            floor = offset;
            sent += 1;
        }
        self.carry.copy_within(sent..self.len, 0);
        self.len -= sent;
        // Everything queued before the stand-in has gone out with it, so the carry accepts starts
        // again; a dropped start was covered by that all-notes-off. Releases queued behind it stay.
        if stand_in_sent {
            self.stand_in = None;
        }
        self.clock = end;
        dropped
    }
}

#[cfg(test)]
#[path = "note_delay_tests.rs"]
mod tests;

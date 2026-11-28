//! A hosted plug-in's note output, as its adapter hands it to a MIDI effect stage (ADR 0064,
//! decisions 3 and 6), and the report of the native calls a block made.
//!
//! The adapter reads the plug-in's first readable note output after each native call and keeps
//! every event it wrote in a [`HostedOutBuffer`]: the plug-in's own signed addresses (port,
//! channel, key, note id; `-1` kept as written), its values in the format's units, an offset in
//! the block (the call's start added, clamped into the call as the host's policy), and a sequence
//! number in write order. Nothing here matches, pairs or translates: that is the stage's (ADR 0064,
//! decision 5). Events the record drops are handed out too, as [`OutKind::Dropped`] with their
//! reason, and counted in the block's [`CallReport`].

/// Events one block's buffer holds: the native output list's capacity (ADR 0064, decision 3).
pub const HOSTED_OUT_CAP: usize = 1024;

/// Runs of identical calls a [`CallReport`] holds. A block needs at most three: the instance's
/// core is held across all its calls, so they share one generation; only the first can follow the
/// stage's reset; and the first call that reports an error ends the block. Every call of a block,
/// however many it makes, is described by its run.
pub const REPORT_RUNS: usize = 4;

/// What an output event is, in the format's own terms (ADR 0064, decision 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutKind {
    NoteOn,
    NoteOff,
    /// A CLAP `NOTE_CHOKE`.
    Choke,
    PolyPressure,
    Controller,
    ChannelPressure,
    PitchBend,
    Program,
    /// Dropped by the record's table, with the reason.
    Dropped(OutDrop),
}

/// Why an output event is dropped (each row of ADR 0064's translation table that drops, and an
/// output other than the one the stage reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutDrop {
    /// Addressed to a bus or port other than the first readable one (`other_output`).
    OtherOutput,
    /// A VST3 legacy controller event for `kPitchBend` (129): no public document states its layout.
    LegacyPitchBend,
    /// A VST3 legacy controller event for `kCtrlPolyPressure` (131): no public document states it.
    LegacyPolyPressure,
    /// A VST3 legacy controller event with any other number above 127 but 128 and 130.
    OtherController,
    /// A VST3 `kDataEvent`.
    Data,
    /// A note expression other than pressure (VST3 note expressions, CLAP non-pressure ones).
    NoteExpression,
    /// A VST3 `kChordEvent`.
    Chord,
    /// A VST3 `kScaleEvent`.
    Scale,
    /// A CLAP `MIDI_SYSEX`.
    Sysex,
    /// A CLAP `MIDI2`.
    Midi2,
    /// Any other event type, or a MIDI byte message that is not a channel voice message.
    Unknown,
}

impl OutDrop {
    /// How many reasons there are (the length of [`CallReport::dropped`]).
    pub const COUNT: usize = 11;

    /// The reason's index in [`CallReport::dropped`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            OutDrop::OtherOutput => 0,
            OutDrop::LegacyPitchBend => 1,
            OutDrop::LegacyPolyPressure => 2,
            OutDrop::OtherController => 3,
            OutDrop::Data => 4,
            OutDrop::NoteExpression => 5,
            OutDrop::Chord => 6,
            OutDrop::Scale => 7,
            OutDrop::Sysex => 8,
            OutDrop::Midi2 => 9,
            OutDrop::Unknown => 10,
        }
    }
}

/// How the plug-in wrote the event: a format event (a VST3 event or a CLAP note event) or MIDI
/// 1.0 bytes (a CLAP MIDI event).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutForm {
    Native,
    MidiBytes,
}

/// An event's values, in the format's units, untouched (the stage converts after matching).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutValues {
    /// A note's velocity (0..1 as written; not clamped).
    Velocity(f64),
    /// A poly pressure (VST3 `pressure`, CLAP pressure expression value).
    Pressure(f64),
    /// A VST3 legacy controller event: its controller number and `value` as written (`value2` is
    /// never read).
    Controller { number: u8, value: i8 },
    /// A CLAP MIDI event's three bytes.
    Midi([u8; 3]),
    /// Nothing the record reads.
    None,
}

/// One event a hosted plug-in wrote on its note output (ADR 0064, decision 3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostedOut {
    /// Frame in the block: the call's start plus the offset written, clamped into the call.
    pub offset: u32,
    /// Write order in the block, from 0.
    pub seq: u32,
    pub kind: OutKind,
    /// The bus (VST3) or note port (CLAP) written, signed, as written.
    pub port: i32,
    pub channel: i16,
    pub key: i16,
    pub note_id: i32,
    pub form: OutForm,
    pub values: OutValues,
}

/// The note output a configured instance has: the index of the output the stage reads (the
/// first readable one) and how many note outputs the plug-in declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteOutputInfo {
    pub read: u32,
    pub declared: u32,
}

/// The instance's lifecycle (ADR 0064, decision 6): a generation bumped at every activation
/// attempt, every state load applied to the processor and every `prepare`, and whether the last
/// activation succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lifecycle {
    pub generation: u64,
    pub active: bool,
}

/// One native call of a block, as its adapter saw it under the instance's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallRecord {
    /// The lifecycle generation the call ran under.
    pub generation: u64,
    /// The stage's pending reset ([`crate::Plugin::restart_processing`]) ran before this call's
    /// `process`.
    pub reset_before: bool,
    /// The call (or the reset before it) reported a processing error.
    pub error: bool,
}

/// A run of consecutive native calls of one block with the same record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallRun {
    pub record: CallRecord,
    /// How many consecutive calls (at least one).
    pub calls: u32,
}

/// What the native calls of the last block did (ADR 0064, decisions 3 and 6). Every call made is
/// described: [`CallReport::runs`] holds them as runs of identical records, in call order, and
/// [`CallReport::calls`] expands them one record per call. `made + skipped` is the number of
/// native calls the block was due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallReport {
    /// Native calls made.
    pub made: u32,
    /// Native calls the block was due and did not make: all of them when the block was skipped
    /// (the instance held by its main thread, parked, inactive, never prepared, or its processing
    /// refused to start), and those after a call that reported a processing error (the block ends
    /// there). A block of no frames is due none.
    pub skipped: u32,
    /// Any call of the block reported a processing error.
    pub error: bool,
    /// Events whose offset was clamped into their call.
    pub offset_clamped: u32,
    /// Events the full buffer refused.
    pub overflow: u32,
    /// Dropped events by reason ([`OutDrop::index`]).
    pub dropped: [u32; OutDrop::COUNT],
    runs: [CallRun; REPORT_RUNS],
    run_count: usize,
}

impl Default for CallReport {
    fn default() -> Self {
        CallReport {
            made: 0,
            skipped: 0,
            error: false,
            offset_clamped: 0,
            overflow: 0,
            dropped: [0; OutDrop::COUNT],
            runs: [CallRun::default(); REPORT_RUNS],
            run_count: 0,
        }
    }
}

impl CallReport {
    /// The block's calls as runs of identical records, in call order.
    #[must_use]
    pub fn runs(&self) -> &[CallRun] {
        self.runs.get(..self.run_count).unwrap_or(&[])
    }

    /// Every call made, one record per call, in call order.
    pub fn calls(&self) -> impl Iterator<Item = CallRecord> + '_ {
        self.runs().iter().flat_map(|r| std::iter::repeat_n(r.record, usize::try_from(r.calls).unwrap_or(0)))
    }

    /// Records a native call: it extends the last run when its record is the same, else starts a
    /// run. A block cannot need more than [`REPORT_RUNS`] runs (see there); were it to, the call
    /// is folded into the last run with that run marked failed, so the report never claims a
    /// clean call it cannot describe.
    pub fn record(&mut self, call: CallRecord) {
        self.made = self.made.saturating_add(1);
        self.error |= call.error;
        if let Some(last) = self.run_count.checked_sub(1).and_then(|i| self.runs.get_mut(i))
            && last.record == call
        {
            last.calls = last.calls.saturating_add(1);
            return;
        }
        if let Some(slot) = self.runs.get_mut(self.run_count) {
            *slot = CallRun { record: call, calls: 1 };
            self.run_count += 1;
        } else if let Some(last) = self.runs.last_mut() {
            last.calls = last.calls.saturating_add(1);
            last.record.error = true;
            self.error = true;
        }
    }
}

/// The block output buffer an adapter keeps: the events of the last block's native calls (at
/// most [`HOSTED_OUT_CAP`], allocated by [`HostedOutBuffer::new`] or [`HostedOutBuffer::reserve`],
/// never on the audio thread) and their report. [`HostedOutBuffer::empty`] holds no events: it
/// refuses every push, and only reports calls.
pub struct HostedOutBuffer {
    events: Vec<HostedOut>,
    report: CallReport,
}

impl Default for HostedOutBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl HostedOutBuffer {
    #[must_use]
    pub fn new() -> Self {
        HostedOutBuffer { events: Vec::with_capacity(HOSTED_OUT_CAP), report: CallReport::default() }
    }

    /// A buffer with no event storage (nothing allocated): it reports calls only.
    #[must_use]
    pub fn empty() -> Self {
        HostedOutBuffer { events: Vec::new(), report: CallReport::default() }
    }

    /// Allocates the event storage if it is not there (control thread only: it allocates).
    pub fn reserve(&mut self) {
        if self.events.capacity() < HOSTED_OUT_CAP {
            self.events.reserve_exact(HOSTED_OUT_CAP - self.events.len());
        }
    }

    /// The buffer's allocated capacity (storage accounting).
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }

    /// A new block: the previous block's events and report are dropped (a block's output is its
    /// own; one nobody took is gone).
    pub fn begin_block(&mut self) {
        self.events.clear();
        self.report = CallReport::default();
    }

    /// The most events the buffer takes: [`HOSTED_OUT_CAP`], or 0 without storage.
    fn cap(&self) -> usize {
        HOSTED_OUT_CAP.min(self.events.capacity())
    }

    /// Room left in the block.
    #[must_use]
    pub fn room(&self) -> usize {
        self.cap().saturating_sub(self.events.len())
    }

    /// Appends an event the plug-in wrote at `local` (its offset in the call, as written) in the
    /// call of `frames` frames that starts at block frame `start`. The host policy: an offset
    /// below 0 becomes 0 and one at or past `frames` the call's last frame, both counted. False
    /// (counted) when the block's buffer is full. `ev.offset` and `ev.seq` are set here.
    pub fn push(&mut self, local: i64, start: u32, frames: u32, mut ev: HostedOut) -> bool {
        if self.events.len() >= self.cap() {
            self.report.overflow = self.report.overflow.saturating_add(1);
            return false;
        }
        let last = i64::from(frames.max(1) - 1);
        let at = local.clamp(0, last);
        if at != local {
            self.report.offset_clamped = self.report.offset_clamped.saturating_add(1);
        }
        ev.offset = start.saturating_add(u32::try_from(at).unwrap_or(0));
        ev.seq = u32::try_from(self.events.len()).unwrap_or(u32::MAX);
        if let OutKind::Dropped(why) = ev.kind
            && let Some(n) = self.report.dropped.get_mut(why.index())
        {
            *n = n.saturating_add(1);
        }
        self.events.push(ev);
        true
    }

    /// Counts events refused before they reached the buffer (a native list that was full).
    pub fn refused(&mut self, n: u32) {
        self.report.overflow = self.report.overflow.saturating_add(n);
    }

    /// Records a native call.
    pub fn record_call(&mut self, call: CallRecord) {
        self.report.record(call);
    }

    /// Counts native calls the block was due and did not make.
    pub fn skipped(&mut self, calls: u32) {
        self.report.skipped = self.report.skipped.saturating_add(calls);
    }

    /// Hands the events in write order to `out` until it returns false, clears the buffer, and
    /// returns the block's report (the report stays until the next block).
    pub fn take(&mut self, out: &mut dyn FnMut(HostedOut) -> bool) -> CallReport {
        for e in self.events.drain(..) {
            if !out(e) {
                break;
            }
        }
        self.events.clear();
        self.report
    }
}

/// Native calls a block of `frames` makes at `max_block` frames per call (the calls a skipped
/// block was due). One when the instance was never prepared.
#[must_use]
pub fn calls_due(frames: usize, max_block: usize) -> u32 {
    if frames == 0 {
        return 0;
    }
    u32::try_from(frames.div_ceil(max_block.max(1))).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(key: i16) -> HostedOut {
        HostedOut {
            offset: 0,
            seq: 0,
            kind: OutKind::NoteOn,
            port: 0,
            channel: 0,
            key,
            note_id: -1,
            form: OutForm::Native,
            values: OutValues::Velocity(1.0),
        }
    }

    #[test]
    fn offsets_are_clamped_into_their_call_counted_and_numbered_in_write_order() {
        let mut b = HostedOutBuffer::new();
        b.begin_block();
        for (k, local) in [-1i64, 0, 15, 16, i64::from(i32::MAX)].into_iter().enumerate() {
            assert!(b.push(local, 32, 16, on(k as i16)));
        }
        let mut got = Vec::new();
        let r = b.take(&mut |e| {
            got.push((e.offset, e.seq));
            true
        });
        assert_eq!(got, vec![(32, 0), (32, 1), (47, 2), (47, 3), (47, 4)]);
        assert_eq!(r.offset_clamped, 3);
    }

    #[test]
    fn a_full_buffer_refuses_and_counts_and_a_new_block_starts_empty() {
        let mut b = HostedOutBuffer::new();
        b.begin_block();
        for _ in 0..HOSTED_OUT_CAP {
            assert!(b.push(0, 0, 1, on(60)));
        }
        assert!(!b.push(0, 0, 1, on(61)), "the 1,025th event is refused");
        b.refused(2);
        let mut n = 0;
        let r = b.take(&mut |_| {
            n += 1;
            true
        });
        assert_eq!((n, r.overflow), (HOSTED_OUT_CAP, 3));
        b.begin_block();
        assert_eq!(b.take(&mut |_| true), CallReport::default());
    }

    #[test]
    fn the_report_keeps_each_call_and_counts_drops_by_reason() {
        let mut b = HostedOutBuffer::new();
        b.begin_block();
        b.record_call(CallRecord { generation: 4, reset_before: true, error: false });
        b.record_call(CallRecord { generation: 4, reset_before: false, error: true });
        let mut dropped = on(1);
        dropped.kind = OutKind::Dropped(OutDrop::Sysex);
        assert!(b.push(0, 0, 16, dropped));
        let r = b.take(&mut |_| true);
        assert_eq!(r.made, 2);
        assert_eq!(
            r.calls().collect::<Vec<_>>(),
            [CallRecord { generation: 4, reset_before: true, error: false }, CallRecord { generation: 4, reset_before: false, error: true }]
        );
        assert!(r.error);
        assert_eq!(r.dropped[OutDrop::Sysex.index()], 1);
        assert_eq!(calls_due(64, 16), 4);
        assert_eq!(calls_due(0, 16), 0);
        assert_eq!(calls_due(65, 16), 5);
    }

    /// Every call of a block of any length is described: a reset call, 200 ordinary calls and a
    /// failing one are three runs, and the expansion has 202 records in call order.
    #[test]
    fn every_call_of_a_long_block_is_described() {
        let mut r = CallReport::default();
        r.record(CallRecord { generation: 7, reset_before: true, error: false });
        for _ in 0..200 {
            r.record(CallRecord { generation: 7, reset_before: false, error: false });
        }
        r.record(CallRecord { generation: 7, reset_before: false, error: true });
        assert_eq!(r.made, 202);
        assert_eq!(r.runs().iter().map(|x| x.calls).collect::<Vec<_>>(), [1, 200, 1]);
        let all: Vec<CallRecord> = r.calls().collect();
        assert_eq!(all.len(), 202);
        assert!(all[0].reset_before && !all[1].reset_before && !all[200].error && all[201].error);
        assert!(r.error);
    }

    #[test]
    fn an_empty_buffer_reports_calls_and_refuses_events_until_reserved() {
        let mut b = HostedOutBuffer::empty();
        assert_eq!((b.capacity(), b.room()), (0, 0));
        b.begin_block();
        assert!(!b.push(0, 0, 16, on(60)));
        b.record_call(CallRecord::default());
        let r = b.take(&mut |_| true);
        assert_eq!((r.made, r.overflow), (1, 1));
        b.reserve();
        assert_eq!(b.room(), HOSTED_OUT_CAP);
    }
}

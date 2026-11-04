//! What the core is told in a block: the [`Context`] and a borrowed span of time-stamped
//! [`Event`]s.
//!
//! Events act on their exact output frame, in span order: the caller sorts them by frame, stably,
//! so events at one offset keep their incoming order (the last admitted trigger of a choke group at
//! an offset wins). An event at or past the block's length acts after its last frame. The event
//! queue with its 1024 entries and 257-entry release reserve is the adapter's; the core only
//! consumes spans and never sorts, copies or stores them.

/// The block's explicit inputs besides the events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Context {
    /// The absolute frame of the block's first frame since the last reset (free-running LFO phases
    /// at a note start are worked out from it, so they do not depend on block boundaries).
    pub frame: u64,
    /// Beats per minute.
    pub tempo: f64,
    /// The song position in quarter notes at the block's first frame while the transport plays
    /// (tempo-synced, free-running LFOs anchor to it, ADR 0038); `None` when stopped.
    pub quarters: Option<f64>,
}

impl Default for Context {
    fn default() -> Self {
        Context { frame: 0, tempo: 120.0, quarters: None }
    }
}

/// A channel controller or a rack macro (bounded numeric updates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// The sustain pedal (any channel).
    Sustain(bool),
    /// 0..1.
    ModWheel(f32),
    /// -1..1.
    PitchBend(f32),
    /// Channel pressure, 0..1.
    Aftertouch(f32),
    /// Note pressure of `key`, 0..1.
    PolyAftertouch { key: u8, value: f32 },
    /// Rack macro `index` (0..8), 0..1.
    Macro { index: u8, value: f32 },
}

/// What happens at an event's frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventKind {
    /// A note: triggers the pad whose trigger note is `key` and the unbound zones whose ranges hold
    /// it. Velocity 0 is a note-off. `id` is the host's note id (negative: none).
    NoteOn {
        channel: u8,
        key: u8,
        velocity: u8,
        id: i32,
    },
    /// A note-off: releases the voices the note started (by `id` when it is not negative, else by
    /// channel and key). `velocity` is the release velocity.
    NoteOff {
        channel: u8,
        key: u8,
        velocity: u8,
        id: i32,
    },
    /// A pad pressed directly (a controller pad or the panel), by pad index.
    PadPress {
        pad: u8,
        velocity: u8,
    },
    /// The pad press let go.
    PadRelease {
        pad: u8,
    },
    Control(Control),
    /// Releases Gate voices; one-shots fade over [`crate::CHOKE_FRAMES`].
    AllNotesOff,
    /// Cuts every voice at the frame.
    AllSoundOff,
    /// The transport stopped (or located): every voice fades over [`crate::CHOKE_FRAMES`], the
    /// sustain pedal lifts.
    Stop,
    /// Back to the reset state: every voice cut, controllers cleared, voices' random state reseeded.
    Reset,
}

/// An event at output frame `frame` of the block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub frame: u32,
    pub kind: EventKind,
}

impl Event {
    #[must_use]
    pub fn new(frame: u32, kind: EventKind) -> Event {
        Event { frame, kind }
    }

    /// A note-on without a host note id.
    #[must_use]
    pub fn on(frame: u32, key: u8, velocity: u8) -> Event {
        Event::new(frame, EventKind::NoteOn { channel: 0, key, velocity, id: -1 })
    }

    /// A note-off without a host note id.
    #[must_use]
    pub fn off(frame: u32, key: u8) -> Event {
        Event::new(frame, EventKind::NoteOff { channel: 0, key, velocity: 64, id: -1 })
    }
}

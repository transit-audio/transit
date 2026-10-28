//! The audio callback's phase marker (issue #162): which phase, and which strip, the callback is
//! in, written by the callback itself with relaxed stores and read by whoever records the
//! callback's duration. No allocation, no lock, no syscall: one clock read and a few plain
//! atomic stores per phase change.
//!
//! The marker also keeps, for the callback in flight, the longest stretch between two phase
//! changes and the phase (and strip) that stretch was spent in. At the end of the callback the
//! current phase is already `Idle`, so that stretch, not the current phase, is what names the
//! cause of a long callback. The fields are read independently, so a reader that races a writer
//! can see a phase from one change and a strip from the next; the figures are approximate.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

/// The strip index of a phase that belongs to no strip.
pub const NO_STRIP: u32 = u32::MAX;

/// What the callback is doing.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Between callbacks, or the callback has finished.
    #[default]
    Idle = 0,
    /// Servicing the command lanes, plan adoption and the transport.
    Service = 1,
    /// Dispatching live MIDI to the mix.
    MidiDispatch = 2,
    /// Rendering a strip: its source, inserts and fader (strip index set).
    StripRender = 3,
    /// Mixdown work that belongs to no strip: clearing, delay compensation, routing, meters.
    Mixdown = 4,
    /// Rendering an aux strip (strip index set).
    Auxes = 5,
    /// Processing a master strip (strip index set).
    Masters = 6,
    /// Copying the mix to the output block and converting it for the device.
    Output = 7,
    /// The device hand-off after our render: probes and the backend's conversion.
    DeviceIo = 8,
}

impl Phase {
    /// The phase stored as `code`; an unknown code reads as `Idle`.
    #[must_use]
    pub const fn from_code(code: u8) -> Phase {
        match code {
            1 => Phase::Service,
            2 => Phase::MidiDispatch,
            3 => Phase::StripRender,
            4 => Phase::Mixdown,
            5 => Phase::Auxes,
            6 => Phase::Masters,
            7 => Phase::Output,
            8 => Phase::DeviceIo,
            _ => Phase::Idle,
        }
    }

    /// The name the diagnostics report prints.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Service => "service",
            Phase::MidiDispatch => "MIDI dispatch",
            Phase::StripRender => "strip render",
            Phase::Mixdown => "mixdown",
            Phase::Auxes => "aux render",
            Phase::Masters => "master render",
            Phase::Output => "output copy",
            Phase::DeviceIo => "device I/O",
        }
    }

    /// True for the phases that run plug-in code for one strip.
    #[must_use]
    pub const fn runs_strip(self) -> bool {
        matches!(self, Phase::StripRender | Phase::Auxes | Phase::Masters)
    }
}

/// What a reader sees of the marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// The phase now.
    pub phase: Phase,
    /// The strip now ([`NO_STRIP`] when none).
    pub strip: u32,
    /// When the phase last changed, in nanoseconds on the marker's clock.
    pub at_ns: u64,
    /// The longest stretch between two phase changes since [`PhaseMarker::begin`].
    pub longest_gap_ns: u64,
    /// The phase that stretch was spent in.
    pub gap_phase: Phase,
    /// The strip that stretch was spent in ([`NO_STRIP`] when none).
    pub gap_strip: u32,
}

impl Default for Stamp {
    fn default() -> Self {
        Self { phase: Phase::Idle, strip: NO_STRIP, at_ns: 0, longest_gap_ns: 0, gap_phase: Phase::Idle, gap_strip: NO_STRIP }
    }
}

/// The marker. One per output stream; shared by the callback (writer) and any reader.
#[derive(Debug)]
pub struct PhaseMarker {
    phase: AtomicU8,
    strip: AtomicU32,
    at_ns: AtomicU64,
    longest_gap_ns: AtomicU64,
    gap_phase: AtomicU8,
    gap_strip: AtomicU32,
    epoch: std::time::Instant,
}

impl Default for PhaseMarker {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseMarker {
    /// A marker in `Idle`, its clock starting now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: AtomicU8::new(Phase::Idle as u8),
            strip: AtomicU32::new(NO_STRIP),
            at_ns: AtomicU64::new(0),
            longest_gap_ns: AtomicU64::new(0),
            gap_phase: AtomicU8::new(Phase::Idle as u8),
            gap_strip: AtomicU32::new(NO_STRIP),
            epoch: std::time::Instant::now(),
        }
    }

    /// Nanoseconds on the marker's clock (0 where the platform has no monotonic clock).
    #[must_use]
    pub fn now_ns(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// A callback starts: forget the previous callback's longest stretch and enter `Service`.
    pub fn begin(&self) {
        self.begin_at(self.now_ns());
    }

    /// [`Self::begin`] with the time given.
    pub fn begin_at(&self, now_ns: u64) {
        self.longest_gap_ns.store(0, Ordering::Relaxed);
        self.gap_phase.store(Phase::Idle as u8, Ordering::Relaxed);
        self.gap_strip.store(NO_STRIP, Ordering::Relaxed);
        self.strip.store(NO_STRIP, Ordering::Relaxed);
        self.phase.store(Phase::Service as u8, Ordering::Relaxed);
        self.at_ns.store(now_ns, Ordering::Relaxed);
    }

    /// The callback moves to `phase` (and `strip`, when the phase belongs to one).
    pub fn enter(&self, phase: Phase, strip: Option<u32>) {
        self.enter_at(phase, strip, self.now_ns());
    }

    /// [`Self::enter`] with the time given.
    pub fn enter_at(&self, phase: Phase, strip: Option<u32>, now_ns: u64) {
        let gap = now_ns.saturating_sub(self.at_ns.load(Ordering::Relaxed));
        if gap > self.longest_gap_ns.load(Ordering::Relaxed) {
            self.longest_gap_ns.store(gap, Ordering::Relaxed);
            self.gap_phase.store(self.phase.load(Ordering::Relaxed), Ordering::Relaxed);
            self.gap_strip.store(self.strip.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.phase.store(phase as u8, Ordering::Relaxed);
        self.strip.store(strip.unwrap_or(NO_STRIP), Ordering::Relaxed);
        self.at_ns.store(now_ns, Ordering::Relaxed);
    }

    /// The callback finished: close the last stretch and go `Idle`.
    pub fn end(&self) {
        self.enter(Phase::Idle, None);
    }

    /// The marker as it is now.
    #[must_use]
    pub fn read(&self) -> Stamp {
        Stamp {
            phase: Phase::from_code(self.phase.load(Ordering::Relaxed)),
            strip: self.strip.load(Ordering::Relaxed),
            at_ns: self.at_ns.load(Ordering::Relaxed),
            longest_gap_ns: self.longest_gap_ns.load(Ordering::Relaxed),
            gap_phase: Phase::from_code(self.gap_phase.load(Ordering::Relaxed)),
            gap_strip: self.gap_strip.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_stretch_names_the_phase_it_was_spent_in() {
        let m = PhaseMarker::new();
        m.begin_at(0);
        m.enter_at(Phase::StripRender, Some(0), 100);
        m.enter_at(Phase::StripRender, Some(1), 600);
        m.enter_at(Phase::Output, None, 19_000);
        m.enter_at(Phase::Idle, None, 19_100);
        let s = m.read();
        assert_eq!((s.longest_gap_ns, s.gap_phase, s.gap_strip), (18_400, Phase::StripRender, 1));
        assert_eq!((s.phase, s.strip), (Phase::Idle, NO_STRIP));
    }

    #[test]
    fn begin_forgets_the_previous_callback() {
        let m = PhaseMarker::new();
        m.begin_at(0);
        m.enter_at(Phase::Idle, None, 5_000);
        m.begin_at(6_000);
        assert_eq!(m.read().longest_gap_ns, 0);
    }
}

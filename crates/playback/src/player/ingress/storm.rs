//! Feedback storm detector of one input port (#99). The ingress worker counts what the port
//! delivers per window of arrival time; a port that stays far above any human or sequencer source
//! for [`WINDOWS`] windows in a row is muted until the user re-enables it. Counters only: no
//! allocation, lock or system call. The rationale of every bound is on its constant.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The counting window, in nanoseconds of the events' host arrival time.
pub const WINDOW_NS: u64 = 50_000_000;
/// Consecutive hot windows that mute the port: 200 ms of storm. A burst (a controller's preset
/// dump, a sequencer's chase at play start) is shorter.
pub const WINDOWS: u32 = 4;
/// Messages per second (Clock and Active Sensing excluded) above which a window is hot. Twice the
/// densest legitimate stream the tests play (a sequencer at 1,000 messages a second), and below
/// the 3,125 three-byte messages a second a DIN link carries, so a loop that saturates a DIN cable
/// still trips it.
pub const RATE: u32 = 2_000;
/// One identical message (same status and data) per second above which a window is also hot. A
/// loop through a device's MIDI thru repeats each message once per round trip (about 1 ms in
/// #99's capture: 1,000 a second per looping message), which stays under [`RATE`] while only a
/// few messages loop; this bound still catches a round trip as slow as 4 ms. No musical source
/// repeats one identical message that fast: a 1/128 ratchet at 300 BPM is 160 note-ons a second,
/// and controllers send a held value only when it changes.
pub const REPEAT: u32 = 250;
const RATE_PER_WINDOW: u32 = RATE / (1_000_000_000 / WINDOW_NS as u32);
const REPEAT_PER_WINDOW: u32 = REPEAT / (1_000_000_000 / WINDOW_NS as u32);
/// Distinct messages tracked per window for [`REPEAT`]; the least seen is replaced.
const SLOTS: usize = 4;

/// The detector of one port.
#[derive(Debug, Default)]
pub struct Storm {
    window: u64,
    count: u32,
    /// (message packed with its length, times seen in this window).
    seen: [(u32, u32); SLOTS],
    /// Hot windows in a row before the current one.
    hot: u32,
    tripped: bool,
}

impl Storm {
    /// Counts one message that arrived at `arrival_ns`; true once, when the port must be muted.
    /// Clock (`F8`) and Active Sensing (`FE`) never count.
    pub fn note(&mut self, arrival_ns: u64, bytes: [u8; 3], len: u8) -> bool {
        if self.tripped || matches!(bytes[0], 0xF8 | 0xFE) {
            return false;
        }
        let window = arrival_ns / WINDOW_NS;
        if window > self.window {
            self.hot = if self.is_hot() && window == self.window + 1 { self.hot + 1 } else { 0 };
            self.window = window;
            self.count = 0;
            self.seen = [(0, 0); SLOTS];
        }
        self.count = self.count.saturating_add(1);
        let key = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], len]);
        match self.seen.iter_mut().find(|(k, n)| *n > 0 && *k == key) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => {
                if let Some(slot) = self.seen.iter_mut().min_by_key(|(_, n)| *n) {
                    *slot = (key, 1);
                }
            }
        }
        if self.is_hot() && self.hot + 1 >= WINDOWS {
            self.tripped = true;
            return true;
        }
        false
    }

    fn is_hot(&self) -> bool {
        self.count > RATE_PER_WINDOW || self.seen.iter().any(|(_, n)| *n > REPEAT_PER_WINDOW)
    }

    /// The user re-enabled the port: count from scratch.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn tripped(&self) -> bool {
        self.tripped
    }
}

/// A port's detector and the mute flag Main shares with it: the worker sets it when the detector
/// trips, Main clears it when the user re-enables the port.
#[derive(Debug, Default)]
pub struct Guard {
    pub storm: Storm,
    pub muted: Arc<AtomicBool>,
}

impl Guard {
    #[must_use]
    pub fn new(muted: Arc<AtomicBool>) -> Self {
        Self { storm: Storm::default(), muted }
    }

    #[must_use]
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays `per_second` messages a second for `ms` milliseconds, `make(i)` giving the i-th; the
    /// time it tripped, if it did.
    fn play(storm: &mut Storm, per_second: u64, ms: u64, make: impl Fn(u64) -> [u8; 3]) -> Option<u64> {
        let n = per_second * ms / 1000;
        (0..n).find_map(|i| {
            let at = 1_000_000_000 + i * 1_000_000_000 / per_second;
            storm.note(at, make(i), 3).then_some((at - 1_000_000_000) / 1_000_000)
        })
    }

    fn notes(i: u64) -> [u8; 3] {
        // Notes on and off over 4 octaves and 16 channels, as a dense sequencer plays them.
        let key = 36 + (i / 2 % 48) as u8;
        let channel = (i / 96 % 16) as u8;
        if i.is_multiple_of(2) { [0x90 | channel, key, 100] } else { [0x80 | channel, key, 0] }
    }

    #[test]
    fn the_bounds_follow_the_window() {
        assert_eq!((RATE_PER_WINDOW, REPEAT_PER_WINDOW), (100, 12));
    }

    #[test]
    fn a_dense_sequencer_is_never_muted() {
        let mut storm = Storm::default();
        assert_eq!(play(&mut storm, 1_000, 10_000, notes), None);
        // 1,900 a second, just under the bound, for 10 s.
        let mut storm = Storm::default();
        assert_eq!(play(&mut storm, 1_900, 10_000, notes), None);
    }

    #[test]
    fn a_flood_of_distinct_messages_trips_within_the_windows() {
        let mut storm = Storm::default();
        let at = play(&mut storm, 3_000, 1_000, notes).expect("muted");
        assert!(at < u64::from(WINDOWS) * WINDOW_NS / 1_000_000, "{at} ms");
        assert!(storm.tripped());
        assert!(!storm.note(5_000_000_000, [0x90, 1, 1], 3), "trips once");
        storm.reset();
        assert!(!storm.tripped());
    }

    #[test]
    fn one_message_looping_at_the_round_trip_trips_on_repeats() {
        // #99: CC 64 0 and CC 123 coming back every millisecond.
        let mut storm = Storm::default();
        let at = play(&mut storm, 2_000, 1_000, |i| if i.is_multiple_of(2) { [0xB0, 64, 0] } else { [0xB0, 123, 0] });
        assert!(at.is_some_and(|ms| ms < 200), "{at:?}");
        // One looping note at 1,000 a second, well under RATE, and at a 4 ms round trip.
        for per_second in [1_000, 260] {
            let mut storm = Storm::default();
            assert!(play(&mut storm, per_second, 1_000, |_| [0x90, 60, 100]).is_some_and(|ms| ms < 200), "{per_second}");
        }
        // A 1/128 ratchet at 300 BPM: 160 identical note-ons a second, with their note-offs.
        let mut storm = Storm::default();
        assert_eq!(play(&mut storm, 320, 10_000, |i| if i.is_multiple_of(2) { [0x90, 38, 100] } else { [0x80, 38, 0] }), None);
    }

    #[test]
    fn clock_and_active_sensing_never_count() {
        let mut storm = Storm::default();
        // 24 ppqn at 300 BPM is 120 a second; a flood far past any rate counts no more.
        assert_eq!(play(&mut storm, 120, 5_000, |_| [0xF8, 0, 0]), None);
        assert_eq!(play(&mut storm, 20_000, 1_000, |i| if i.is_multiple_of(2) { [0xF8, 0, 0] } else { [0xFE, 0, 0] }), None);
    }

    #[test]
    fn a_short_burst_or_a_gap_resets_the_streak() {
        // 150 ms of flood (three windows), then silence, then another 150 ms: never four in a row.
        let mut storm = Storm::default();
        for start in [0u64, 400] {
            for i in 0..450u64 {
                let at = 1_000_000_000 + start * 1_000_000 + i * 1_000_000_000 / 3_000;
                assert!(!storm.note(at, notes(i), 3));
            }
        }
    }
}

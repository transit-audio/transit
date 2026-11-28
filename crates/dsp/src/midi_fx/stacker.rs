//! Test-only (feature `testing`): the owning stacker, a fixture of the host slices' tests (ADR
//! 0057, decision 13, "Host slices"; slice P3b). Not in the registry and never in the app. Chord
//! with fixed shifts behaves so (3.3, 3.4):
//!
//! - **Note-on** (channel `c`, key `k`, velocity `v`, voice id): an entry of its input ledger
//!   (decision 11, `ledger`), or rejected (nothing written). Its output set is `k` with the input's
//!   voice id, then `k + shift_n` for each slot `n` in order whose shift is not 0, with no voice id;
//!   a key outside 0..=127 or already in the set is left out (2.6, 3.3). Every velocity is `v`.
//! - **Counting** (3.4), per channel and output key: from 0 to 1 a note-on with the entry's identity
//!   for that key, stored as the sounding identity; from n to n + 1 a note-off with the stored
//!   identity, then a note-on with the new identity (a re-strike); from n + 1 to n nothing; from
//!   1 to 0 a note-off with the stored identity.
//! - **Its own output refuses a note-on** (final under slice P3b): that output note is Cancelled
//!   (counted in `cancelled`). From 0 to 1 the key leaves the entry's set, which keeps its other
//!   outputs (2.8); in a re-strike the key stays counted but no longer sounds, so no note-off is
//!   written for it later. A refused note-off is not retried: at the host a release refused by a
//!   full buffer triggers the reset barrier, which resets this fixture.
//! - **Note-off** closing an Open entry releases its set by counting; one closing a rejection
//!   record or nothing is consumed (the last counted, 2.4).
//! - **Everything else** passes at its offset; **between blocks** the same, at offset 0.
//! - **Reset** forgets the ledger and every count (the host has ended the notes).

use super::ledger::{Closed, Ledger};
use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{Message, NoteId, PluginEvent};
use crate::params::lin;
use crate::{Category, ParamInfo, PluginInfo, Unit};

static STACKER_PARAMS: [ParamInfo; 3] = [
    lin("shift_1", "Shift 1", -36.0, 36.0, 0.0, Unit::None),
    lin("shift_2", "Shift 2", -36.0, 36.0, 0.0, Unit::None),
    lin("shift_3", "Shift 3", -36.0, 36.0, 0.0, Unit::None),
];

pub static STACKER_INFO: PluginInfo = PluginInfo {
    id: "midi_test_stacker",
    name: "Test Owning Stacker",
    short_name: "Stack",
    category: Category::Other,
    params: &STACKER_PARAMS,
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// Keys in one entry's output set at most: the played key and three shifts.
const SET: usize = 4;
const NO_KEY: u8 = u8::MAX;

/// Per channel and output key: entries whose set holds the key, and the identity that sounds.
#[derive(Clone, Copy, Debug, Default)]
struct Count {
    n: u16,
    identity: i32,
    sounding: bool,
}

#[derive(Clone, Debug)]
pub struct Stacker {
    shifts: [i8; 3],
    ledger: Ledger<[u8; SET]>,
    counts: [[Count; 128]; 16],
    /// Note-ons its own output refused.
    pub cancelled: u64,
    /// Note-ons it wrote that its output accepted.
    pub written_ons: u64,
}

impl Default for Stacker {
    fn default() -> Self {
        Stacker { shifts: [0; 3], ledger: Ledger::default(), counts: [[Count::default(); 128]; 16], cancelled: 0, written_ons: 0 }
    }
}

fn note(offset: u32, channel: u8, key: u8, voice: i32, velocity: Option<u8>) -> TimedEvent {
    let message = match velocity {
        Some(velocity) => Message::NoteOn { key, velocity, id: NoteId::NONE },
        None => Message::NoteOff { key, velocity: 0, id: NoteId::NONE },
    };
    TimedEvent { offset, event: PluginEvent { channel, note_id: voice, message } }
}

impl Stacker {
    /// Entries in use.
    #[must_use]
    pub fn open(&self) -> usize {
        self.ledger.open()
    }

    /// The most entries in use at once since it was made.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.ledger.peak
    }

    /// Note-offs that closed nothing (consumed).
    #[must_use]
    pub fn unmatched_releases(&self) -> u64 {
        self.ledger.unmatched_releases
    }

    /// Output keys counted on any channel.
    #[must_use]
    pub fn counted(&self) -> usize {
        self.counts.iter().flatten().filter(|c| c.n > 0).count()
    }

    fn strike(&mut self, offset: u32, channel: u8, key: u8, voice: i32, velocity: u8, out: &mut MidiOut<'_>) -> bool {
        let Some(c) = self.counts.get_mut(usize::from(channel)).and_then(|r| r.get_mut(usize::from(key))) else { return false };
        if c.n > 0 && c.sounding {
            out.push(note(offset, channel, key, c.identity, None));
        }
        let accepted = out.push(note(offset, channel, key, voice, Some(velocity)));
        if accepted {
            self.written_ons = self.written_ons.saturating_add(1);
        } else {
            self.cancelled = self.cancelled.saturating_add(1);
        }
        if c.n == 0 && !accepted {
            return false;
        }
        c.n = c.n.saturating_add(1);
        c.identity = voice;
        c.sounding = accepted;
        true
    }

    fn input(&mut self, offset: u32, ev: PluginEvent, out: &mut MidiOut<'_>) {
        let channel = ev.channel & 0x0F;
        let message = match ev.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        match message {
            Message::NoteOn { key, velocity, .. } => {
                let mut set = [NO_KEY; SET];
                set[0] = key;
                for (n, s) in self.shifts.iter().enumerate() {
                    let k = i16::from(key) + i16::from(*s);
                    if *s != 0
                        && (0..=127).contains(&k)
                        && let Ok(k) = u8::try_from(k)
                        && !set.contains(&k)
                        && let Some(slot) = set.get_mut(n + 1)
                    {
                        *slot = k;
                    }
                }
                let Some(i) = self.ledger.admit(channel, key, ev.note_id, set, true) else { return };
                for (n, k) in set.iter().enumerate().filter(|(_, k)| **k != NO_KEY) {
                    let voice = if n == 0 { ev.note_id } else { PluginEvent::NO_ID };
                    if !self.strike(offset, channel, *k, voice, velocity, out)
                        && let Some(slot) = self.ledger.payload_mut(i).and_then(|p| p.get_mut(n))
                    {
                        *slot = NO_KEY;
                    }
                }
            }
            Message::NoteOff { key, .. } => {
                let Closed::Open(set) = self.ledger.close(channel, key, ev.note_id) else { return };
                for k in set.iter().filter(|k| **k != NO_KEY) {
                    let Some(c) = self.counts.get_mut(usize::from(channel)).and_then(|r| r.get_mut(usize::from(*k))) else { continue };
                    c.n = c.n.saturating_sub(1);
                    if c.n == 0 && c.sounding {
                        out.push(note(offset, channel, *k, c.identity, None));
                        c.sounding = false;
                    }
                }
            }
            _ => {
                out.push(TimedEvent { offset, event: ev });
            }
        }
    }
}

impl MidiEffect for Stacker {
    fn info(&self) -> &'static PluginInfo {
        &STACKER_INFO
    }

    fn reset(&mut self) {
        self.ledger.reset();
        self.counts = [[Count::default(); 128]; 16];
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = STACKER_INFO.param(id) else { return false };
        let n = match id {
            "shift_1" => 0,
            "shift_2" => 1,
            _ => 2,
        };
        if let Some(s) = self.shifts.get_mut(n) {
            *s = p.clamp(value).round() as i8;
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let n = match id {
            "shift_1" => 0,
            "shift_2" => 1,
            "shift_3" => 2,
            _ => return None,
        };
        self.shifts.get(n).map(|s| f32::from(*s))
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, _frames: u32) {
        for e in input {
            self.input(e.offset, e.event, out);
        }
    }

    fn process_between(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>) {
        for e in input {
            self.input(0, e.event, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on, run};
    use super::*;

    fn stacker(shifts: &[f32]) -> Stacker {
        let mut s = Stacker::default();
        for (n, v) in shifts.iter().enumerate() {
            assert!(s.set_param(["shift_1", "shift_2", "shift_3"][n], *v));
        }
        s
    }

    fn anon(offset: u32, key: u8, on: bool) -> TimedEvent {
        note(offset, 0, key, PluginEvent::NO_ID, on.then_some(100))
    }

    #[test]
    fn it_stacks_by_the_counting_rule() {
        // Decision 13's Chord table with shifts +4 and +7 at 100 %: "on 60 @0, on 64 @100, off 60
        // @200, off 64 @300", with voice ids 1 and 2.
        let mut s = stacker(&[4.0, 7.0]);
        let out = run(&mut s, &[on(0, 1, 60, 100), on(100, 2, 64, 100), off(200, 1, 60), off(300, 2, 64)]);
        assert_eq!(
            out,
            [
                on(0, 1, 60, 100),
                anon(0, 64, true),
                anon(0, 67, true),
                anon(100, 64, false),
                on(100, 2, 64, 100),
                anon(100, 68, true),
                anon(100, 71, true),
                off(200, 1, 60),
                anon(200, 67, false),
                off(300, 2, 64),
                anon(300, 68, false),
                anon(300, 71, false),
            ]
        );
        assert_eq!((s.open(), s.counted(), s.unmatched_releases()), (0, 0, 0));
        assert!(run(&mut s, &[off(0, 3, 50)]).is_empty(), "a release that closes nothing is consumed");
        assert_eq!(s.unmatched_releases(), 1);
    }

    #[test]
    fn a_refused_note_on_leaves_the_set_and_its_release_writes_nothing_for_it() {
        let mut s = stacker(&[12.0]);
        let mut buf = [TimedEvent::EMPTY; 1];
        let mut out = MidiOut::new(&mut buf);
        s.process(&[on(0, 1, 60, 100)], &mut out, 64);
        assert_eq!(out.events(), [on(0, 1, 60, 100)]);
        assert_eq!((s.cancelled, s.written_ons), (1, 1));
        assert_eq!(run(&mut s, &[off(5, 1, 60)]), [off(5, 1, 60)], "72 never sounded");
        assert_eq!(s.counted(), 0);
    }
}

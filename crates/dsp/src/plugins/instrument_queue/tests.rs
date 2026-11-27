//! ADR 0059 R1 at the shared ledger and the hosted adapters' queue (Decision 6's tests that do not
//! need a native plug-in): a fake plug-in logs every delivered event's identity, offset and call,
//! and the assertions are on the resulting held and sustained state, never only on counts.

use super::delivery::{Class, Delivery, Item, NATIVE_EVENTS, ORDINARY, SAFETY_SLOTS};
use super::ledger::{ENTRIES, Entry, Ledger};
use crate::event::{Message, NoteId, PluginEvent};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn on(channel: u8, key: u8, voice: i32) -> PluginEvent {
    PluginEvent { channel, note_id: voice, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE } }
}

pub(super) fn off(channel: u8, key: u8, voice: i32) -> PluginEvent {
    PluginEvent { channel, note_id: voice, message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE } }
}

pub(super) fn cc(channel: u8, cc: u8, value: u8) -> PluginEvent {
    PluginEvent { channel, note_id: PluginEvent::NO_ID, message: Message::Control { cc, value } }
}

/// A fake plug-in that pairs by native id (CLAP and VST3 note ids), with sustain and sostenuto per
/// channel. It applies events in the order given and logs (call, absolute frame, item).
#[derive(Default, Clone)]
pub(super) struct Fake {
    /// Native id -> (channel, key): keys held down.
    pub held: BTreeMap<i32, (u8, u8)>,
    /// Native id -> (channel, key): released under a pedal, still sounding.
    pub sustained: BTreeMap<i32, (u8, u8)>,
    /// Voices the sostenuto pedal caught when it went down.
    pub caught: BTreeSet<i32>,
    /// Pedal down per channel and pedal (64, 66, 67, 69).
    pub pedal: BTreeMap<(u8, u8), bool>,
    pub log: Vec<(usize, u64, Item)>,
    pub calls: usize,
    pub bend_reset: Vec<u8>,
    pub silenced: Vec<u8>,
}

impl Fake {
    fn down(&self, ch: u8, cc: u8) -> bool {
        self.pedal.get(&(ch, cc)).copied().unwrap_or(false)
    }

    /// Releases the sustained voices of `ch` no pedal still holds.
    fn let_go(&mut self, ch: u8) {
        let (sus, sost) = (self.down(ch, 64), self.down(ch, 66));
        let caught = &self.caught;
        self.sustained.retain(|k, v| v.0 != ch || sus || (sost && caught.contains(k)));
    }

    pub fn apply(&mut self, at: u64, it: &Item) {
        self.log.push((self.calls, at + u64::from(it.offset), *it));
        self.act(it);
    }

    /// Applies one event without logging it.
    pub fn act(&mut self, it: &Item) {
        let ch = it.event.channel;
        match it.event.message {
            Message::NoteOn { key, .. } => {
                self.held.insert(it.native, (ch, key));
            }
            Message::NoteOff { .. } => {
                if let Some(v) = self.held.remove(&it.native)
                    && (self.down(ch, 64) || (self.down(ch, 66) && self.caught.contains(&it.native)))
                {
                    self.sustained.insert(it.native, v);
                }
            }
            Message::Control { cc: 64, value } => {
                self.pedal.insert((ch, 64), value >= 64);
                self.let_go(ch);
            }
            Message::Control { cc: 66, value } => {
                let down = value >= 64;
                if down && !self.down(ch, 66) {
                    let held: Vec<i32> = self.held.iter().filter(|(_, v)| v.0 == ch).map(|(k, _)| *k).collect();
                    self.caught.extend(held);
                }
                self.pedal.insert((ch, 66), down);
                if !down {
                    let ours: Vec<i32> =
                        self.caught.iter().copied().filter(|k| self.held.get(k).or(self.sustained.get(k)).is_none_or(|v| v.0 == ch)).collect();
                    for k in ours {
                        self.caught.remove(&k);
                    }
                }
                self.let_go(ch);
            }
            Message::Control { cc: c @ (67 | 69), value } => {
                self.pedal.insert((ch, c), value >= 64);
            }
            Message::Control { cc: 120, .. } => {
                self.held.retain(|_, v| v.0 != ch);
                self.sustained.retain(|_, v| v.0 != ch);
                self.silenced.push(ch);
            }
            Message::Control { cc: 121, .. } => {
                for c in [64, 66, 67, 69] {
                    self.pedal.insert((ch, c), false);
                }
                self.let_go(ch);
                self.bend_reset.push(ch);
            }
            Message::Control { cc: 123, .. } => {
                let gone: Vec<i32> = self.held.iter().filter(|(_, v)| v.0 == ch).map(|(k, _)| *k).collect();
                for k in gone {
                    self.act(&Item { event: super::tests::off(ch, 0, -1), native: k, ..*it });
                }
            }
            _ => {}
        }
    }

    pub fn sounding(&self) -> usize {
        self.held.len() + self.sustained.len()
    }

    pub fn keys(&self) -> Vec<(u8, u8)> {
        self.held.values().chain(self.sustained.values()).copied().collect()
    }
}

/// Runs one block of `frames` through `q` into `fake` as one native call that succeeds.
pub(super) fn block(q: &mut Delivery, fake: &mut Fake, at: u64, frames: u32) {
    q.prepare(frames);
    let batch: Vec<Item> = q.batch(frames).to_vec();
    for it in &batch {
        fake.apply(at, it);
    }
    q.finish(true);
    q.carry();
    fake.calls += 1;
}

/// Feeds `script` (absolute frame, event) to `q` and renders `total` frames in blocks of `size`.
/// Returns the refusals as (frame, event).
pub(super) fn play(q: &mut Delivery, fake: &mut Fake, script: &[(u64, PluginEvent)], size: u32, total: u64) -> Vec<(u64, PluginEvent)> {
    let mut refused = Vec::new();
    let mut at = 0u64;
    let mut next = 0usize;
    while at < total {
        let n = u64::from(size).min(total - at);
        while let Some(&(f, ev)) = script.get(next) {
            if f >= at + n {
                break;
            }
            if !q.admit((f - at) as u32, &ev) {
                refused.push((f, ev));
            }
            next += 1;
        }
        block(q, fake, at, n as u32);
        at += n;
    }
    refused
}

#[test]
fn storage_is_accounted() {
    // ADR 0059, Decision 1: 2 x 704 + 1,024 + 48 = 2,480 native events; the record estimated 48
    // bytes per entry and per native event (about 149 KiB per hosted instance).
    assert_eq!(ENTRIES, 704);
    assert_eq!(SAFETY_SLOTS, 48);
    assert_eq!(ORDINARY, 1024);
    assert_eq!(NATIVE_EVENTS, 2480);
    assert_eq!(std::mem::size_of::<Option<Entry>>(), 24, "one ledger entry");
    assert_eq!(std::mem::size_of::<Item>(), 40, "one queued event of the native list");
    let bytes = ENTRIES * std::mem::size_of::<Option<Entry>>() + ENTRIES * 2 + NATIVE_EVENTS * std::mem::size_of::<Item>();
    assert_eq!(bytes, 16_896 + 1_408 + 99_200, "ledger, free list and native list per hosted instance");
}

#[test]
fn two_voices_of_one_key_both_end_at_a_full_allowance() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(0, &on(0, 60, -1)));
    assert!(q.admit(1, &on(0, 60, -1)));
    block(&mut q, &mut fake, 0, 16);
    assert_eq!(fake.held.len(), 2, "two voices of key 60");
    assert!(q.admit(0, &off(0, 60, -1)));
    for _ in 0..ORDINARY {
        assert!(q.admit(1, &cc(0, 1, 5)));
    }
    assert!(!q.admit(1, &cc(0, 1, 5)), "the ordinary allowance is full");
    assert!(q.admit(2, &off(0, 60, -1)), "an owned release is never refused");
    block(&mut q, &mut fake, 16, 16);
    assert_eq!(fake.sounding(), 0, "both voices end");
    assert_eq!(q.ledger().outstanding(), 0);
    q.reset();
    assert!(q.items().is_empty(), "Stop leaves nothing");
}

#[test]
fn a_skipped_call_keeps_its_events_at_offset_zero() {
    // #557: on at 500 in a skipped call, off at 10 in the next.
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(500, &on(0, 60, -1)));
    q.carry(); // skipped
    assert!(q.admit(10, &off(0, 60, -1)));
    block(&mut q, &mut fake, 512, 512);
    let got: Vec<(u64, bool)> = fake.log.iter().map(|(_, f, it)| (*f, matches!(it.event.message, Message::NoteOn { .. }))).collect();
    assert_eq!(got, [(512, true), (522, false)], "on at 0, off at 10");
    assert_eq!(fake.sounding(), 0);
    // Stale both: on at 500 and off at 10 of the next block, both skipped.
    let mut fake = Fake::default();
    assert!(q.admit(500, &on(0, 61, -1)));
    q.carry();
    assert!(q.admit(10, &off(0, 61, -1)));
    q.carry();
    block(&mut q, &mut fake, 0, 512);
    let got: Vec<(u64, bool)> = fake.log.iter().map(|(_, f, it)| (*f, matches!(it.event.message, Message::NoteOn { .. }))).collect();
    assert_eq!(got, [(0, true), (0, false)], "both at 0, the on first");
    assert_eq!(fake.sounding(), 0);
}

#[test]
fn a_release_never_goes_ahead_of_its_own_start() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(300, &on(0, 60, -1)));
    assert!(q.admit(10, &off(0, 60, -1)));
    assert!(q.admit(10, &cc(0, 1, 9)));
    block(&mut q, &mut fake, 0, 512);
    let order: Vec<_> = fake.log.iter().map(|(_, f, it)| (*f, it.event.message)).collect();
    assert!(matches!(order[0], (10, Message::Control { cc: 1, .. })), "time order kept: {order:?}");
    assert!(matches!(order[1], (300, Message::NoteOn { .. })));
    assert!(matches!(order[2], (300, Message::NoteOff { .. })));
    assert_eq!(fake.sounding(), 0);
}

#[test]
fn retained_events_fill_the_allowance_and_an_owned_release_still_gets_in() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(0, &on(0, 60, -1)));
    block(&mut q, &mut fake, 0, 64);
    let mut n = 0;
    for _ in 0..3 {
        while q.admit(5, &cc(0, 7, 1)) {
            n += 1;
        }
        q.carry(); // skipped
    }
    assert_eq!(n, ORDINARY, "skipped calls keep every admitted event");
    assert!(q.admit(9, &off(0, 60, -1)));
    block(&mut q, &mut fake, 64, 64);
    assert_eq!(fake.sounding(), 0, "the owned release was delivered");
}

#[test]
fn five_hundred_twelve_note_pairs_through_a_skipped_call_arrive_in_order() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    let mut sent = Vec::new();
    for round in 0..2u8 {
        for k in 0..128u8 {
            for ch in 0..2u8 {
                let key = k;
                let ch = ch + 2 * round;
                assert!(q.admit(0, &on(ch, key, -1)));
                assert!(q.admit(0, &off(ch, key, -1)));
                sent.push((ch, key, true));
                sent.push((ch, key, false));
            }
        }
        if round == 0 {
            q.carry(); // a skipped call between the two halves
        }
    }
    block(&mut q, &mut fake, 0, 512);
    let got: Vec<(u8, u8, bool)> = fake
        .log
        .iter()
        .map(|(_, _, it)| match it.event.message {
            Message::NoteOn { key, .. } => (it.event.channel, key, true),
            Message::NoteOff { key, .. } => (it.event.channel, key, false),
            m => panic!("unexpected {m:?}"),
        })
        .collect();
    assert_eq!(got.len(), 1024);
    assert_eq!(got, sent, "every note event delivered at the next call in arrival order");
    assert_eq!(fake.sounding(), 0);
}

#[test]
fn a_full_ledger_refuses_the_next_start_and_admits_every_release() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    for i in 0..ENTRIES {
        assert!(q.admit(0, &on((i % 16) as u8, (i / 16) as u8, -1)), "start {i}");
    }
    assert!(!q.admit(0, &on(15, 127, -1)), "the 705th start is refused at once");
    assert_eq!(q.refusals.starts, 1);
    assert!(!q.admit(0, &cc(3, 64, 127)), "a pedal press is a start too");
    for i in 0..ENTRIES {
        assert!(q.admit(1, &off((i % 16) as u8, (i / 16) as u8, -1)), "release {i}");
    }
    block(&mut q, &mut fake, 0, 4);
    assert_eq!(fake.sounding(), 0);
    assert_eq!(q.ledger().outstanding(), 0);
}

fn owned_voices_and_pedals(q: &mut Delivery, fake: &mut Fake) {
    for k in 0..8u8 {
        assert!(q.admit(0, &on(k % 2, 60 + k, -1)));
    }
    assert!(q.admit(0, &cc(0, 64, 127)));
    assert!(q.admit(0, &cc(1, 66, 127)));
    assert!(q.admit(0, &on(0, 70, 42)));
    block(q, fake, 0, 32);
    for _ in 0..ORDINARY {
        assert!(q.admit(3, &cc(0, 1, 1)));
    }
    // A start still queued is cancelled together with its release.
    assert!(q.admit(5, &on(0, 90, -1)));
}

#[test]
fn the_panic_releases_every_owned_voice_and_pedal_without_a_reset() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    owned_voices_and_pedals(&mut q, &mut fake);
    q.all_notes_off();
    assert_eq!(q.ordinary(), ORDINARY, "the panic resets nothing: queued controllers stay");
    block(&mut q, &mut fake, 32, 32);
    assert_eq!(fake.sounding(), 0, "every owned voice ended");
    assert!(fake.pedal.values().all(|d| !d), "every owned pedal released");
    assert!(!fake.log.iter().any(|(_, _, it)| matches!(it.event.message, Message::NoteOn { key: 90, .. })), "the queued start was cancelled");
    assert_eq!(q.ledger().outstanding(), 0);
    // The identified voice was released with its own native id and voice id.
    let start = fake.log.iter().find(|(_, _, it)| matches!(it.event.message, Message::NoteOn { key: 70, .. })).map(|x| x.2).unwrap();
    let release = fake.log.iter().find(|(_, _, it)| matches!(it.event.message, Message::NoteOff { key: 70, .. })).map(|x| x.2).unwrap();
    assert_eq!((release.native, release.event.note_id), (start.native, 42));
}

#[test]
fn stop_releases_then_keeps_only_what_survives_a_reset() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    owned_voices_and_pedals(&mut q, &mut fake);
    q.all_notes_off();
    q.reset();
    assert_eq!(q.ordinary(), 0, "Stop drops queued controllers");
    assert_eq!(q.owed(), 11, "owed releases stay for the next actual call");
    block(&mut q, &mut fake, 32, 32);
    assert_eq!(fake.sounding(), 0);
    assert!(fake.pedal.values().all(|d| !d));
    assert_eq!(q.ledger().outstanding(), 0, "freed when that call succeeded");
}

#[test]
fn identified_and_anonymous_releases_keep_their_pairing_in_both_orders() {
    for anonymous_first in [true, false] {
        let mut q = Delivery::new(false);
        let mut fake = Fake::default();
        assert!(q.admit(0, &on(0, 60, 42)));
        assert!(q.admit(1, &on(0, 60, -1)));
        block(&mut q, &mut fake, 0, 8);
        let native = |fake: &Fake, voice: i32| fake.log.iter().find(|(_, _, it)| it.event.note_id == voice).map(|x| x.2.native).unwrap();
        let (id42, anon) = (native(&fake, 42), native(&fake, -1));
        assert_ne!(id42, anon, "every entry has its own native id");
        let (first, second) = if anonymous_first { (off(0, 60, -1), off(0, 60, 42)) } else { (off(0, 60, 42), off(0, 60, -1)) };
        assert!(q.admit(0, &first));
        block(&mut q, &mut fake, 8, 8);
        let left: Vec<i32> = fake.held.keys().copied().collect();
        assert_eq!(left, [if anonymous_first { id42 } else { anon }], "the first off ended its own voice");
        assert!(q.admit(0, &second));
        block(&mut q, &mut fake, 16, 8);
        assert_eq!(fake.sounding(), 0);
        assert_eq!(q.ledger().outstanding(), 0);
    }
}

#[test]
fn an_unowned_release_is_accepted_as_nothing() {
    let mut q = Delivery::new(false);
    assert!(q.admit(0, &off(0, 60, -1)));
    assert!(q.admit(0, &off(0, 60, 7)));
    assert!(q.admit(0, &cc(0, 64, 0)), "a pedal up with no press open is an ordinary value");
    assert_eq!(q.items().len(), 1);
    assert_eq!(q.items()[0].class, Class::Ordinary);
}

#[test]
fn safety_controllers_release_their_channel_and_take_a_reserved_slot() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(0, &on(0, 60, -1)));
    assert!(q.admit(0, &on(0, 62, -1)));
    assert!(q.admit(0, &on(1, 64, -1)));
    assert!(q.admit(0, &cc(0, 64, 127)));
    assert!(q.admit(0, &cc(0, 66, 127)));
    block(&mut q, &mut fake, 0, 8);
    for _ in 0..ORDINARY {
        assert!(q.admit(1, &cc(0, 1, 1)));
    }
    assert!(q.admit(2, &cc(0, 121, 0)), "CC 121 at a full allowance");
    assert!(q.admit(3, &cc(0, 120, 0)), "CC 120 at a full allowance");
    // A second CC 121 of that channel before the call: its forwarding is refused and counted.
    assert!(q.admit(4, &cc(0, 121, 0)), "partial acceptance still reports true");
    assert_eq!(q.refusals.forwarding, 1);
    block(&mut q, &mut fake, 8, 8);
    assert_eq!(fake.bend_reset, [0], "CC 121 forwarded once");
    assert_eq!(fake.silenced, [0], "CC 120 forwarded");
    assert_eq!(fake.keys(), [(1, 64)], "every owned voice of channel 0 ended, channel 1 untouched");
    assert!(fake.pedal.iter().filter(|((ch, _), _)| *ch == 0).all(|(_, d)| !d), "every owned pedal of channel 0 released");
    assert_eq!(q.ledger().outstanding(), 1);
    // CC 123 with a pedal held is accepted as the releases it implies.
    assert!(q.admit(0, &cc(1, 64, 127)));
    assert!(q.admit(1, &cc(1, 123, 0)));
    block(&mut q, &mut fake, 16, 8);
    assert!(fake.held.is_empty());
    assert_eq!(q.ledger().outstanding(), 1, "only the pedal is still owned");
}

#[test]
fn pedal_presses_are_entries_and_value_changes_are_ordinary() {
    let mut q = Delivery::new(false);
    assert!(q.admit(0, &cc(0, 64, 127)));
    for v in 0..100u8 {
        assert!(q.admit(1, &cc(0, 64, 64 + v % 64)));
    }
    assert_eq!(q.ledger().outstanding(), 1, "values of the down class own nothing");
    assert!(q.admit(2, &cc(0, 64, 0)), "its release");
    assert!(q.admit(3, &cc(0, 64, 100)), "a second press is admitted");
    assert_eq!(q.ledger().outstanding(), 2);
}

#[test]
fn five_press_cycles_sustain_a_note_to_the_third_release_at_every_size() {
    // Pedal cycles at frames 0/10, 20/30, 40/50, 60/70, 80/90; note 60 on at 5, off at 45 (inside
    // the third cycle): the note sounds until the third release at 50.
    let mut script = Vec::new();
    for c in 0..5u64 {
        script.push((c * 20, cc(0, 64, 127)));
        script.push((c * 20 + 10, cc(0, 64, 0)));
    }
    script.push((5, on(0, 60, -1)));
    script.push((45, off(0, 60, -1)));
    script.sort_by_key(|(f, _)| *f);
    for size in [1u32, 7, 64, 512] {
        let mut q = Delivery::new(false);
        let mut fake = Fake::default();
        let refused = play(&mut q, &mut fake, &script, size, 128);
        assert!(refused.is_empty(), "every press admitted at size {size}");
        let presses = fake.log.iter().filter(|(_, _, it)| matches!(it.event.message, Message::Control { cc: 64, value: 127 })).count();
        assert_eq!(presses, 5);
        // The note's end: the pedal release that ends its sustain.
        let mut f2 = Fake::default();
        let mut ended = None;
        for (_, f, it) in &fake.log {
            f2.act(it);
            if ended.is_none() && f2.sounding() == 0 && *f > 5 {
                ended = Some(*f);
            }
        }
        assert_eq!(ended, Some(50), "sustained to the third release at size {size}");
    }
}

#[test]
fn press_cycles_through_three_skipped_calls_are_each_delivered() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    for _ in 0..3 {
        assert!(q.admit(0, &cc(0, 64, 127)));
        assert!(q.admit(0, &cc(0, 64, 0)));
        assert!(q.admit(1, &cc(0, 66, 127)));
        assert!(q.admit(1, &cc(0, 66, 0)));
        q.carry();
    }
    block(&mut q, &mut fake, 0, 16);
    let values: Vec<(u8, u8)> = fake
        .log
        .iter()
        .filter_map(|(_, _, it)| if let Message::Control { cc, value } = it.event.message { Some((cc, value)) } else { None })
        .collect();
    assert_eq!(values, [(64, 127), (64, 0), (66, 127), (66, 0)].repeat(3), "every cycle, in arrival order");
    assert_eq!(q.ledger().outstanding(), 0);
}

#[test]
fn a_failed_call_owes_every_handed_start_and_cancels_the_rest() {
    // A call hands [pedal-up, pedal-down, anonymous off A, on B] and fails, whatever the plug-in
    // applied of it: after Stop the next call ends everything and the ledger is empty.
    for k in 0..=4usize {
        let mut q = Delivery::new(false);
        let mut fake = Fake::default();
        assert!(q.admit(0, &cc(0, 64, 127)));
        assert!(q.admit(0, &on(0, 60, -1)));
        block(&mut q, &mut fake, 0, 4);
        assert!(q.admit(0, &cc(0, 64, 0)));
        assert!(q.admit(0, &cc(0, 64, 127)));
        assert!(q.admit(1, &off(0, 60, -1)));
        assert!(q.admit(1, &on(0, 61, -1)));
        // A later start that the failed call never handed.
        assert!(q.admit(9, &on(0, 62, -1)));
        q.prepare(16);
        let batch: Vec<Item> = q.batch(4).to_vec();
        assert_eq!(batch.len(), 4, "the sub-call hands four events; the start at 9 waits");
        for it in batch.iter().take(k) {
            fake.apply(4, it);
        }
        q.finish(false);
        q.carry();
        // The mix contains it until Stop: Stop's all_notes_off and reset.
        q.all_notes_off();
        q.reset();
        block(&mut q, &mut fake, 8, 4);
        assert_eq!(fake.sounding(), 0, "no voice after recovery (k = {k})");
        assert!(fake.pedal.values().all(|d| !d), "no pedal held (k = {k})");
        assert!(!fake.log.iter().any(|(_, _, it)| matches!(it.event.message, Message::NoteOn { key: 62, .. })), "never handed: cancelled");
        assert_eq!(q.ledger().outstanding(), 0, "the ledger is empty (k = {k})");
    }
}

#[test]
fn a_failed_recovery_call_is_owed_again() {
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    assert!(q.admit(0, &on(0, 60, -1)));
    block(&mut q, &mut fake, 0, 4);
    q.all_notes_off();
    q.reset();
    q.prepare(4);
    let _ = q.batch(4);
    q.finish(false); // the recovery call fails too
    q.carry();
    assert_eq!(q.owed(), 1, "the release is owed again");
    block(&mut q, &mut fake, 8, 4);
    assert_eq!(fake.sounding(), 0);
    assert_eq!(q.ledger().outstanding(), 0);
}

#[test]
fn capacity_two_hundred_releases_then_two_hundred_starts_refuse_nothing() {
    // ADR 0057's capacity example: 200 sounding notes; 200 offs over frames 0..200, then 200 ons
    // over frames 200..400.
    for size in [1u32, 7, 64, 512] {
        let mut q = Delivery::new(false);
        let mut fake = Fake::default();
        for k in 0..200u16 {
            assert!(q.admit(0, &on((k % 16) as u8, (k / 16) as u8, -1)));
        }
        block(&mut q, &mut fake, 0, 1);
        let mut script = Vec::new();
        for k in 0..200u16 {
            script.push((u64::from(k), off((k % 16) as u8, (k / 16) as u8, -1)));
        }
        for k in 0..200u16 {
            script.push((200 + u64::from(k), on((k % 16) as u8, 20 + (k / 16) as u8, -1)));
        }
        let refused = play(&mut q, &mut fake, &script, size, 512);
        assert!(refused.is_empty(), "size {size}");
        assert_eq!(fake.sounding(), 200);
    }
}

#[test]
fn capacity_ninety_six_resets_and_four_presses_refuse_nothing() {
    // 256 notes and 64 pedals held; 96 times CC 121 on channel 0, each followed by presses of its
    // four pedals, one event a frame (480 events): needs exactly 704 entries at block size 512.
    for size in [1u32, 7, 64, 512] {
        let mut q = Delivery::new(false);
        let mut fake = Fake::default();
        for k in 0..256u16 {
            assert!(q.admit(0, &on((k % 16) as u8, (k / 16) as u8, -1)));
        }
        for ch in 0..16u8 {
            for p in [64, 66, 67, 69] {
                assert!(q.admit(0, &cc(ch, p, 127)));
            }
        }
        block(&mut q, &mut fake, 0, 1);
        assert_eq!(q.ledger().outstanding(), 320);
        let mut script = Vec::new();
        let mut f = 0u64;
        for _ in 0..96 {
            script.push((f, cc(0, 121, 0)));
            f += 1;
            for p in [64, 66, 67, 69] {
                script.push((f, cc(0, p, 127)));
                f += 1;
            }
        }
        let mut peak = 0;
        let mut at = 0u64;
        let mut next = 0;
        while at < 512 {
            let n = u64::from(size).min(512 - at);
            while let Some(&(sf, ev)) = script.get(next) {
                if sf >= at + n {
                    break;
                }
                assert!(q.admit((sf - at) as u32, &ev), "nothing refused at size {size}");
                next += 1;
            }
            peak = peak.max(q.ledger().outstanding());
            block(&mut q, &mut fake, at, n as u32);
            at += n;
        }
        let expect = match size {
            1 => 320,
            7 => 325,
            64 => 371,
            _ => 704,
        };
        assert_eq!(peak, expect, "entries outstanding at size {size}");
    }
}

#[test]
fn the_ledger_matches_the_oldest_anonymous_entry() {
    let mut l = Ledger::new();
    let a = l.admit(&on(0, 60, -1));
    let b = l.admit(&on(0, 60, -1));
    let (super::ledger::Verdict::Start(a), super::ledger::Verdict::Start(b)) = (a, b) else { panic!() };
    assert_eq!(l.admit(&off(0, 60, -1)), super::ledger::Verdict::Release(a));
    assert_eq!(l.admit(&off(0, 60, -1)), super::ledger::Verdict::Release(b));
    assert_eq!(l.admit(&off(0, 60, -1)), super::ledger::Verdict::Nothing);
}

#[test]
fn native_ids_skip_an_outstanding_entry_after_the_counter_wraps() {
    let mut l = Ledger::new();
    let start = |l: &mut Ledger, key| match l.admit(&on(0, key, -1)) {
        super::ledger::Verdict::Start(i) => l.get(i).unwrap().native,
        v => panic!("{v:?}"),
    };
    l.seed_native(0);
    assert_eq!(start(&mut l, 60), 0, "a long-held note keeps id 0");
    l.seed_native(i32::MAX - 1);
    assert_eq!(start(&mut l, 61), i32::MAX - 1);
    assert_eq!(start(&mut l, 62), i32::MAX);
    assert_eq!(start(&mut l, 63), 1, "the counter wraps past the outstanding id 0");
    assert_eq!(start(&mut l, 64), 2);
}

#[test]
fn an_ordinary_event_reserves_its_whole_native_expansion() {
    let two = |m: &Message| if matches!(m, Message::Control14 { .. }) { 2 } else { 1 };
    let c14 = PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control14 { msb: 1, value: 9000 } };
    let mut q = Delivery::with_expansion(false, two);
    for _ in 0..ORDINARY / 2 {
        assert!(q.admit(0, &c14));
    }
    assert_eq!(q.ordinary(), ORDINARY, "each takes two native slots");
    assert!(!q.admit(0, &c14), "refused whole at a full allowance");
    assert!(!q.admit(0, &cc(0, 1, 1)));
    let mut q = Delivery::with_expansion(false, two);
    for _ in 0..ORDINARY - 1 {
        assert!(q.admit(0, &cc(0, 1, 1)));
    }
    assert!(!q.admit(0, &c14), "one slot left: neither half is admitted");
    assert!(q.admit(0, &cc(0, 1, 1)), "the last slot takes a one-event controller");
    let mut fake = Fake::default();
    block(&mut q, &mut fake, 0, 4);
    assert_eq!(q.ordinary(), 0, "delivery frees every slot");
}

//! ADR 0057, slice P3b (host note ownership, the delivery contract): decision 13's P3b evidence
//! with the fixture effects (the owning pass-through and the owning stacker) and the existing
//! Velocity effect at its defaults (it passes every event unchanged). The instrument is a recording
//! one that accepts every event unless a case refuses note-ons or releases. Expected values are
//! written from the record, not read back from the implementation. The stage buffer's final
//! acceptance (512 note-ons offered) is `overflow::an_accepted_note_on_is_never_evicted`.
//!
//! Covered here: releases never use the non-release limit (block sizes 1, 7, 64 and 512); the
//! reset barrier with both tables full; a full refused table's reset at a refusing instrument;
//! retried releases given directly and owed once; the two-ledger case, through the host's epoch
//! and through the devices; the sparse case; a refused note-on older than a delivered one of its
//! key; oldest-first `out` matching; the refused table cleared at a chain edit (the anonymous-key
//! replay, with an old-epoch release swallowed); a block with an unchanged chain (what a plan
//! adoption with the same chain gives the host) keeping every table.
use super::*;
use transit_dsp::midi_fx::{owning::Owning, stacker::Stacker};

const NO: i32 = PluginEvent::NO_ID;
const VELOCITY: (&str, &[(&str, f32)]) = ("midi_velocity", &[]);

/// An instrument that writes down every event it accepts, at its absolute frame.
#[derive(Default)]
struct Inst {
    /// The current block's first frame.
    frame: u64,
    heard: Vec<(u64, PluginEvent)>,
    refuse_ons: bool,
    refuse_releases: bool,
}

impl Plugin for Inst {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if (self.refuse_ons && matches!(ev.message, Message::NoteOn { .. })) || (self.refuse_releases && ev.is_release()) {
            return false;
        }
        self.heard.push((self.frame + u64::from(offset), *ev));
        true
    }
}

/// A host and its instrument on a render clock.
struct Run {
    host: Box<MidiFxHost>,
    inst: Inst,
    frame: u64,
}

impl Run {
    fn new() -> Self {
        Run { host: MidiFxHost::new(), inst: Inst::default(), frame: 0 }
    }

    /// One block of `frames` with `events` (offset, event), taken in the given order.
    fn block(&mut self, t: &Track, frames: usize, events: &[(u32, PluginEvent)]) {
        self.inst.frame = self.frame;
        self.host.begin(&mut self.inst, t, None, 0, &Transport::default(), 48_000.0, self.frame, frames);
        for (o, e) in events {
            assert!(self.host.take(&mut self.inst, *o, e), "taken: {e:?}");
        }
        self.host.flush(&mut self.inst);
        self.frame += frames as u64;
    }

    /// Blocks of `size` until frame `end`, each taking the `events` (absolute frame, event) due in
    /// it, in order.
    fn until(&mut self, t: &Track, size: usize, end: u64, events: &[(u64, PluginEvent)]) {
        while self.frame < end {
            let start = self.frame;
            let due: Vec<(u32, PluginEvent)> =
                events.iter().filter(|(f, _)| (start..start + size as u64).contains(f)).map(|(f, e)| ((f - start) as u32, *e)).collect();
            self.block(t, size, &due);
        }
    }

    fn count(&self, pick: fn(&Message) -> bool) -> usize {
        self.inst.heard.iter().filter(|(_, e)| pick(&e.message)).count()
    }

    fn ons(&self) -> usize {
        self.count(|m| matches!(m, Message::NoteOn { .. }))
    }

    fn offs(&self) -> usize {
        self.count(|m| matches!(m, Message::NoteOff { .. }))
    }

    fn used(table: &[Note]) -> usize {
        table.iter().filter(|n| n.used).count()
    }
}

fn note_on(channel: u8, voice: i32, key: u8) -> PluginEvent {
    PluginEvent { channel, note_id: voice, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE } }
}

fn note_off(channel: u8, voice: i32, key: u8) -> PluginEvent {
    PluginEvent { channel, note_id: voice, message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE } }
}

/// The voices sounding at the instrument after `heard`, in arrival order: an identified note-off
/// ends the voice with its id, an anonymous one the oldest anonymous voice of its channel and key;
/// CC 120 and 123 end their channel's. A note-off that ends nothing is a stray (an error).
fn sounding(heard: &[(u64, PluginEvent)]) -> Result<Vec<(u8, u8, i32)>, String> {
    let mut v: Vec<(u8, u8, i32)> = Vec::new();
    for (f, e) in heard {
        let ch = e.channel & 0x0F;
        match e.message {
            Message::NoteOn { key, .. } => v.push((ch, key, e.note_id)),
            Message::NoteOff { key, .. } => {
                let i =
                    if e.note_id == NO { v.iter().position(|x| *x == (ch, key, NO)) } else { v.iter().position(|x| x.0 == ch && x.2 == e.note_id) };
                match i {
                    Some(i) => {
                        v.remove(i);
                    }
                    None => return Err(format!("a stray note-off at {f}: {e:?}")),
                }
            }
            Message::Control { cc: 120 | 123, .. } => v.retain(|x| x.0 != ch),
            _ => {}
        }
    }
    Ok(v)
}

fn owning(host: &MidiFxHost, stage: usize) -> &Owning {
    match host.stages[stage].as_ref().and_then(|s| s.fx.as_ref()) {
        Some(Builtin::Owning(o)) => o,
        _ => panic!("stage {stage} is not the owning pass-through"),
    }
}

fn stacker(host: &MidiFxHost, stage: usize) -> &Stacker {
    match host.stages[stage].as_ref().and_then(|s| s.fx.as_ref()) {
        Some(Builtin::Stacker(s)) => s,
        _ => panic!("stage {stage} is not the owning stacker"),
    }
}

#[test]
fn releases_already_admitted_never_use_the_non_release_limit() {
    // 200 sounding notes, then 200 releases followed by 200 note-ons, one event a frame over 400
    // frames (512 to 911, one block at size 512): all 200 note-ons admitted at every size.
    let t = track(&[VELOCITY]);
    let key = |i: i32| ((i / 100) as u8, 20 + (i % 100) as u8);
    let mut events = Vec::new();
    for i in 0..200 {
        events.push((i as u64, note_on(key(i).0, i, key(i).1)));
    }
    for i in 0..200 {
        events.push((512 + i as u64, note_off(key(i).0, i, key(i).1)));
    }
    for i in 0..200 {
        events.push((712 + i as u64, note_on(key(i).0, 200 + i, key(i).1)));
    }
    for size in [1, 7, 64, 512] {
        let mut r = Run::new();
        r.until(&t, size, 1024, &events);
        assert_eq!((r.ons(), r.offs()), (400, 200), "size {size}: every note-on admitted");
        assert_eq!((r.host.counts.overflow, r.host.counts.refused_ons), (0, 0), "size {size}");
    }
}

#[test]
fn the_reset_barrier_drops_the_note_ons_after_its_trigger_and_nothing_sounds_afterwards() {
    // `out` and the refused table full, then a block whose batch holds two note-ons: the first
    // triggers the reset (it finds the refused table full), the second is dropped even though a
    // release between them freed room in `out`; the release still ends its note; after the
    // block's last event the reset ends every delivered note and gives the safety controllers.
    let t = track(&[VELOCITY]);
    let at = |i: i32| ((i / 128) as u8, (i % 128) as u8);
    let mut r = Run::new();
    r.block(&t, 256, &(0..256).map(|i| (0, note_on(at(i).0, i, at(i).1))).collect::<Vec<_>>());
    r.block(&t, 256, &(256..512).map(|i| (0, note_on(at(i).0, i, at(i).1))).collect::<Vec<_>>());
    assert_eq!((r.ons(), Run::used(&r.host.out), Run::used(&r.host.refusals)), (256, 256, 256), "both tables full");
    let before = r.inst.heard.len();
    let epoch = r.host.epoch;
    r.block(&t, 256, &[(10, note_on(at(512).0, 512, at(512).1)), (20, note_off(0, 0, 0)), (30, note_on(at(513).0, 513, at(513).1))]);
    let heard = &r.inst.heard[before..];
    assert!(heard.iter().all(|(_, e)| !matches!(e.message, Message::NoteOn { .. })), "no note-on after the trigger: {heard:?}");
    assert_eq!(heard[0], (512 + 20, note_off(0, 0, 0)), "the release ends its note");
    let offs: Vec<_> = heard[1..256].iter().filter(|(f, e)| *f == 767 && matches!(e.message, Message::NoteOff { .. })).collect();
    assert_eq!(offs.len(), 255, "the reset ends every other delivered note after the block's events");
    let safety: Vec<_> = heard[256..].iter().filter(|(f, e)| *f == 767 && matches!(e.message, Message::Control { value: 0, .. })).collect();
    assert_eq!((safety.len(), heard.len()), (16 * 7, 256 + 16 * 7), "then the safety controllers, and nothing else");
    let c = r.host.counts;
    assert_eq!((c.safety_resets, c.dropped_ons, c.refused_ons), (1, 2, 256 + 1));
    assert_eq!((Run::used(&r.host.out), Run::used(&r.host.refusals), r.host.epoch), (0, 0, epoch.wrapping_add(1)));
    // Afterwards every input is released: the old epoch swallows the given ones (513 took the
    // given entry 0's release freed), and the untracked ones (256 to 512: the given table was
    // full) match nothing and are consumed.
    let n = r.inst.heard.len();
    r.block(&t, 256, &(1..256).map(|i| (0, note_off(at(i).0, i, at(i).1))).collect::<Vec<_>>());
    r.block(&t, 256, &(256..514).map(|i| (0, note_off(at(i).0, i, at(i).1))).collect::<Vec<_>>());
    assert_eq!(r.inst.heard.len(), n, "nothing reaches the instrument");
    assert_eq!(sounding(&r.inst.heard), Ok(vec![]), "nothing sounds");
    assert_eq!(r.host.counts.consumed_releases, 257);
}

#[test]
fn a_full_refused_table_makes_a_safety_reset() {
    // The instrument refuses every note-on: 256 are recorded, the 257th finds the table full.
    let t = track(&[VELOCITY]);
    let mut r = Run::new();
    r.block(&t, 512, &[]);
    r.inst.refuse_ons = true;
    let epoch = r.host.epoch;
    r.block(&t, 512, &(0..257).map(|i| (i as u32, note_on(0, i, (i % 128) as u8))).collect::<Vec<_>>());
    let c = r.host.counts;
    assert_eq!((c.refused, c.safety_resets, c.dropped_ons), (257, 1, 1));
    assert_eq!((Run::used(&r.host.refusals), r.host.epoch), (0, epoch.wrapping_add(1)), "cleared, a new epoch");
    assert_eq!(r.inst.heard.len(), 16 * 7, "the safety controllers");
    assert!(r.inst.heard.iter().all(|(f, e)| *f == 1023 && matches!(e.message, Message::Control { value: 0, .. })));
}

#[test]
fn a_refused_release_is_given_again_directly_and_owed_once() {
    let t = track(&[VELOCITY]);
    let mut bypassed = t.clone();
    bypassed.midi_effects[0].bypass = true;
    // A reset note-off (an edit's) the instrument refuses once is given at the next block, at
    // offset 0, and never consumed (its `out` entry was freed when it was refused).
    let mut r = Run::new();
    r.block(&t, 256, &[(10, note_on(0, 1, 60))]);
    r.inst.refuse_releases = true;
    r.block(&bypassed, 256, &[]);
    assert_eq!((r.host.retry_len, r.offs()), (1, 0), "refused and held");
    r.inst.refuse_releases = false;
    r.block(&bypassed, 256, &[]);
    assert_eq!(r.inst.heard.last(), Some(&(512, note_off(0, 1, 60))), "given at the next block's offset 0");
    assert_eq!((r.host.counts.consumed_releases, r.host.retry_len), (0, 0));
    // A chain note-off the instrument refuses frees its `out` entry as it enters the retry buffer:
    // a later anonymous note-off of its key claims the next entry, and a reset gives no second
    // note-off for it.
    let mut r = Run::new();
    r.block(&t, 256, &[(10, note_on(0, NO, 60)), (20, note_on(0, NO, 60))]);
    r.inst.refuse_releases = true;
    r.block(&t, 256, &[(5, note_off(0, NO, 60))]);
    assert_eq!((Run::used(&r.host.out), r.host.retry_len), (1, 1), "owed once, from the retry buffer");
    r.inst.refuse_releases = false;
    r.block(&t, 256, &[(10, note_off(0, NO, 60))]);
    assert_eq!(&r.inst.heard[2..], [(512, note_off(0, NO, 60)), (522, note_off(0, NO, 60))]);
    assert_eq!(Run::used(&r.host.out), 0, "the later note-off claimed the next entry");
    r.host.panic(&mut r.inst);
    assert_eq!(r.offs(), 2, "the reset gives no second note-off");
    assert_eq!(sounding(&r.inst.heard), Ok(vec![]));
}

/// Decision 13's two-ledger inputs: roots 12o + 0, 1, 2 for octaves o = 0 to 9, and 120, on
/// channels 0 to 5 in turn, the first 171, voice ids from 1000.
fn two_ledger_inputs() -> Vec<(u8, u8, i32)> {
    let mut v = Vec::new();
    for ch in 0..6u8 {
        for root in (0..10u8).flat_map(|o| (0..3).map(move |r| 12 * o + r)).chain([120]) {
            v.push((ch, root, 1000 + v.len() as i32));
        }
    }
    v.truncate(171);
    v
}

fn two_ledger_case(through_devices: bool) {
    let t = track(&[("midi_test_owning", &[]), ("midi_test_stacker", &[("shift_1", 4.0), ("shift_2", 7.0)])]);
    let inputs = two_ledger_inputs();
    let strikes: Vec<(u64, PluginEvent)> = inputs.iter().enumerate().map(|(i, (c, k, v))| (600 * i as u64, note_on(*c, *v, *k))).collect();
    let last = 600 * 170;
    let block_of_last = last / 512 * 512;
    let mut r = Run::new();
    r.until(&t, 512, block_of_last, &strikes);
    assert_eq!((r.ons(), Run::used(&r.host.out), Run::used(&r.host.refusals)), (256, 256, 254), "before the 171st input's block");
    r.until(&t, 512, block_of_last + 512, &strikes);
    let ctx = format!("through the devices: {through_devices}");
    assert_eq!((owning(&r.host, 0).peak(), stacker(&r.host, 1).peak()), (171, 171), "{ctx}: both ledgers held 171 entries");
    assert_eq!(stacker(&r.host, 1).written_ons, 513, "{ctx}: the stacker wrote 513 note-ons");
    let c = r.host.counts;
    assert_eq!((c.safety_resets, c.dropped_ons, c.released_on_overflow), (1, 1, 256), "{ctx}: the 513th triggered the barrier");
    assert_eq!((owning(&r.host, 0).open(), stacker(&r.host, 1).open(), Run::used(&r.host.refusals)), (0, 0, 0), "{ctx}: cleared");
    // Every input released 600 frames apart, then a new chord struck and released, then a pedal-up
    // that matches nothing.
    if through_devices {
        r.host.given = [NO_NOTE; MIDI_FX_NOTES];
        r.host.given_len = 0;
    }
    let base = r.frame + 88;
    let mut later: Vec<(u64, PluginEvent)> = inputs.iter().enumerate().map(|(i, (c, k, v))| (base + 600 * i as u64, note_off(*c, *v, *k))).collect();
    let chord = base + 600 * 171;
    later.push((chord, note_on(7, 5000, 60)));
    later.push((chord + 600, note_off(7, 5000, 60)));
    let pedal = PluginEvent { channel: 7, note_id: NO, message: Message::Control { cc: 64, value: 0 } };
    later.push((chord + 1200, pedal));
    r.until(&t, 512, chord + 2048, &later);
    assert_eq!((r.ons(), r.offs()), (259, 259), "{ctx}");
    assert_eq!(sounding(&r.inst.heard), Ok(vec![]), "{ctx}: no stray note-off, nothing sounds");
    assert!(r.inst.heard.iter().any(|(f, e)| *f == chord + 1200 && *e == pedal), "{ctx}: the pedal-up is delivered, not consumed");
    let (own, stack) = (owning(&r.host, 0), stacker(&r.host, 1));
    assert_eq!((own.open(), stack.open(), Run::used(&r.host.out), Run::used(&r.host.refusals)), (0, 0, 0, 0), "{ctx}: all empty");
    if through_devices {
        assert_eq!(own.unmatched_releases, 171, "{ctx}: each old release consumed by an empty ledger");
    } else {
        assert_eq!((own.unmatched_releases, r.host.given_len), (0, 0), "{ctx}: the epoch swallowed the old releases");
    }
    assert_eq!(r.host.counts.consumed_releases, 0, "{ctx}: no old release reached the delivery");
}

#[test]
fn two_owning_ledgers_in_series_fill_both_host_tables_and_the_barrier_leaves_nothing_sounding() {
    two_ledger_case(false);
    two_ledger_case(true);
}

#[test]
fn the_sparse_case_gives_one_note_off_for_each_note_on() {
    // The stacker with +12 only, 129 held inputs 600 frames apart on keys whose shifted partners
    // are no other input's key: 258 output notes; `out` takes 256 and records the last two as
    // refused; every release then gives the instrument 256 note-offs (the old host gave 258).
    let t = track(&[("midi_test_stacker", &[("shift_1", 12.0)])]);
    let keys: Vec<(u8, u8)> =
        (0..3u8).flat_map(|ch| [0u8, 24, 48, 72, 96].into_iter().flat_map(move |b| (b..b + 12).map(move |k| (ch, k)))).take(129).collect();
    let mut events: Vec<(u64, PluginEvent)> = keys.iter().enumerate().map(|(i, (c, k))| (600 * i as u64, note_on(*c, 2000 + i as i32, *k))).collect();
    let base = 600 * 129;
    events.extend(keys.iter().enumerate().map(|(i, (c, k))| (base + 600 * i as u64, note_off(*c, 2000 + i as i32, *k))));
    let mut r = Run::new();
    r.until(&t, 512, base + 600 * 129 + 512, &events);
    assert_eq!((r.ons(), r.offs()), (256, 256));
    assert_eq!(sounding(&r.inst.heard), Ok(vec![]), "no stray note-off");
    assert_eq!((r.host.counts.consumed_releases, r.host.counts.safety_resets), (2, 0));
}

#[test]
fn a_refused_note_on_is_older_than_a_delivered_strike_of_its_key() {
    // A note-on refused at delivery, then an admitted strike of the same key: the first anonymous
    // note-off is consumed (the refused note is older), the second ends the delivered note.
    let t = track(&[VELOCITY]);
    let mut r = Run::new();
    r.inst.refuse_ons = true;
    r.block(&t, 256, &[(10, note_on(0, NO, 60))]);
    r.inst.refuse_ons = false;
    r.block(&t, 256, &[(10, note_on(0, NO, 60))]);
    r.block(&t, 256, &[(10, note_off(0, NO, 60))]);
    assert_eq!(r.inst.heard, [(266, note_on(0, NO, 60))], "the first note-off consumed");
    r.block(&t, 256, &[(10, note_off(0, NO, 60))]);
    assert_eq!(r.inst.heard[1..], [(778, note_off(0, NO, 60))], "the second ends the delivered note");
    assert_eq!(r.host.counts.consumed_releases, 1);
}

#[test]
fn a_note_off_closes_the_oldest_entry_by_sequence_never_the_first_used_slot() {
    let t = track(&[VELOCITY]);
    let mut r = Run::new();
    // Slot 0: 61, slot 1: 60 (older); 61 ends, the newer 60 takes slot 0.
    r.block(&t, 256, &[(1, note_on(0, NO, 61)), (2, note_on(0, NO, 60))]);
    r.block(&t, 256, &[(1, note_off(0, NO, 61)), (2, note_on(0, NO, 60))]);
    let (older, newer) = (r.host.out[1].seq, r.host.out[0].seq);
    assert!(r.host.out[0].used && r.host.out[1].used && older < newer);
    r.block(&t, 256, &[(1, note_off(0, NO, 60))]);
    assert!(!r.host.out[1].used && r.host.out[0].used && r.host.out[0].seq == newer, "the anonymous note-off closed the older entry");
    // An identified note-off closes its own entry only: an unknown id closes nothing (consumed),
    // and an older anonymous entry of the key is not taken for it.
    let n = r.inst.heard.len();
    r.block(&t, 256, &[(1, note_on(0, NO, 62)), (2, note_on(0, 7, 62)), (3, note_off(0, 8, 62)), (4, note_off(0, 7, 62))]);
    assert_eq!(r.inst.heard[n..], [(769, note_on(0, NO, 62)), (770, note_on(0, 7, 62)), (772, note_off(0, 7, 62))]);
    assert_eq!(r.host.counts.consumed_releases, 1);
    let left: Vec<(u8, i32)> = r.host.out.iter().filter(|n| n.used).map(|n| (n.key, n.voice)).collect();
    assert_eq!(left, [(60, NO), (62, NO)]);
}

/// 128 inputs on channels 1 to 3 whose stacked partners (+12) are no other input's key: with the
/// stacker they fill `out` (256 notes).
fn fill_out() -> Vec<(u32, PluginEvent)> {
    (1..4u8)
        .flat_map(|ch| [0u8, 24, 48, 72, 96].into_iter().flat_map(move |b| (b..b + 12).map(move |k| (ch, k))))
        .take(128)
        .enumerate()
        .map(|(i, (c, k))| (i as u32, note_on(c, 100 + i as i32, k)))
        .collect()
}

#[test]
fn a_chain_edit_clears_the_refused_table() {
    // The anonymous-key replay, the instrument accepting every event offered: a full `out` table
    // records the stacker's anonymous 72 (from input 60) as refused; an edit resets the stacker;
    // the late release of the old-epoch input is swallowed by its tombstone; the rebuilt chain
    // delivers a new anonymous 72 and its note-off, which ends it: nothing sounds. (With the table
    // kept across the edit, the new note-off would be consumed against the stale refusal.)
    let t = track(&[("midi_test_stacker", &[("shift_1", 12.0)])]);
    let edited = track(&[VELOCITY, ("midi_test_stacker", &[("shift_1", 12.0)])]);
    let mut r = Run::new();
    r.block(&t, 512, &fill_out());
    r.block(&t, 512, &[(5, note_on(0, 500, 60))]);
    assert_eq!((r.ons(), Run::used(&r.host.refusals)), (256, 2), "60 (id 500) and the anonymous 72 refused");
    r.block(&edited, 512, &[]);
    assert_eq!(r.offs(), 256, "the edit ended the delivered notes");
    let n = r.inst.heard.len();
    r.block(&edited, 512, &[(5, note_off(0, 500, 60))]);
    assert_eq!((r.inst.heard.len(), r.host.given_len), (n, 128), "the old-epoch release swallowed, its tombstone freed");
    r.block(&edited, 512, &[(5, note_on(0, 501, 60)), (50, note_off(0, 501, 60))]);
    assert_eq!(
        r.inst.heard[n..],
        [(2053, note_on(0, 501, 60)), (2053, note_on(0, NO, 72)), (2098, note_off(0, 501, 60)), (2098, note_off(0, NO, 72))]
    );
    assert_eq!(sounding(&r.inst.heard), Ok(vec![]), "nothing sounds");
    assert_eq!((r.host.counts.consumed_releases, Run::used(&r.host.refusals)), (0, 0));
}

#[test]
fn a_block_with_an_unchanged_chain_keeps_every_table() {
    // A plan adopted with the same chain reaches the host as a block whose track is a new value
    // with the same effects (here a renamed, recolored copy): no edit, so the delivery tables,
    // the given notes, the sequence and the epoch are kept, and the refused notes' note-offs are
    // still consumed afterwards.
    let t = track(&[("midi_test_stacker", &[("shift_1", 12.0)])]);
    let mut r = Run::new();
    r.block(&t, 512, &fill_out());
    r.block(&t, 512, &[(5, note_on(0, 500, 60))]);
    let tables = |h: &MidiFxHost| format!("{:?} {:?} {:?} {} {} {}", h.out, h.refusals, h.given, h.given_len, h.seq, h.epoch);
    let before = tables(&r.host);
    let mut adopted = t.clone();
    adopted.name = "Keys, renamed".into();
    adopted.color = [1, 2, 3];
    let n = r.inst.heard.len();
    r.block(&adopted, 512, &[]);
    assert_eq!(tables(&r.host), before, "every table kept");
    assert_eq!((r.inst.heard.len(), r.host.counts.released_on_edit), (n, 0), "nothing ended");
    r.block(&adopted, 512, &[(5, note_off(0, 500, 60))]);
    assert_eq!(r.inst.heard.len(), n, "60 and the anonymous 72 consumed against their refusals");
    assert_eq!((r.host.counts.consumed_releases, Run::used(&r.host.refusals)), (2, 0));
}

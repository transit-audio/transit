//! ADR 0059 R1 through the VST3 adapter's native translation and processing boundary (review round
//! 2): the fixture's events synth records every note event and mapped controller point it receives,
//! by call, apart from its audio output (its record channel), so partitions down to one frame can be
//! inspected. The voice and pedal state is evaluated on those native records with the plug-in
//! reading each frame's points before its events and after them; both must equal the state of the
//! events in arrival order. The fixture maps CC 64 on channel c to parameter 100 + c, and CC 66, 67,
//! 69, 120, 121, 123 to 200, 300, 400, 500, 600, 700 + c.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};

mod common;
use common::fixture_dir;

fn synth(block: usize) -> Box<dyn Plugin> {
    let found = transit_vst3_host::scan_paths(&[fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Events").unwrap().id;
    let mut p = transit_vst3_host::create(&id).unwrap();
    p.prepare(48_000.0, block, 2);
    assert!(p.load_state(b"REC"), "record mode");
    let _ = p.save_state(); // empty the log of the creation's calls
    p
}

/// One native record at its absolute frame: 1 on, 2 off (a = key, b = note id); 4 point
/// (a = parameter id, b = value x 1e6).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rec {
    frame: u64,
    code: u32,
    a: u32,
    b: i64,
}

/// The records logged since the last read, framed by the calls they arrived in (a call of zero
/// frames is never made: a skipped block leaves no marker).
fn log(p: &mut Box<dyn Plugin>, base: &mut u64) -> Vec<Rec> {
    let state = p.save_state().unwrap();
    let at = state.windows(4).position(|w| w == b"REC1").expect("the record channel");
    let n = u32::from_le_bytes(state[at + 4..at + 8].try_into().unwrap()) as usize;
    let mut out = Vec::new();
    let mut call = *base;
    for k in 0..n {
        let o = at + 8 + 16 * k;
        let f = |i: usize| f32::from_le_bytes(state[o + 4 * i..o + 4 * i + 4].try_into().unwrap());
        let (code, x, a, b) = (f(0) as u32, f(1) as u64, f(2) as u32, f(3) as i64);
        if code == 9 {
            call = *base;
            *base += x;
        } else {
            out.push(Rec { frame: call + x, code, a: a % 1000, b });
        }
    }
    out
}

fn ev(channel: u8, message: Message) -> PluginEvent {
    PluginEvent { channel, note_id: PluginEvent::NO_ID, message }
}
fn on(key: u8) -> PluginEvent {
    ev(0, Message::NoteOn { key, velocity: 100, id: NoteId::NONE })
}
fn off(key: u8) -> PluginEvent {
    ev(0, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
}
fn cc(n: u8, v: u8) -> PluginEvent {
    ev(0, Message::Control { cc: n, value: v })
}

/// Plays `script` (absolute frame, event) through `p` in blocks of `size` for `total` frames.
/// Returns the native records and the number of events `event` refused.
fn play(p: &mut Box<dyn Plugin>, script: &[(u64, PluginEvent)], size: usize, total: u64) -> (Vec<Rec>, usize) {
    let mut base = 0u64;
    let mut recs = Vec::new();
    let mut refused = 0;
    let mut at = 0u64;
    let mut next = 0;
    let mut io = vec![vec![0.0f32; size], vec![0.0f32; size]];
    while at < total {
        let n = (size as u64).min(total - at);
        while let Some(&(f, e)) = script.get(next) {
            if f >= at + n {
                break;
            }
            if !p.event((f - at) as u32, &e) {
                refused += 1;
            }
            next += 1;
        }
        p.process(&mut io, n as usize);
        recs.extend(log(p, &mut base));
        at += n;
    }
    assert_eq!(transit_vst3_host::UNTRANSLATED.load(std::sync::atomic::Ordering::Relaxed), 0, "nothing dropped in translation");
    (recs, refused)
}

/// The plug-in's voices: held keys and voices still sounding under a pedal (by note id), the
/// sostenuto catch, and every pedal and safety controller per channel's mapped parameter.
#[derive(Default, Debug, Clone, PartialEq)]
struct Voices {
    held: BTreeSet<i64>,
    sustained: BTreeSet<i64>,
    caught: BTreeSet<i64>,
    pedal: BTreeMap<u32, bool>,
}

impl Voices {
    fn down(&self, cc: u32) -> bool {
        self.pedal.get(&cc).copied().unwrap_or(false)
    }
    fn let_go(&mut self) {
        let (sus, sost) = (self.down(64), self.down(66));
        let caught = self.caught.clone();
        self.sustained.retain(|k| sus || (sost && caught.contains(k)));
    }
    fn release(&mut self, id: i64) {
        if self.held.remove(&id) && (self.down(64) || (self.down(66) && self.caught.contains(&id))) {
            self.sustained.insert(id);
        }
    }
    fn apply(&mut self, r: &Rec) {
        match (r.code, r.a / 100) {
            (1, _) => {
                self.held.insert(r.b);
            }
            (2, _) => self.release(r.b),
            (4, 1..=4) => {
                let cc = [0, 64, 66, 67, 69][(r.a / 100) as usize];
                let down = r.b >= 500_000;
                if cc == 66 && down && !self.down(66) {
                    self.caught = self.held.clone();
                }
                self.pedal.insert(cc, down);
                if cc == 66 && !down {
                    self.caught.clear();
                }
                self.let_go();
            }
            (4, 5) => {
                self.held.clear();
                self.sustained.clear();
            }
            (4, 6) => {
                for cc in [64, 66, 67, 69] {
                    self.pedal.insert(cc, false);
                }
                self.caught.clear();
                self.let_go();
            }
            (4, 7) => {
                for id in self.held.clone() {
                    self.release(id);
                }
            }
            _ => {}
        }
    }
    fn sounding(&self) -> usize {
        self.held.len() + self.sustained.len()
    }
}

/// The state after the records, reading each frame's points before (or after) its note events.
fn read(recs: &[Rec], points_first: bool) -> Voices {
    let mut frames: BTreeMap<u64, Vec<Rec>> = BTreeMap::new();
    for r in recs {
        frames.entry(r.frame).or_default().push(*r);
    }
    let mut v = Voices::default();
    for rs in frames.values() {
        let (points, notes): (Vec<Rec>, Vec<Rec>) = rs.iter().partition(|r| r.code == 4);
        let (a, b) = if points_first { (points, notes) } else { (notes, points) };
        for r in a.iter().chain(b.iter()) {
            v.apply(r);
        }
    }
    v
}

/// Both reading orders give `want`.
fn both(recs: &[Rec], want: &Voices, what: &str) {
    for points_first in [true, false] {
        let got = read(recs, points_first);
        assert_eq!((got.held.len(), got.sustained.len()), (want.held.len(), want.sustained.len()), "{what}, points first: {points_first}");
        assert_eq!(got.pedal.values().filter(|d| **d).count(), want.pedal.values().filter(|d| **d).count(), "{what} pedals");
    }
}

/// The frame a note stops sounding in both reading orders (None: still sounding).
fn end_of(recs: &[Rec], key_id: i64) -> [Option<u64>; 2] {
    [true, false].map(|points_first| {
        let mut frames: BTreeMap<u64, Vec<Rec>> = BTreeMap::new();
        for r in recs {
            frames.entry(r.frame).or_default().push(*r);
        }
        let mut v = Voices::default();
        let mut started = false;
        for (f, rs) in frames {
            let (points, notes): (Vec<Rec>, Vec<Rec>) = rs.iter().partition(|r| r.code == 4);
            let (a, b) = if points_first { (points, notes) } else { (notes, points) };
            for r in a.iter().chain(b.iter()) {
                v.apply(r);
            }
            let sounding = v.held.contains(&key_id) || v.sustained.contains(&key_id);
            started |= sounding;
            if started && !sounding {
                return Some(f);
            }
        }
        None
    })
}

const SIZES: [usize; 4] = [1, 7, 64, 512];

#[test]
fn five_press_cycles_sustain_a_note_to_the_third_release_at_every_size_and_through_skipped_calls() {
    let mut script = Vec::new();
    for c in 0..5u64 {
        script.push((c * 20, cc(64, 127)));
        script.push((c * 20 + 10, cc(64, 0)));
    }
    script.push((5, on(60)));
    script.push((45, off(60)));
    script.sort_by_key(|(f, _)| *f);
    for size in SIZES {
        let mut p = synth(size);
        let (recs, refused) = play(&mut p, &script, size, 128);
        assert_eq!(refused, 0);
        let presses = recs.iter().filter(|r| r.code == 4 && r.a == 100 && r.b == 1_000_000).count();
        assert_eq!(presses, 5, "every press reached the plug-in at size {size}");
        let id = recs.iter().find(|r| r.code == 1).unwrap().b;
        assert_eq!(end_of(&recs, id), [Some(50), Some(50)], "sustained to the third release at size {size}");
    }
    // Through three skipped calls: the cycles of the skipped blocks arrive at offset 0, each one.
    let mut p = synth(64);
    let mut base = 0;
    let mut io = vec![vec![0.0f32; 64]; 2];
    for _ in 0..3 {
        for e in [cc(64, 127), cc(64, 0), cc(66, 127), cc(66, 0)] {
            assert!(p.event(0, &e));
        }
        p.process(&mut io, 0);
    }
    p.process(&mut io, 64);
    let recs = log(&mut p, &mut base);
    let edges: Vec<(u32, i64)> = recs.iter().filter(|r| r.code == 4).map(|r| (r.a, r.b)).collect();
    assert_eq!(edges.len(), 12, "every edge of the three skipped cycles: {edges:?}");
    // The fixture lists its points per parameter queue; by frame they are the arrival order.
    let mut by_frame: Vec<(u64, u32, i64)> = recs.iter().filter(|r| r.code == 4).map(|r| (r.frame, r.a, r.b)).collect();
    by_frame.sort_unstable();
    let want: Vec<(u64, u32, i64)> = (0..12u64).map(|f| (f, [100, 100, 200, 200][(f % 4) as usize], [1_000_000, 0][(f % 2) as usize])).collect();
    assert_eq!(by_frame, want, "each edge on its own frame (3b), in arrival order");
    both(&recs, &Voices::default(), "after the cycles");
}

#[test]
fn every_pair_of_controller_borne_voice_events_reaches_its_own_parameter_on_successive_frames() {
    let kinds: [(u8, u8, u32); 7] = [(64, 127, 100), (66, 127, 200), (67, 127, 300), (69, 127, 400), (120, 0, 500), (121, 0, 600), (123, 0, 700)];
    for a in kinds {
        for b in kinds {
            if a.0 == b.0 {
                continue;
            }
            let mut p = synth(64);
            let (recs, refused) =
                play(&mut p, &[(3, ev(2, Message::Control { cc: a.0, value: a.1 })), (3, ev(2, Message::Control { cc: b.0, value: b.1 }))], 64, 64);
            assert_eq!(refused, 0);
            let mut points: Vec<(u64, u32)> = recs.iter().filter(|r| r.code == 4 && r.a >= 100).map(|r| (r.frame, r.a)).collect();
            points.sort_unstable(); // the fixture lists points per parameter queue
            // CC 121 after a press also releases that pedal (an implied release, on its own frame).
            let mapped: Vec<u32> = points.iter().map(|x| x.1).collect();
            let frames: Vec<u64> = points.iter().map(|x| x.0).collect();
            assert_eq!(frames, (3..3 + points.len() as u64).collect::<Vec<_>>(), "{a:?} {b:?}: {points:?}");
            let ia = mapped.iter().position(|m| *m == a.2 + 2).expect("first controller's parameter (channel 2)");
            let ib = mapped.iter().rposition(|m| *m == b.2 + 2).expect("second controller's parameter");
            assert!(ia < ib, "arrival order: {points:?}");
        }
    }
}

#[test]
fn capacity_cases_refuse_nothing_at_every_size() {
    for size in SIZES {
        // 200 sounding notes; 200 offs over frames 0..200, then 200 ons over 200..400.
        let mut p = synth(size);
        let mut script = Vec::new();
        for k in 0..200u16 {
            script.push((0, ev((k % 16) as u8, Message::NoteOn { key: (k / 16) as u8, velocity: 90, id: NoteId::NONE })));
        }
        for k in 0..200u16 {
            script.push((1 + u64::from(k), ev((k % 16) as u8, Message::NoteOff { key: (k / 16) as u8, velocity: 0, id: NoteId::NONE })));
        }
        for k in 0..200u16 {
            script.push((201 + u64::from(k), ev((k % 16) as u8, Message::NoteOn { key: 20 + (k / 16) as u8, velocity: 90, id: NoteId::NONE })));
        }
        let (recs, refused) = play(&mut p, &script, size, 512);
        assert_eq!(refused, 0, "200/200 at size {size}");
        let v = read(&recs, true);
        assert_eq!(v.sounding(), 200);
        // 256 notes and 64 pedals held, then 96 x CC 121 on channel 0 with presses of its four pedals.
        let mut p = synth(size);
        let mut script = Vec::new();
        for k in 0..256u16 {
            script.push((0, ev((k % 16) as u8, Message::NoteOn { key: (k / 16) as u8, velocity: 90, id: NoteId::NONE })));
        }
        for ch in 0..16u8 {
            for n in [64, 66, 67, 69] {
                script.push((0, ev(ch, Message::Control { cc: n, value: 127 })));
            }
        }
        let mut f = 1u64;
        for _ in 0..96 {
            script.push((f, cc(121, 0)));
            f += 1;
            for n in [64, 66, 67, 69] {
                script.push((f, cc(n, 127)));
                f += 1;
            }
        }
        let (recs, refused) = play(&mut p, &script, size, 1024);
        assert_eq!(refused, 0, "96 x CC 121 at size {size}");
        let v = read(&recs, true);
        assert!([64, 66, 67, 69].iter().all(|c| v.down(*c)), "channel 0's pedals end pressed at size {size}");
    }
}

#[test]
fn a_separation_at_every_frame_falls_behind_with_bounded_service_through_the_adapter() {
    for total in [1204u64, 4000] {
        let mut script = Vec::new();
        for f in 0..total {
            let key = ((f / 2) % 128) as u8;
            if f % 2 == 0 {
                script.push((f, cc(64, 127)));
                script.push((f, on(key)));
            } else {
                script.push((f, cc(64, 0)));
                script.push((f, off(key)));
            }
        }
        for size in SIZES {
            let mut p = synth(size);
            let (recs, refused) = play(&mut p, &script, size, total + 2600);
            // A refused start's note-off matches nothing and is not delivered; a refused press's
            // release is an ordinary pedal value and is. So at most every other event is delivered.
            assert!(recs.len() + refused <= script.len() && recs.len() + 2 * refused >= script.len(), "delivered {} refused {refused}", recs.len());
            let last = recs.iter().map(|r| r.frame).max().unwrap();
            let served: BTreeSet<u64> = recs.iter().map(|r| r.frame).collect();
            assert!((0..=last).all(|f| served.contains(&f)), "a frame served nothing at size {size}");
            assert!(last - (total - 1) <= 2480, "delay {} at size {size}", last - (total - 1));
            if total == 1204 && size == 1 {
                assert_eq!(refused, 0, "604 entries by frame 1,204 at block size 1");
            }
            if total == 4000 {
                assert!(refused > 0, "a sustained stream refuses starts at size {size}");
            }
            both(&recs, &Voices::default(), &format!("stream {total} at size {size}"));
        }
    }
}

#[test]
fn a_call_that_applies_its_first_k_events_and_fails_leaves_nothing_after_stop() {
    for k in 0..=4u32 {
        let mut p = synth(64);
        let mut base = 0;
        let mut io = vec![vec![0.0f32; 64]; 2];
        assert!(p.event(0, &cc(64, 127)));
        assert!(p.event(0, &on(60)));
        p.process(&mut io, 64);
        let mut recs = log(&mut p, &mut base);
        assert!(p.load_state(format!("STOP:{k}").as_bytes()));
        for e in [cc(64, 0), cc(64, 127), off(60), on(62)] {
            assert!(p.event(0, &e));
        }
        p.process(&mut io, 64); // applies its first k records, then fails
        assert_eq!(p.process_errors(), 1, "k = {k}");
        recs.extend(log(&mut p, &mut base));
        p.all_notes_off();
        p.reset();
        p.process(&mut io, 64);
        p.process(&mut io, 64);
        recs.extend(log(&mut p, &mut base));
        both(&recs, &Voices::default(), &format!("k = {k}"));
        assert_eq!(transit_vst3_host::UNTRANSLATED.load(std::sync::atomic::Ordering::Relaxed), 0);
    }
}

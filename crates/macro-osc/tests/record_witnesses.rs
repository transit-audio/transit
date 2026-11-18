//! Witnesses for the voice record's dormant, derived and unobservable classes (ADR 0029,
//! amendment for #742, sections 12 and 13; feature `testing`, which this crate's tests turn on).
//!
//! - Dormant engines: every engine switch calls the new engine's `reset()` (`voice.rs`, the
//!   engine selection), so what that reset rewrites in full is left out of the record while the
//!   engine is dormant. For every engine: A plays it, then another engine; a fresh B restores A's
//!   record (so B's dormant fields are a new voice's, not A's), its unobservable state is poisoned,
//!   and both switch back and play; outputs and records must stay identical at every block, with B
//!   poisoned again at every block boundary.

use std::hash::Hasher;
use transit_macro_osc::SampleRate;
use transit_macro_osc::record::{Poison, Reader, Record, Writer};
use transit_macro_osc::voice::{Frame, Modulations, NUM_ENGINES, Patch, Voice};

#[derive(Default)]
struct Words(Vec<u64>);

impl Hasher for Words {
    fn write(&mut self, _: &[u8]) {
        unreachable!("the record writer emits words only");
    }
    fn write_u64(&mut self, w: u64) {
        self.0.push(w);
    }
    fn finish(&self) -> u64 {
        0
    }
}

fn words(f: impl FnOnce(&mut Writer<'_>)) -> Vec<u64> {
    let mut sink = Words::default();
    f(&mut Writer::new(&mut sink));
    sink.0
}

fn record(v: &Voice) -> Vec<u64> {
    words(|w| v.record(w))
}

/// One 12-frame control block, as the Macro Oscillator drives the voice: the key as the note
/// modulation, the trigger and the level patched.
fn block(v: &mut Voice, engine: usize, harmonics: f32, morph: f32, gate: bool, key: f32) -> [Frame; 12] {
    let patch = Patch { note: 0.0, engine, harmonics, morph, decay: 0.4, ..Patch::default() };
    let mods = Modulations {
        note: key,
        trigger: if gate { 1.0 } else { 0.0 },
        level: if gate { 0.8 } else { 0.0 },
        trigger_patched: true,
        level_patched: true,
        ..Modulations::default()
    };
    let mut frames = [Frame::default(); 12];
    v.render(&patch, &mods, &mut frames);
    frames
}

/// The engine instance at `index`: the three six-operator models share one.
fn instance(index: usize) -> usize {
    if (2..=4).contains(&index) { 2 } else { index }
}

/// Engines whose reset rewrites state the record leaves out while they are dormant.
const WITH_DORMANT: [usize; 9] = [6, 7, 12, 14, 15, 16, 18, 19, 20];

/// A voice restored from `a`'s record (a new voice, so its dormant state is a new voice's).
fn transplant(a: &Voice) -> Voice<'static> {
    let rec = record(a);
    let mut b = Voice::new(SampleRate::Hz48000);
    let mut r = Reader::new(&rec);
    assert!(b.restore(&mut r).is_some() && r.finish(), "the restore refused the record or left words unread");
    assert_eq!(record(&b), rec, "the restored voice re-records other words");
    b
}

/// Blocks to play an engine for: past the particle engine's diffuser ring (8192 samples) and a
/// string's lines (1280 samples).
const PLAY: usize = 8192 / 12 + 40;

#[test]
fn a_dormant_engines_reset_state_is_unobservable_across_a_switch_to_it_for_every_engine() {
    for e in 0..NUM_ENGINES {
        let other = if instance(e) == 10 { 12 } else { 10 };
        let mut a = Voice::new(SampleRate::Hz48000);
        // Play `e` (its lines fill), then the other engine, so `e` is dormant and not a new
        // voice's.
        for k in 0..PLAY {
            block(&mut a, e, 0.3 + 0.4 * (k % 7) as f32 / 7.0, 0.2, k % 300 < 250, 48.0 + (k / 300) as f32 * 5.0);
        }
        for k in 0..40 {
            block(&mut a, other, 0.5, 0.5, k < 30, 60.0);
        }
        let mut b = transplant(&a);
        // Every value the poison writes differs from the one it replaces, so a nonzero count
        // means the engine's left-out state changed.
        let mut p = Poison::new(e as u32 + 1);
        b.poison_engine(e, &mut p);
        assert_eq!(p.count() > 0, WITH_DORMANT.contains(&e), "engine {e}: {} values poisoned", p.count());
        b.poison_unobservable(&mut p);
        assert_eq!(record(&a), record(&b), "engine {e}: poisoning changed the record");
        // Switch back to `e` and play it: strikes, releases and a held note, past its lines.
        for k in 0..PLAY + 120 {
            let (h, m, gate, key) = (0.2 + 0.6 * (k % 11) as f32 / 11.0, 0.3 + 0.4 * (k % 5) as f32 / 5.0, k % 200 < 150, 50.0 + (k / 200) as f32 * 3.0);
            let fa = block(&mut a, e, h, m, gate, key);
            let fb = block(&mut b, e, h, m, gate, key);
            assert_eq!(fa, fb, "engine {e}, block {k}: the outputs differ");
            assert_eq!(record(&a), record(&b), "engine {e}, block {k}: the records differ");
            b.poison_unobservable(&mut p);
        }
    }
}

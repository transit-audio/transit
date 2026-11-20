//! The part's own transplant witnesses (ADR 0029, amendment for #742, sections 12 and 13; feature
//! `testing`, which this crate's tests turn on), for state the Resonator instrument never sets and
//! so its witnesses cannot reach: bypass, the strum rule the instrument fixes at construction
//! (`set_strum_keeps_voice_notes`, here off in the receiver), and an odd polyphony's strum order
//! (`step_counter`; the instrument offers 1, 2 and 4 voices).
//!
//! Each case: A reaches T, a fresh B restores A's record, re-records exactly the same words, and
//! both run the same continuation; the outputs must stay identical bit for bit and the records
//! equal at every block, with B's unobservable state poisoned after the restore and after every
//! block.

use std::hash::Hasher;
use transit_resonator::record::{Poison, Reader, Record, Writer};
use transit_resonator::{MAX_BLOCK_SIZE, Patch, PerformanceState, Resonator, ResonatorModel};

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

fn record(v: &Resonator) -> Vec<u64> {
    let mut sink = Words::default();
    v.record(&mut Writer::new(&mut sink));
    sink.0
}

/// One part block: a strum of `note` when `strum`, the internal exciter, and a nonzero audio input
/// (the bypass copies it).
fn block(v: &mut Resonator, strum: bool, note: f32, k: usize) -> Vec<u32> {
    let performance = PerformanceState { strum, internal_exciter: true, tonic: 36.0, note, fm: 0.0, chord: 3, ..PerformanceState::default() };
    let patch = Patch { structure: 0.6, brightness: 0.7, damping: 0.4, position: 0.3 };
    let input: [f32; MAX_BLOCK_SIZE] = std::array::from_fn(|i| ((i + k * 7) % 13) as f32 / 13.0 - 0.5);
    let (mut out, mut aux) = ([0.0f32; MAX_BLOCK_SIZE], [0.0f32; MAX_BLOCK_SIZE]);
    v.process(&performance, &patch, &input, &mut out, &mut aux);
    out.iter().chain(&aux).map(|x| x.to_bits()).collect()
}

/// What one continuation block does: a strum of a note, and the bypass switch (`None`: unchanged).
type Script = fn(usize) -> (bool, f32, Option<bool>);

/// Restores A's record into `b`, then runs `blocks` of `script` on both, comparing outputs and
/// records at every block; B's unobservable state is poisoned after the restore and every block.
fn transplant(a: &mut Resonator, mut b: Resonator, blocks: usize, script: Script) {
    let rec = record(a);
    let mut r = Reader::new(&rec);
    b.restore(&mut r).expect("the restore takes the record");
    assert!(r.finish(), "the restore reads every word");
    assert_eq!(record(&b), rec, "the restored part re-records the same words");
    let mut p = Poison::new(7);
    b.poison_unobservable(&mut p);
    for k in 0..blocks {
        let (strum, note, bypass) = script(k);
        if let Some(on) = bypass {
            a.set_bypass(on);
            b.set_bypass(on);
        }
        assert_eq!(block(a, strum, note, k), block(&mut b, strum, note, k), "block {k}: the outputs differ");
        assert_eq!(record(a), record(&b), "block {k}: the records differ");
        b.poison_unobservable(&mut p);
    }
}

/// Bypassed at T: the part copies its input; the continuation stays bypassed for a while, then
/// plays again (the voices kept what they had).
#[test]
fn a_bypassed_part_transplants() {
    let mut a = Resonator::new();
    a.set_model(ResonatorModel::SympatheticStrings);
    for k in 0..40 {
        block(&mut a, k % 9 == 0, 12.0 + k as f32, k);
    }
    a.set_bypass(true);
    block(&mut a, false, 0.0, 99);
    let mut b = Resonator::new();
    b.set_model(ResonatorModel::SympatheticStrings);
    transplant(&mut a, b, 300, |k| (k == 100 || k == 160, 19.0, (k == 60).then_some(false)));
}

/// The strum rule on at T in A and off in the receiver (its default): strums on consecutive blocks
/// (a chord) then keep each outgoing voice on its note only if the rule was restored.
#[test]
fn the_strum_rule_transplants() {
    let mut a = Resonator::new();
    a.set_strum_keeps_voice_notes(true);
    a.set_polyphony(4);
    for k in 0..20 {
        block(&mut a, k == 2, 10.0, k);
    }
    transplant(&mut a, Resonator::new(), 200, |k| (k < 4, 10.0 + 5.0 * k as f32, None));
}

/// Three voices strum in the firmware's ping order (`step_counter`): T after two strums.
#[test]
fn an_odd_polyphony_strum_order_transplants() {
    let mut a = Resonator::new();
    a.set_polyphony(3);
    for k in 0..30 {
        block(&mut a, k == 3 || k == 20, 14.0, k);
    }
    let mut b = Resonator::new();
    b.set_polyphony(3);
    transplant(&mut a, b, 300, |k| (k % 25 == 0, 14.0 + (k / 25) as f32, None));
}

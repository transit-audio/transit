//! The part's own transplant witnesses (ADR 0029, amendment for #742, sections 12 and 13; feature
//! `testing`, which this crate's tests turn on), for state the Modal Voice instrument never sets and
//! so its witnesses cannot reach: a seed (`seed` scrambles five patch values the record encodes),
//! bypass, and the runaway guard's flag (`panic`, which resets the resonators on the next block).
//!
//! Each case: A reaches T, a fresh B restores A's record, re-records exactly the same words, and
//! both run the same continuation, writing every control and the raw share before each block (the
//! owner's obligation the record relies on); the outputs must stay identical bit for bit and the
//! records equal at every block, with B's unobservable state poisoned after the restore and after
//! every block.

use std::hash::Hasher;
use transit_modal_voice::{MAX_BLOCK_SIZE, ModalVoice, Patch, PerformanceState, ResonatorModel};
use transit_resonator::record::{Poison, Reader, Record, Writer};

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

fn record(v: &ModalVoice) -> Vec<u64> {
    let mut sink = Words::default();
    v.record(&mut Writer::new(&mut sink));
    sink.0
}

/// One part block: every control and the raw share written first, the gate as given, nonzero
/// inputs (the bypass copies them).
fn block(v: &mut ModalVoice, gate: bool, k: usize) -> Vec<u32> {
    *v.patch_mut() = Patch {
        exciter_envelope_shape: 0.3,
        exciter_bow_level: 0.5,
        exciter_bow_timbre: 0.4,
        exciter_blow_level: 0.8,
        exciter_blow_meta: 0.6,
        exciter_blow_timbre: 0.5,
        exciter_strike_level: 0.9,
        exciter_strike_meta: 0.3,
        exciter_strike_timbre: 0.7,
        resonator_geometry: 0.4,
        resonator_brightness: 0.6,
        resonator_damping: 0.6,
        resonator_position: 0.35,
        space: 0.8,
    };
    v.set_raw_share(1.0);
    let performance = PerformanceState { gate, note: 57.0, modulation: 0.0, strength: 0.8 };
    let blow: [f32; MAX_BLOCK_SIZE] = std::array::from_fn(|i| ((i + k * 5) % 11) as f32 / 11.0 - 0.5);
    let strike: [f32; MAX_BLOCK_SIZE] = std::array::from_fn(|i| ((i + k * 3) % 7) as f32 / 7.0 - 0.5);
    let (mut main, mut aux) = ([0.0f32; MAX_BLOCK_SIZE], [0.0f32; MAX_BLOCK_SIZE]);
    v.process(&performance, &blow, &strike, &mut main, &mut aux);
    main.iter().chain(&aux).map(|x| x.to_bits()).collect()
}

/// What one continuation block does: the gate, and the bypass switch (`None`: unchanged).
type Script = fn(usize) -> (bool, Option<bool>);

/// Restores A's record into `b`, then runs `blocks` of `script` on both, comparing outputs and
/// records at every block; B's unobservable state is poisoned after the restore and every block.
fn transplant(a: &mut ModalVoice, mut b: ModalVoice, blocks: usize, script: Script) {
    let rec = record(a);
    let mut r = Reader::new(&rec);
    b.restore(&mut r).expect("the restore takes the record");
    assert!(r.finish(), "the restore reads every word");
    assert_eq!(record(&b), rec, "the restored part re-records the same words");
    let mut p = Poison::new(11);
    b.poison_unobservable(&mut p);
    b.poison_owner_written(&mut p);
    for k in 0..blocks {
        let (gate, bypass) = script(k);
        if let Some(on) = bypass {
            a.set_bypass(on);
            b.set_bypass(on);
        }
        assert_eq!(block(a, gate, k), block(&mut b, gate, k), "block {k}: the outputs differ");
        assert_eq!(record(a), record(&b), "block {k}: the records differ");
        b.poison_unobservable(&mut p);
        b.poison_owner_written(&mut p);
    }
}

/// A seeded part (the module's serial number scrambles the side LFO, the reverb's diffusion and
/// low-pass and the exciter signature) into an unseeded receiver.
#[test]
fn a_seeded_part_transplants() {
    let mut a = ModalVoice::new();
    a.seed(&[0x1234_5678, 0x9abc_def0, 0x0f0f_0f0f]);
    for k in 0..50 {
        block(&mut a, k % 20 < 12, k);
    }
    transplant(&mut a, ModalVoice::new(), 600, |k| (k % 40 < 25, None));
}

/// Bypassed at T: the part copies its inputs; the continuation stays bypassed, then plays again.
#[test]
fn a_bypassed_part_transplants() {
    let mut a = ModalVoice::new();
    a.set_model(ResonatorModel::Strings);
    for k in 0..50 {
        block(&mut a, k % 20 < 12, k);
    }
    a.set_bypass(true);
    block(&mut a, false, 50);
    transplant(&mut a, ModalVoice::new(), 400, |k| (k > 100 && k % 40 < 25, (k == 80).then_some(false)));
}

/// The runaway guard raised at T: the next block resets the resonators and drops the flag.
#[test]
fn a_raised_runaway_guard_transplants() {
    let mut a = ModalVoice::new();
    a.set_model(ResonatorModel::String);
    for k in 0..50 {
        block(&mut a, k % 20 < 12, k);
    }
    a.force_panic();
    transplant(&mut a, ModalVoice::new(), 400, |k| (k % 40 < 25, None));
}

/// The resonator meter's integrator just below the runaway guard at T (it decays slowly; the
/// guard trips at 200): the continuation's blocks keep it there or trip the guard only if the
/// restored value is the recorded one.
#[test]
fn a_resonator_level_near_the_guard_transplants() {
    let mut a = ModalVoice::new();
    for k in 0..50 {
        block(&mut a, k % 20 < 12, k);
    }
    a.force_resonator_level(199.99);
    transplant(&mut a, ModalVoice::new(), 300, |k| (k % 40 < 25, None));
}

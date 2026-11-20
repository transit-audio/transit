//! The sequencer's own transplant witnesses (ADR 0029, amendment for #742, sections 12 and 13;
//! feature `testing`, which this crate's tests turn on): a voice turned on while its gate is high
//! plays its channel's last drawn voltage (the sequencer's `key` reads the channel's voltage and
//! its quantization, not a new draw), at every gate length and each model, so the output channels'
//! recorded voltages and quantizers are read right after T.
//!
//! Each case: A plays with gate 1 alone, then at T every voice turns on; a fresh B restores A's
//! record, re-records exactly the same words, and both run on; the emitted events (offsets
//! included) must stay identical and the records equal at every block, with B's unobservable
//! state poisoned after the restore and after every block.

use std::hash::Hasher;
use transit_random_sequencer::record::{Poison, Reader, Record, Writer};
use transit_random_sequencer::{Block, Event, Model, Sequencer, Settings, Voices};

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

fn record(v: &Sequencer) -> Vec<u64> {
    let mut sink = Words::default();
    v.record(&mut Writer::new(&mut sink));
    sink.0
}

const FRAMES: u32 = 256;

fn block(v: &mut Sequencer, s: &Settings, k: i64) -> Vec<(u32, String)> {
    let b = Block {
        playing: true,
        samples: k * i64::from(FRAMES),
        quarters: k as f64 * f64::from(FRAMES) * 2.0 / 48_000.0,
        tempo: 120.0,
        sample_rate: 48_000.0,
        frames: FRAMES,
    };
    let mut out = Vec::new();
    v.process(s, &b, &mut |e: Event| out.push((e.offset, format!("{:?}", e.kind))));
    out
}

#[test]
fn a_voice_turned_on_mid_gate_transplants() {
    let mut caught_paths = 0;
    for (model, gate) in [(Model::CoinToss, 1.0), (Model::Clusters, 0.98), (Model::Independent, 0.9), (Model::Drums, 1.0)] {
        let before = Settings { model, gate_length: gate, gate_random: 0.0, voices: Voices::Gate1, steps: 0.8, rate: 13, ..Settings::default() };
        let after = Settings { voices: Voices::All, ..before };
        let mut a = Sequencer::new(7, true);
        for k in 0..400 {
            block(&mut a, &before, k);
        }
        let rec = record(&a);
        let mut b = Sequencer::new(1, false);
        let mut r = Reader::new(&rec);
        b.restore(&mut r).expect("the restore takes the record");
        assert!(r.finish(), "the restore reads every word");
        assert_eq!(record(&b), rec, "the restored sequencer re-records the same words");
        let mut p = Poison::new(3);
        b.poison_unobservable(&mut p);
        for k in 400..1200 {
            let (ea, eb) = (block(&mut a, &after, k), block(&mut b, &after, k));
            assert_eq!(ea, eb, "{model:?}: block {k}: the emitted events differ");
            assert_eq!(record(&a), record(&b), "{model:?}: block {k}: the records differ");
            caught_paths += ea.len();
            b.poison_unobservable(&mut p);
        }
    }
    assert!(caught_paths > 0, "the continuation emitted nothing");
}

/// The quantizers' level hysteresis: Steps put the level at 1 before T (7 × 0.2 = 1.4); at T it
/// drops to where the kept level rounds to 1 (quantized to the scale) and a fresh quantizer's to 0
/// (not quantized; 7 × 0.0857 = 0.6), so the pitches after T follow the restored level.
#[test]
fn the_quantizer_level_hysteresis_transplants() {
    let before = Settings { gate_length: 0.5, voices: Voices::All, steps: 0.2, rate: 13, ..Settings::default() };
    let after = Settings { steps: 0.0857, ..before };
    let mut a = Sequencer::new(11, true);
    for k in 0..300 {
        block(&mut a, &before, k);
    }
    let rec = record(&a);
    let mut b = Sequencer::new(1, false);
    let mut r = Reader::new(&rec);
    b.restore(&mut r).expect("the restore takes the record");
    assert!(r.finish(), "the restore reads every word");
    let mut p = Poison::new(5);
    b.poison_unobservable(&mut p);
    for k in 300..900 {
        assert_eq!(block(&mut a, &after, k), block(&mut b, &after, k), "block {k}: the emitted events differ");
        assert_eq!(record(&a), record(&b), "block {k}: the records differ");
        b.poison_unobservable(&mut p);
    }
}

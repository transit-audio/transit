//! The full 256-leaf fixture through the desktop event and generation boundary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../sampler-core/tests/support/full_patch.rs"]
mod fixture;

use std::sync::Arc;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::plugins::sampler::{PatchSpec, Sampler};
use transit_dsp::{Plugin, SampleBuffer, SampleSource, Transport};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn spec(sources: &[Arc<dyn SampleSource>]) -> PatchSpec {
    PatchSpec { sources: sources.to_vec(), zones: fixture::zones(false), pads: fixture::pads(), groups: fixture::groups() }
}

#[test]
fn no_allocation_with_all_zones_and_groups() {
    let sources: Vec<Arc<dyn SampleSource>> = fixture::data()
        .into_iter()
        .map(|p| Arc::new(SampleBuffer { sample_rate: fixture::RATE, channels: vec![p] }) as Arc<dyn SampleSource>)
        .collect();
    let mut s = Sampler::new();
    s.prepare(fixture::RATE, 512, 2);
    let mut publisher = s.enable_core(fixture::SEED);
    for z in 1..=4 {
        for (field, v) in [("shaper", 1.0), ("cutoff", 1024.0), ("resonance", 25.0)] {
            assert!(s.set_param(&format!("zone{z:03}_{field}"), v));
        }
        for row in 1..=16 {
            for (field, value) in [("source", (row % 12 + 1) as f32), ("dest", (row % 8 + 6) as f32), ("amount", 2.5)] {
                assert!(s.set_param(&format!("zone{z:03}_mod{row}_{field}"), value));
            }
        }
    }
    publisher.publish(spec(&sources)).unwrap();
    let note = |key, on| PluginEvent {
        channel: 0,
        note_id: -1,
        message: if on { Message::NoteOn { key, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key, velocity: 64, id: NoteId::NONE } },
    };
    let mut io = vec![vec![0.0; 512]; 2];
    let mut audible = false;
    let mut retired = 0;
    for block in 0..12 {
        if block == 3 {
            let mut changed = sources.clone();
            changed[0] = Arc::new(SampleBuffer { sample_rate: fixture::RATE, channels: vec![fixture::noise(999)] });
            publisher.publish(spec(&changed)).unwrap();
        }
        let ((), stats) = measure(|| {
            s.set_transport(&Transport {
                playing: block < 8,
                samples: block * 512,
                quarters: block as f64 * 512.0 / 24000.0,
                tempo: 120.0,
                ..Transport::default()
            });
            if block < 6 {
                // Fill the event budget, including its release reserve, on the real event path.
                for i in 0..767u32 {
                    let key = match i % 4 {
                        0 => (i % 16) as u8,
                        1 => 36 + (i % 16) as u8,
                        _ => 64 + (i % 56) as u8,
                    };
                    assert!(s.event(i % 400, &note(key, true)));
                }
                assert!(!s.event(0, &note(0, true)));
                for i in 0..257u32 {
                    assert!(s.event(450, &note((i % 120) as u8, false)));
                }
                assert!(s.set_param("zone001_pan", block as f32 * 10.0));
            }
            if block == 7 {
                s.all_notes_off();
            }
            if block == 10 {
                s.reset();
            }
            s.process(&mut io, 512);
        });
        assert!(stats.is_zero(), "block {block}: {stats:?}");
        assert!(io.iter().flatten().all(|x| x.is_finite()));
        audible |= io.iter().flatten().any(|x| *x != 0.0);
        retired += publisher.collect();
    }
    assert!(audible);
    assert_eq!(retired, 1, "the replaced generation was destroyed on the control side");
    assert!(io.iter().flatten().all(|x| *x == 0.0), "reset ended every voice");
}

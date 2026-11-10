//! Combined bounds and determinism evidence for the complete instrument patch.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/full_patch.rs"]
mod fixture;

use fixture::*;
use transit_sampler_core::event::{Context, Event, EventKind};
use transit_sampler_core::patch::{GroupMode, PlayMode, Zone};
use transit_sampler_core::reader::SliceSource;
use transit_sampler_core::{SamplerCore, VOICES};

#[test]
fn bits_equal_at_64_and_512_frames() {
    let pcm = data();
    for shaped in [false, true] {
        let mut reference = core(shaped);
        let (want, peak) = render(&mut reference, &pcm, &[64]);
        assert_eq!(peak, VOICES);
        assert!(reference.diagnostics().steals > 0 && reference.diagnostics().chokes > 0);
        assert!(want.iter().any(|b| *b != 0));
        for sizes in [&[128][..], &[512], &[97], &[1], &[7], &[127], &[1, 97, 512, 7, 64]] {
            let mut c = core(shaped);
            let (got, _) = render(&mut c, &pcm, sizes);
            assert_eq!(got.iter().zip(&want).position(|(a, b)| a != b), None, "shaped {shaped}, blocks {sizes:?}");
            assert_eq!(c.diagnostics(), reference.diagnostics());
            assert_eq!(c.playing_sources().collect::<Vec<_>>(), reference.playing_sources().collect::<Vec<_>>());
        }
    }
}

#[test]
fn full_load_unshaped_golden_hash() {
    let (bits, _) = render(&mut core(false), &data(), &[64]);
    assert_eq!(hash(&bits), 0x386a_e5b2_795e_f13b);
}

fn process(c: &mut SamplerCore, pcm: &[Vec<f32>], events: &[Event], frames: usize) -> Vec<f32> {
    let sources: Vec<_> = pcm.iter().map(|p| SliceSource::mono(p, RATE)).collect();
    let mut out = vec![0.0; frames];
    c.process(&Context::default(), &sources[..], events, &mut [&mut out]);
    out
}

#[test]
fn missing_media_zone_stays_silent_and_keeps_order() {
    let pcm = data();
    for mode in [GroupMode::Sequential, GroupMode::Random] {
        let mut complete = core(false);
        let mut missing = core(false);
        let mut modes = groups();
        modes[0] = mode;
        for c in [&mut complete, &mut missing] {
            c.set_group_modes(&modes);
        }
        let mut z = *missing.zone(3).unwrap();
        z.region.source = u16::MAX;
        missing.set_zone(3, Some(z)).unwrap();
        let mut silent_pcm = pcm.clone();
        silent_pcm[3].fill(0.0);
        for _ in 0..32 {
            let events = [Event::new(0, EventKind::AllSoundOff), Event::on(0, 3, 127)];
            let want = process(&mut complete, &silent_pcm, &events, 32);
            let got = process(&mut missing, &pcm, &events, 32);
            assert_eq!(got, want, "a missing alternative keeps its turn");
        }
        assert!(missing.diagnostics().refused_starts > 0);
        missing.set_zone(3, complete.zone(3).copied()).unwrap();
        for _ in 0..16 {
            let events = [Event::new(0, EventKind::AllSoundOff), Event::on(0, 3, 127)];
            assert_eq!(process(&mut missing, &pcm, &events, 32), process(&mut complete, &pcm, &events, 32));
        }
    }
}

#[test]
fn all_notes_off_and_stop_end_every_voice() {
    let pcm = vec![vec![1.0; 4096]; 256];
    for ending in [EventKind::AllNotesOff, EventKind::Stop] {
        let mut c = core(false);
        // Eight notes fill 64 Gate voices, then a slice one-shot steals one of them.
        let mut events: Vec<_> = (0..8).map(|key| Event::on(0, key, 127)).collect();
        events.push(Event::on(0, 42, 127));
        process(&mut c, &pcm, &events, 1);
        assert_eq!(c.active_voices(), VOICES);
        // Remove fades/velocity weighting so every active voice contributes unity.
        // Rebuild and retrigger to capture unity at the event, including the one-shot.
        for z in 0..256 {
            let zone = *c.zone(z).unwrap();
            c.set_zone(
                z,
                Some(Zone {
                    fades: Default::default(),
                    playback: transit_sampler_core::patch::Playback { velocity_sensitivity: 0.0, ..zone.playback },
                    ..zone
                }),
            )
            .unwrap();
        }
        c.reset();
        process(&mut c, &pcm, &events, 1);
        let out = process(&mut c, &pcm, &[Event::new(0, ending)], 40);
        for (n, got) in out.iter().enumerate() {
            let want = if n < 32 { 64.0 * (1.0 - (n + 1) as f32 / 32.0) } else { 0.0 };
            assert_eq!(got.to_bits(), want.to_bits(), "{ending:?}, frame {n}");
        }
        assert_eq!(c.active_voices(), 0);
    }
}

#[test]
fn voice_steal_order_is_deterministic() {
    let pcm = data();
    let run = || {
        let mut c = core(false);
        // One loose zone per key: source IDs expose the physical voice-slot order.
        let zones: Vec<_> = (0..256)
            .map(|z| {
                let old = *c.zone(z).unwrap();
                Some(Zone { membership: None, keys: (127, 127), ..old })
            })
            .collect();
        let pads = core::array::from_fn(transit_sampler_core::patch::Pad::new);
        c.set_patch(&zones, &pads).unwrap();
        for key in 0..65u8 {
            let mut z = *c.zone(144 + usize::from(key)).unwrap();
            z.keys = (key, key);
            z.velocities = (1, 127);
            z.playback.mode = PlayMode::Gate;
            z.playback.release_ms = 1000.0;
            c.set_zone(144 + usize::from(key), Some(z)).unwrap();
        }
        let ons: Vec<_> = (0..64).map(|key| Event::on(0, key, 127)).collect();
        process(&mut c, &pcm, &ons, 1);
        assert_eq!(c.playing_sources().collect::<Vec<_>>(), (144..208).collect::<Vec<u16>>());
        process(&mut c, &pcm, &[Event::off(0, 21), Event::off(0, 9), Event::on(0, 64, 127)], 1);
        let slots = c.playing_sources().collect::<Vec<_>>();
        assert_eq!(slots[21], 208, "oldest release wins, even above a lower slot");
        process(&mut c, &pcm, &[Event::on(0, 64, 127)], 1);
        assert_eq!(c.playing_sources().nth(9), Some(208));
        process(&mut c, &pcm, &[Event::on(0, 64, 127)], 1);
        assert_eq!(c.playing_sources().next(), Some(208), "oldest sounding voice");
        (process(&mut c, &pcm, &[], 64), c.playing_sources().collect::<Vec<_>>())
    };
    assert_eq!(run(), run());
}

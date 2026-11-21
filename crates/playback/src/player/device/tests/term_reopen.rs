//! #709 device reopens through the fake device (ADR 0015, Amendment 2026-11-16 on the converter
//! term). A reopen is a new Player on the same device, as the app builds one after a reset, a
//! lost device or Apply: its input is aligned anew from the output converter's coordinates
//! (rule 3), so at a converted rate the round trip gains exactly the term T again, and at equal
//! rates it is the only thing that claims the term again after another speed (the amendment's
//! build note: "after another speed the term is not claimed again at equal rates until the input
//! is aligned anew (a device reopen)").

use super::converted_rate::{Prelude, WARM, delay, open_on, probe_on, start_bound};
use super::*;
use transit_model::{Alignment, converter_term};

/// Lockstep callbacks of 128 frames with silent input on an open device.
fn pump(player: &Player, fake: &ab::fake::FakeBackend, device: u32, callbacks: usize) {
    use ab::fake::{SliceSink, SliceSource};
    let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
    let (mut output, input) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for _ in 0..callbacks {
        assert!(fake.pump_capture(128, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 }));
        assert!(fake.pump_render(128, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 }));
        player.service().unwrap();
    }
}

/// The acceptance list's "device reopens" at a converted rate: with the term in force (the first
/// open's probe reads exactly T over the ideal round trip, term provenance, Valid), the device is
/// lost and reopened on the same backend. The reopened Player's probe reads exactly T again, with
/// term provenance and a Valid alignment, its start-up silence within rule 9's bound and none
/// after, whether its ring was empty or prefilled when the plan first read it. Both ways at 44.1
/// and 48 kHz, two callback patterns.
#[test]
fn a_device_reopen_while_the_term_is_in_use_gains_exactly_the_term_again() {
    for sizes in [&[128usize][..], &[100, 37, 256]] {
        for (session, device) in [(48_000u32, 44_100u32), (44_100, 48_000)] {
            for reopened in [Prelude::RenderOnly(4), Prelude::CaptureOnly(3)] {
                let at = format!("{session} on {device}, callbacks {sizes:?}, reopened with {reopened:?}");
                let term = i64::from(converter_term(session, device));
                let (delay, ideal) = delay(session, device, sizes);
                let fake = ab::fake::FakeBackend::new(ab::Format::new(device, 2, ab::SampleFormat::F32)).with_capture(2, true);
                {
                    let (player, recorder) = open_on(&fake, true, session);
                    let first = probe_on(&player, &fake, &recorder, Prelude::RenderOnly(4), session, device, delay, sizes, WARM);
                    assert_eq!(first.m.round_trip_frames as i64 - ideal, term, "{at}: the first open");
                    assert!(first.m.term && first.alignment == Alignment::Valid, "{at}: the term is in use before the reopen");
                    assert!(fake.inject_error(ab::Direction::Output, ab::StreamError::DeviceNotAvailable), "{at}");
                }
                let (player, recorder) = open_on(&fake, true, session);
                let p = probe_on(&player, &fake, &recorder, reopened, session, device, delay, sizes, WARM);
                assert_eq!(p.m.round_trip_frames as i64 - ideal, term, "{at}: the reopened device gains exactly T");
                assert_eq!((p.m.found, p.m.spread_frames), (p.m.sent, 0), "{at}");
                if let Prelude::RenderOnly(_) = reopened {
                    // A prefilled ring drops what lies before the first tap (rule 9); an empty one has nothing to drop.
                    assert_eq!(p.discarded, 0, "{at}");
                }
                assert!(p.m.term, "{at}: term provenance after the reopen");
                assert_eq!(p.alignment, Alignment::Valid, "{at}");
                let bound = start_bound(session, device);
                assert!(p.underrun_start <= bound, "{at}: {} frames of silence while capture starts, bound {bound}", p.underrun_start);
                assert_eq!(p.underrun_later, 0, "{at}: silence after the start");
            }
        }
    }
}

/// At equal rates after another speed the term (0 there) is not claimed again by a stop or a
/// locate at 1x, and a Measure then has no term provenance; a device reopen claims it again: the
/// reopened Player's input is Valid and its Measure has term provenance with the ideal round trip.
#[test]
fn at_equal_rates_after_another_speed_only_a_device_reopen_claims_the_term_again() {
    for rate in [44_100u32, 48_000] {
        let at = format!("{rate} on {rate}");
        let sizes = &[128usize][..];
        let (delay, ideal) = delay(rate, rate, sizes);
        let fake = ab::fake::FakeBackend::new(ab::Format::new(rate, 2, ab::SampleFormat::F32)).with_capture(2, true);
        {
            let (player, recorder) = open_on(&fake, true, rate);
            pump(&player, &fake, rate, 20);
            assert_eq!(player.input_alignment(), Alignment::Valid, "{at}: claimed before the speed change");
            player.set_speed(2.0);
            pump(&player, &fake, rate, 10);
            player.set_speed(1.0);
            pump(&player, &fake, rate, 10);
            player.stop().unwrap();
            pump(&player, &fake, rate, 10);
            assert_eq!(player.input_alignment(), Alignment::Invalid, "{at}: a stop at 1x does not claim it");
            player.locate(0).unwrap();
            pump(&player, &fake, rate, 10);
            assert_eq!(player.input_alignment(), Alignment::Invalid, "{at}: nor a locate at 1x");
            let p = probe_on(&player, &fake, &recorder, Prelude::None, rate, rate, delay, sizes, WARM);
            assert!(!p.m.term, "{at}: a Measure now has no term provenance");
            assert_eq!(p.alignment, Alignment::Invalid, "{at}: still not claimed after the whole Measure");
        }
        let (player, recorder) = open_on(&fake, true, rate);
        let p = probe_on(&player, &fake, &recorder, Prelude::RenderOnly(4), rate, rate, delay, sizes, WARM);
        assert_eq!(p.alignment, Alignment::Valid, "{at}: the reopen claims it");
        assert!(p.m.term, "{at}: the reopened device's Measure has term provenance");
        assert_eq!(p.m.round_trip_frames as i64, ideal, "{at}: and T is 0: the ideal round trip");
    }
}

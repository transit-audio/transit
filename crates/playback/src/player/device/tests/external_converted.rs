//! #709, the External Instrument acceptance case at a converted rate (ADR 0015, Amendment
//! 2026-11-16 on the converter term, Acceptance: "For an External Instrument it generates the fake
//! synth's device-rate response from the injected MIDI presentation stamps and applies the input
//! filter"; ADR 0020's entry of the same day). The fake synth answers each Measure note-on at its
//! presentation stamp, as pushed for the MIDI output thread (`MEASURE_NOTE_STAMPS`), plus a
//! physical delay of 20 ms in device frames. The callbacks are stamped by frame
//! (`FRAME_CLOCK_BASE`), so a stamp is a device position: `(at_ns - base) * D / 1e9`.
//!
//! The answer is a Gaussian pulse (sigma 3 device frames), inside both converters' passband, so the
//! input filter leaves it a Gaussian in plan frames centred where rule 3 reads the answer's device
//! position: R0 + the ideal round trip + T. The onset detector's bias on that shape is taken from
//! the same detector on that analytic plan-rate reference (`external_probe::analyze`), so the
//! measured figure must be exactly the reference's for ideal + T, and differs from the reference
//! for T - 1 and T + 1.

use super::*;
use crate::external_probe;
use ab::fake::{SliceSink, SliceSource};
use transit_midi_io::fake::Fake;
use transit_model::{ChannelFormat, MidiOutChannel, MidiOutPort, MidiOutput, TrackKind, converter_term};

/// The answer's width, in device frames.
const SIGMA: f64 = 3.0;
/// The answer's peak.
const LEVEL: f64 = 0.5;
/// Callbacks before the measurement arms.
const ARM_AT: usize = 20;
/// The frame clock's host time of output frame 0.
const BASE_NS: u64 = 1_000_000;

fn pulse(offset: f64) -> f64 {
    LEVEL * (-(offset * offset) / (2.0 * SIGMA * SIGMA)).exp()
}

/// The Measure's figure from an analytic plan-rate return: the answer to each note `round_trip`
/// plan frames after its R0, a Gaussian of `SIGMA` device frames (`SIGMA * q` plan frames). The
/// notes keep the Measure's spacing; where R0 falls does not matter, only the distance.
fn reference(session: u32, device: u32, round_trip: u64) -> external_probe::Measured {
    let q = f64::from(session) / f64::from(device);
    let frames = |ms: u64| u64::from(session) * ms / 1000;
    let sent: Vec<u64> = (0..=external_probe::NOTES as u64).map(|k| frames(300 + 1500 * k)).collect();
    let mut recorded = vec![0.0f32; frames(300 + 1500 * 9 + 1000) as usize];
    for r0 in &sent {
        let at = r0 + round_trip;
        for r in at - 60..at + 60 {
            recorded[r as usize] += pulse((r as f64 - at as f64) / q) as f32;
        }
    }
    external_probe::analyze(&recorded, &sent, session).expect("the reference measures")
}

struct Run {
    m: external_probe::Measured,
    /// Each tapped stamp's device position against the frames pumped when it was pushed.
    stamps: Vec<(f64, u64)>,
}

/// An External Instrument returning on In 1 of a paired fake device at `device` Hz under a
/// `session` Hz plan; its Measure with the synth answering 20 ms (device frames) after each
/// note-on's presentation stamp, callbacks of `sizes` frames in turn.
fn measure(session: u32, device: u32, sizes: &[usize]) -> Run {
    crate::player::audio::FRAME_CLOCK_BASE.set(Some(BASE_NS));
    crate::player::audio::MEASURE_NOTE_STAMPS.with_borrow_mut(Vec::clear);
    let midi = Fake::default();
    let key = midi.add_output("MODEL D");
    let mut s = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let t = s.track_mut(track).unwrap();
    t.instrument = Some(transit_model::Insert::external("In 1"));
    t.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::Channel(2),
        offset_ms: 0.0,
    };
    let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
    let audio = ab::fake::FakeBackend::new(format).with_capture(2, true);
    let cfg = AudioConfig::default();
    let mut player = Player::open_native_or_cpal(Arc::new(s), &cfg, &audio, &fake_factory(false)).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(midi.io());
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let widest = sizes.iter().copied().max().unwrap();
    let (mut output, mut input) = (vec![0.0f32; widest * 2], vec![0.0f32; widest * 2]);
    // Device frames pumped so far: capture frame j and output frame j share a cycle.
    let (mut frame, mut calls) = (0u64, 0usize);
    let delay = f64::from(device / 50);
    let (mut centres, mut stamps) = (Vec::<f64>::new(), Vec::new());
    let mut pump = |player: &Player, frame: &mut u64, centres: &mut Vec<f64>, stamps: &mut Vec<(f64, u64)>| {
        let n = sizes[calls % sizes.len()];
        calls += 1;
        for f in 0..n {
            let j = (*frame + f as u64) as f64;
            input[f * 2] = centres.iter().filter(|c| (j - **c).abs() < 40.0).map(|c| pulse(j - c)).sum::<f64>() as f32;
            input[f * 2 + 1] = 0.0;
        }
        assert!(audio.pump_capture(n, ab::Stamp::INVALID, format, &mut SliceSource { data: &input[..n * 2], channels: 2 }));
        assert!(audio.pump_render(n, ab::Stamp::INVALID, &mut SliceSink { data: &mut output[..n * 2], channels: 2 }));
        *frame += n as u64;
        player.service().unwrap();
        let tapped = crate::player::audio::MEASURE_NOTE_STAMPS.with_borrow(Clone::clone);
        for at_ns in tapped.iter().skip(stamps.len()) {
            let p = (at_ns - BASE_NS) as f64 * f64::from(device) / 1e9;
            stamps.push((p, *frame));
            centres.push(p + delay);
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while player.midi_status().unwrap().output.track(track) != Some(&crate::player::midi_out::DestinationState::Connected) {
        assert!(std::time::Instant::now() < deadline, "the destination did not open");
        pump(&player, &mut frame, &mut centres, &mut stamps);
    }
    for _ in 0..ARM_AT {
        pump(&player, &mut frame, &mut centres, &mut stamps);
    }
    player.start_external_measure(track, &[0]).unwrap_or_else(|e| panic!("{e}"));
    let callbacks = (player.external_measure_duration().as_secs_f64() * f64::from(device)) as usize / widest * sizes.len() + 400;
    for _ in 0..callbacks {
        pump(&player, &mut frame, &mut centres, &mut stamps);
        if let Some(r) = player.external_measure_result() {
            crate::player::audio::FRAME_CLOCK_BASE.set(None);
            let m = r.unwrap_or_else(|e| panic!("{session} on {device}, {sizes:?}: {e:?}"));
            return Run { m, stamps };
        }
    }
    panic!("{session} on {device}, {sizes:?}: no result");
}

/// The acceptance case at 48 kHz on 44.1 kHz and 44.1 kHz on 48 kHz: every note's latency is
/// exactly the reference's for the ideal round trip (20 ms) plus T, the figure has term
/// provenance, and the reference is one frame away for T - 1 and T + 1.
#[test]
fn an_external_instrument_measure_at_a_converted_rate_reads_the_ideal_round_trip_plus_exactly_the_term() {
    for sizes in [&[128usize][..], &[100, 37, 256]] {
        for (session, device) in [(48_000u32, 44_100u32), (44_100, 48_000)] {
            let at = format!("{session} on {device}, callbacks {sizes:?}");
            let (ideal, term) = (u64::from(session / 50), u64::from(converter_term(session, device)));
            let run = measure(session, device, sizes);
            assert_eq!(run.stamps.len(), external_probe::NOTES + 1, "{at}: the warm-up and every measured note were stamped");
            for (p, pumped) in &run.stamps {
                // The frame clock is in effect: each stamp lies near the callback that pushed it.
                assert!((p - *pumped as f64).abs() < 600.0, "{at}: stamp at device frame {p} pushed by frame {pumped}");
            }
            let expected = reference(session, device, ideal + term);
            let (early, late) = (reference(session, device, ideal + term - 1), reference(session, device, ideal + term + 1));
            assert_eq!((early.frames + 1, late.frames - 1), (expected.frames, expected.frames), "{at}: the reference moves with T");
            assert_eq!(run.m.latencies, expected.latencies, "{at}: each note against the reference for ideal {ideal} + T {term}");
            assert_eq!((run.m.frames, run.m.found, run.m.sent), (expected.frames, 8, 8), "{at}");
            assert_eq!(run.m.spread, 0.0, "{at}");
            assert!(run.m.term, "{at}: term provenance");
            // The detector reads this pulse 4 plan frames before its centre at both pairs: the bias
            // is verified equal at both rates (ADR 0015 amendment, Acceptance), so the figure is
            // the ideal round trip plus exactly T, less that same bias.
            assert_eq!(i64::from(run.m.frames), (ideal + term) as i64 - 4, "{at}: ideal {ideal} + T {term} - the detector's 4");
        }
    }
}

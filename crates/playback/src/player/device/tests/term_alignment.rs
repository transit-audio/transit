//! #709 through the fake device (ADR 0015, Amendment 2026-11-16 on the converter term): a
//! measurement at one device rate applied at another gives what a measurement at the running rate
//! gives, at 44.1 and 48 kHz both ways; speed changes invalidate the alignment until a converter
//! restart at 1x (rule 10); separate streams get no term; and the device rate Measure compares
//! against is the backend's live figure where it publishes one.

use super::converted_rate::{WARM, delay, extra, measure_on};
use super::*;
use transit_model::{Alignment, HardwareLatency, converter_term};

/// A stored record of what the probe read at `session` on `device`, as Measure stores it.
fn record(session: u32, device: u32, m: &crate::hardware_probe::Measured) -> HardwareLatency {
    HardwareLatency {
        send: "Out 1".into(),
        ret: "In 1".into(),
        round_trip_frames: m.round_trip_frames,
        rate: session,
        measured_at: 1,
        input_device: String::new(),
        input_ports: Vec::new(),
        device_rate: device,
        normalized_frames: m.term.then(|| m.round_trip_frames - converter_term(session, device)),
    }
}

/// The issue's "Done when": a loop with the same physical delay at both device rates (whole 10 ms
/// periods), measured at one device rate and used at the other, gives the figure the probe reads
/// at the running rate: the normalized figure plus the running term. Both ways, at sessions of 48
/// and 44.1 kHz on devices of 44.1 and 48 kHz, and for two callback sizes.
#[test]
fn a_measurement_at_one_device_rate_gives_the_running_rates_figure_at_another() {
    for sizes in [&[128usize][..], &[100, 37, 256]] {
        for session in [48_000u32, 44_100] {
            let (a, b) = (44_100u32, 48_000u32);
            // The same physical delay at both device rates.
            assert_eq!(delay(session, a, sizes).1, delay(session, b, sizes).1, "{sizes:?}");
            let (_, at_a) = extra(session, a, sizes, WARM, None);
            let (_, at_b) = extra(session, b, sizes, WARM, None);
            assert!(at_a.m.term && at_b.m.term, "{session}, {sizes:?}: both runs had the term in force");
            let (fig_a, fig_b) = (record(session, a, &at_a.m), record(session, b, &at_b.m));
            let at = format!("session {session}, callbacks {sizes:?}");
            assert_eq!(fig_a.aligned_frames(Some(b)), at_b.m.round_trip_frames, "{at}: measured at {a}, used at {b}");
            assert_eq!(fig_b.aligned_frames(Some(a)), at_a.m.round_trip_frames, "{at}: measured at {b}, used at {a}");
            // Where the term is not in force the measured total is used as it is.
            assert_eq!((fig_a.round_trip_frames, fig_b.round_trip_frames), (at_a.m.round_trip_frames, at_b.m.round_trip_frames));
            // And the two readings differ by exactly the two terms.
            let difference = i64::from(at_a.m.round_trip_frames) - i64::from(at_b.m.round_trip_frames);
            assert_eq!(difference, i64::from(converter_term(session, a)) - i64::from(converter_term(session, b)), "{at}");
        }
    }
}

/// Separate input and output streams get no term (rule 1's stated scope): the probe's figure has no
/// term provenance and the plan's input reports no alignment.
#[test]
fn separate_streams_get_no_term() {
    let sizes = &[128usize][..];
    let (delay, _) = delay(48_000, 44_100, sizes);
    let p = measure_on(false, 48_000, 44_100, delay, sizes, WARM, None);
    assert!(!p.m.term, "no term provenance on separate streams");
    assert_eq!(p.alignment, Alignment::None);
}

/// A paired fake device at `device` under a `session` plan, its input feeding the plan, with
/// `pump(n)` running n lockstep callbacks of 128 frames.
struct Rig {
    player: Player,
    fake: ab::fake::FakeBackend,
    _recorder: crate::record::Recorder,
    format: ab::Format,
}

impl Rig {
    fn new(session: u32, device: u32) -> Self {
        let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
        let fake = ab::fake::FakeBackend::new(format).with_capture(2, true);
        let doc = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
        let player = Player::open_native_or_cpal(Arc::new(doc), &AudioConfig::default(), &fake, &fake_factory(false)).unwrap();
        let recorder = player.open_input(&AudioConfig::default()).expect("the duplex input");
        player.set_input(Arc::clone(&recorder.monitor)).unwrap();
        Rig { player, fake, _recorder: recorder, format }
    }

    fn pump(&self, callbacks: usize) {
        use ab::fake::{SliceSink, SliceSource};
        let (mut output, input) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        for _ in 0..callbacks {
            assert!(self.fake.pump_capture(128, ab::Stamp::INVALID, self.format, &mut SliceSource { data: &input, channels: 2 }));
            assert!(self.fake.pump_render(128, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 }));
            self.player.service().unwrap();
        }
    }
}

/// Rule 10: speed 1 → 2 → 1 invalidates the alignment, and returning to 1x alone does not restore
/// it; a stop at 1x (a converter restart) does, and so does a locate. While it is invalid the plan's
/// hardware inserts and External Instruments use their measured totals (the block is not aligned).
#[test]
fn leaving_speed_1_invalidates_the_term_until_a_converter_restart_at_1x() {
    for (session, device) in [(48_000, 44_100), (48_000, 48_000)] {
        let at = format!("{session} on {device}");
        let rig = Rig::new(session, device);
        rig.pump(20);
        assert_eq!(rig.player.input_alignment(), Alignment::Valid, "{at}");
        rig.player.set_speed(2.0);
        rig.pump(10);
        assert_eq!(rig.player.input_alignment(), Alignment::Invalid, "{at}: at 2x");
        rig.player.set_speed(1.0);
        rig.pump(10);
        assert_eq!(rig.player.input_alignment(), Alignment::Invalid, "{at}: back at 1x, no restart yet");
        rig.player.stop().unwrap();
        rig.pump(10);
        assert_eq!(rig.player.input_alignment(), Alignment::Valid, "{at}: a stop at 1x restores it");
        // A stop at another speed restores nothing.
        rig.player.set_speed(0.5);
        rig.pump(5);
        rig.player.stop().unwrap();
        rig.pump(10);
        assert_eq!(rig.player.input_alignment(), Alignment::Invalid, "{at}: stopped at 0.5x");
        rig.player.set_speed(1.0);
        rig.pump(5);
        assert_eq!(rig.player.input_alignment(), Alignment::Invalid, "{at}");
        rig.player.locate(0).unwrap();
        rig.pump(10);
        assert_eq!(rig.player.input_alignment(), Alignment::Valid, "{at}: a locate at 1x restores it");
    }
}

/// Ruling 4: the rate Measure compares at the end of a run is the backend's live figure where it
/// publishes one (a JACK server whose rate changed before the reopen), else the rate the device
/// opened at.
#[test]
fn the_running_device_rate_follows_the_backends_live_rate() {
    let rig = Rig::new(48_000, 48_000);
    assert_eq!(rig.player.running_device_rate(), 48_000, "no live figure: the opened rate");
    rig.fake.set_live_rate(Some(44_100));
    assert_eq!(rig.player.running_device_rate(), 44_100, "the server now runs 44.1 kHz");
    assert_eq!(rig.player.device_rate, 48_000, "the open-time rate is unchanged");
    rig.fake.set_live_rate(None);
    assert_eq!(rig.player.running_device_rate(), 48_000);
}

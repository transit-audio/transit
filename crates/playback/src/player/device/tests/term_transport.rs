//! #709 at a converted rate through the fake device (ADR 0015, Amendment 2026-11-16 on the
//! converter term, the acceptance list's "loop wraps inside callbacks and count-in ends"): the live
//! input stays at rule 3's positions, which follow the output converter's coordinates, not the
//! timeline. A monitored input carries an impulse every 100 ms, a whole number of the pair's
//! rational periods, so with the positions unchanged every impulse leaves the same output samples,
//! bit for bit. A loop wrap or a count-in end that moved the input, by any fraction of a frame,
//! would change the windows after it.

use super::*;
use ab::fake::{SliceSink, SliceSource};
use transit_model::{Alignment, ChannelFormat, TrackKind};

/// Output frames compared after each impulse's capture frame: the monitored path's delay and both
/// converters' reach, with room to spare.
const WINDOW: usize = 400;

/// The smallest device-frame period after which the pair's device and render grids line up again
/// (147 device frames at 48 kHz on 44.1 kHz, 160 at 44.1 kHz on 48 kHz).
fn unit(session: u32, device: u32) -> u64 {
    let gcd = (1..=session.min(device)).rev().find(|g| session.is_multiple_of(*g) && device.is_multiple_of(*g)).unwrap();
    u64::from(device / gcd)
}

/// A paired fake device at `device` Hz under a `session` Hz plan with one mono audio track on
/// input 1 heard through Main: monitored (`armed` false) or armed and heard while armed (the
/// default Monitor Armed Tracks), its input open and feeding the plan. The session click is off.
struct Rig {
    player: Player,
    fake: ab::fake::FakeBackend,
    recorder: crate::record::Recorder,
    track: transit_model::TrackId,
    format: ab::Format,
    /// Device frames pumped so far.
    frame: u64,
    /// Output channel 0 of every frame pumped.
    out: Vec<f32>,
    sizes: &'static [usize],
    calls: usize,
}

impl Rig {
    fn new(session: u32, device: u32, sizes: &'static [usize], armed: bool) -> Self {
        let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
        let fake = ab::fake::FakeBackend::new(format).with_capture(2, true);
        let mut s = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
        s.edit.click = false;
        let track = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mic"));
        let t = s.track_mut(track).unwrap();
        t.mixer.input_monitor = !armed;
        t.mixer.record_arm = armed;
        let cfg = AudioConfig::default();
        let player = Player::open_native_or_cpal(Arc::new(s), &cfg, &fake, &fake_factory(false)).unwrap();
        let recorder = player.open_input(&cfg).expect("the duplex input");
        player.set_input(Arc::clone(&recorder.monitor)).unwrap();
        Rig { player, fake, recorder, track, format, frame: 0, out: Vec::new(), sizes, calls: 0 }
    }

    /// One lockstep callback: an impulse of 0.5 on input 1 at each of `impulses` in it.
    fn callback(&mut self, impulses: &[u64]) {
        let n = self.sizes[self.calls % self.sizes.len()];
        self.calls += 1;
        let mut input = vec![0.0f32; n * 2];
        for &at in impulses.iter().filter(|at| (self.frame..self.frame + n as u64).contains(at)) {
            input[(at - self.frame) as usize * 2] = 0.5;
        }
        let mut output = vec![0.0f32; n * 2];
        assert!(self.fake.pump_capture(n, ab::Stamp::INVALID, self.format, &mut SliceSource { data: &input, channels: 2 }));
        assert!(self.fake.pump_render(n, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 }));
        self.out.extend(output.as_chunks::<2>().0.iter().map(|f| f[0]));
        self.frame += n as u64;
        self.player.service().unwrap();
    }

    /// Callbacks until at least `frame` device frames have been pumped.
    fn run_to(&mut self, frame: u64, impulses: &[u64]) {
        while self.frame < frame {
            self.callback(impulses);
        }
    }

    fn underruns(&self) -> u64 {
        self.recorder.monitor.telemetry().underrun_frames
    }
}

/// Every impulse's output window, from its capture frame on.
fn windows<'a>(out: &'a [f32], impulses: &[u64]) -> Vec<&'a [f32]> {
    impulses.iter().map(|&at| &out[at as usize..at as usize + WINDOW]).collect()
}

/// The index of the first window that differs from the first, bit for bit, and how.
fn first_difference(windows: &[&[f32]]) -> Option<(usize, usize, f32, f32)> {
    let bits = |w: &[f32]| w.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    let reference = bits(windows[0]);
    windows.iter().enumerate().skip(1).find_map(|(k, w)| {
        let f = bits(w).iter().zip(&reference).position(|(a, b)| a != b)?;
        Some((k, f, w[f], windows[0][f]))
    })
}

const PAIRS: [(u32, u32); 2] = [(48_000, 44_100), (44_100, 48_000)];
const SIZES: [&[usize]; 2] = [&[128], &[100, 37, 256]];

/// A loop of 10,007 session frames wraps inside callbacks again and again while the monitored
/// input carries an impulse every 100 ms: every impulse leaves the same output, the input stays
/// aligned (the term in force) and never reads silence after the start.
#[test]
fn a_loop_wrap_inside_a_callback_at_a_converted_rate_leaves_the_live_input_at_its_positions() {
    for sizes in SIZES {
        for (session, device) in PAIRS {
            let at = format!("{session} on {device}, callbacks {sizes:?}");
            let mut r = Rig::new(session, device, sizes, false);
            r.player.play(0, None, Some(transit_time::Range { start: 0, end: 10_007 })).unwrap();
            let period = u64::from(device) / 10;
            assert_eq!(period % unit(session, device), 0, "{at}");
            let first = u64::from(device) / 2;
            let impulses: Vec<u64> = (0..30).map(|m| first + m * period).collect();
            r.run_to(first, &impulses);
            let silent = r.underruns();
            let (mut wraps, mut last) = (0, r.player.position());
            while r.frame < impulses[29] + period {
                r.callback(&impulses);
                let now = r.player.position();
                wraps += usize::from(now < last);
                last = now;
            }
            assert!(r.player.is_playing(), "{at}");
            assert!(wraps >= 10, "{at}: {wraps} loop wraps");
            let w = windows(&r.out, &impulses);
            assert!(w[0].iter().any(|x| x.abs() > 0.1), "{at}: the impulse is heard in its window");
            assert_eq!(first_difference(&w), None, "{at}: (impulse, frame, got, first impulse's)");
            assert_eq!(r.player.input_alignment(), Alignment::Valid, "{at}");
            assert_eq!(r.underruns() - silent, 0, "{at}: silence read after the start");
        }
    }
}

/// A record count-in of one bar (four half-second beats) at a converted rate, the armed track's
/// input heard throughout: the impulses between the count-in's clicks and those after its end,
/// one of them captured just before the hand-over and one just after it, all leave the same output.
/// The input stays aligned (the term in force) and never reads silence.
#[test]
fn a_count_in_end_at_a_converted_rate_leaves_the_live_input_at_its_positions() {
    for sizes in SIZES {
        for (session, device) in PAIRS {
            let at = format!("{session} on {device}, callbacks {sizes:?}");
            let mut r = Rig::new(session, device, sizes, true);
            let dir = transit_test_support::TestDir::new("term-count-in");
            std::fs::create_dir_all(&dir).unwrap();
            let take = vec![(r.track, vec![0], dir.join("take.wav"))];
            r.player.prepare_recording(&mut r.recorder, take).unwrap();
            r.run_to(u64::from(device) / 4, &[]);
            let beat = i64::from(session / 2);
            let count_in = crate::player::audio::CountIn { frames: 4 * beat, beat, per_bar: 4 };
            r.player.record_with_count_in(&r.recorder, Some(0), None, None, Some(count_in)).unwrap();
            // The count-in's first click on the output (the input is silent so far).
            let issued = r.out.len();
            while !r.out[issued..].iter().any(|x| *x != 0.0) {
                assert!(r.frame < u64::from(device) * 2, "{at}: no count-in click");
                r.callback(&[]);
            }
            let c0 = (issued + r.out[issued..].iter().position(|x| *x != 0.0).unwrap()) as u64;
            // Beats in device frames, the count-in's end, and the impulses: 50 ms into each 100 ms
            // (clear of each 30 ms click), and one rational period either side of the end.
            let (unit, beat_dev) = (unit(session, device), u64::from(device) / 2);
            let end = c0 + 4 * beat_dev;
            let (period, offset) = (u64::from(device) / 10, u64::from(device) / 20);
            assert_eq!((period % unit, offset % unit, beat_dev % unit), (0, 0, 0), "{at}");
            let mut impulses: Vec<u64> = (0..30).map(|m| c0 + offset + m * period).collect();
            impulses.extend([end - unit, end + 3 * unit]);
            impulses.sort_unstable();
            let silent = r.underruns();
            r.run_to(impulses.last().unwrap() + period, &impulses);
            assert!(r.player.count_in().is_none() && r.player.is_playing(), "{at}: the count-in ended and the pass runs");
            let w = windows(&r.out, &impulses);
            assert!(w[0].iter().any(|x| x.abs() > 0.1), "{at}: the impulse is heard during the count-in");
            assert_eq!(first_difference(&w), None, "{at}: (impulse, frame, got, first impulse's)");
            assert_eq!(r.player.input_alignment(), Alignment::Valid, "{at}");
            assert_eq!(r.underruns() - silent, 0, "{at}: silence read after the count-in started");
            r.player.stop().unwrap();
            r.run_to(r.frame + 1_024, &[]);
            let _ = r.recorder.finish();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

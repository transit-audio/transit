#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::event::NoteId;

const SR: f32 = 48_000.0;

/// A Filter with `params`, prepared at 48 kHz stereo.
fn filter(params: &[(&str, f32)]) -> Filter {
    let mut f = Filter::new();
    for (id, v) in params {
        assert!(f.set_param(id, *v), "{id}");
    }
    f.prepare(SR, 512, 2);
    f
}

fn ev(message: Message) -> PluginEvent {
    PluginEvent { channel: 0, note_id: -1, message }
}

fn on(key: u8, velocity: u8) -> PluginEvent {
    ev(Message::NoteOn { key, velocity, id: NoteId::NONE })
}

fn off(key: u8) -> PluginEvent {
    ev(Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
}

fn pedal(down: bool) -> PluginEvent {
    ev(Message::Control { cc: CC_SUSTAIN, value: if down { 127 } else { 0 } })
}

/// Renders `input` (mono, copied to both channels) in host blocks of `block`, giving each event
/// (absolute frame, event) in the block it falls in at its offset there. Returns the left channel.
fn render(f: &mut Filter, input: &[f32], block: usize, events: &[(usize, PluginEvent)]) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    let mut io = vec![vec![0.0; block]; 2];
    let mut start = 0;
    while start < input.len() {
        let n = block.min(input.len() - start);
        for (at, e) in events.iter().filter(|(at, _)| (start..start + n).contains(at)) {
            assert!(f.event(u32::try_from(at - start).unwrap(), e));
        }
        for c in &mut io {
            c[..n].copy_from_slice(&input[start..start + n]);
        }
        f.process(&mut io, n);
        assert_eq!(io[0][..n], io[1][..n], "stereo-linked: the same input gives the same output on both sides");
        out.extend_from_slice(&io[0][..n]);
        start += n;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Closed-form response

/// A complex number, enough for the transfer function.
#[derive(Clone, Copy)]
struct C(f64, f64);

impl C {
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn scale(self, k: f64) -> C {
        C(self.0 * k, self.1 * k)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// The digital transfer function of one section, written independently of the filter's code: the
/// analog prototype at `s = (z - 1) / (z + 1) / g`, `z = e^(jω)`.
fn section(mode: Mode, fc: f64, q: f64, f: f64) -> C {
    let g = (std::f64::consts::PI * fc / f64::from(SR)).tan();
    let w = std::f64::consts::TAU * f / f64::from(SR);
    let z = C(w.cos(), w.sin());
    let s = z.add(C(-1.0, 0.0)).div(z.add(C(1.0, 0.0))).scale(1.0 / g);
    let k = 1.0 / q;
    let s2 = s.mul(s);
    let d = s2.add(s.scale(k)).add(C(1.0, 0.0));
    let n = match mode {
        Mode::Low => C(1.0, 0.0),
        Mode::High => s2,
        Mode::Band => s.scale(k),
        Mode::Notch => s2.add(C(1.0, 0.0)),
    };
    n.div(d)
}

fn analytic(mode: Mode, slope24: bool, fc: f64, q: f64, f: f64) -> f64 {
    let h = section(mode, fc, q, f);
    // The second section is a Butterworth one: Q = 1/sqrt(2).
    let h = if slope24 { h.mul(section(mode, fc, std::f64::consts::FRAC_1_SQRT_2, f)) } else { h };
    h.abs()
}

/// |DFT| of `x` at frequency `f` (f64 accumulation).
fn dft(x: &[f32], f: f64) -> f64 {
    let w = std::f64::consts::TAU * f / f64::from(SR);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, v) in x.iter().enumerate() {
        let a = w * n as f64;
        re += f64::from(*v) * a.cos();
        im -= f64::from(*v) * a.sin();
    }
    re.hypot(im)
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Tolerance of the closed-form test: within 0.05 dB of the analytic magnitude where it is above
/// -60 dB, else within 1e-4 linear (-80 dB) of it, where f32 rounding in the integrators dominates.
fn close(measured: f64, want: f64) -> bool {
    if want > 1e-3 { (db(measured) - db(want)).abs() <= 0.05 } else { (measured - want).abs() <= 1e-4 }
}

#[test]
fn every_mode_and_slope_matches_the_analytic_transfer_function() {
    let modes = [Mode::Low, Mode::High, Mode::Band, Mode::Notch];
    for (mi, mode) in modes.iter().enumerate() {
        for slope in [0.0, 1.0] {
            for resonance in [0.0, 50.0, 100.0] {
                for fc in [200.0f32, 2000.0, 9000.0] {
                    let mut f = filter(&[("mode", mi as f32), ("slope", slope), ("resonance", resonance), ("cutoff", fc)]);
                    let mut x = vec![0.0f32; 1 << 16];
                    x[0] = 1.0;
                    let h = render(&mut f, &x, 512, &[]);
                    let q = f64::from(q_of(resonance));
                    let fc = f64::from(fc);
                    for probe in [40.0, 0.5 * fc, 0.9 * fc, fc, 1.1 * fc, 2.0 * fc, 4.0 * fc, 15_000.0, 21_000.0] {
                        let probe = probe.min(23_000.0);
                        let want = analytic(*mode, slope > 0.5, fc, q, probe);
                        let got = dft(&h, probe);
                        assert!(
                            close(got, want),
                            "{mode:?} slope {slope} res {resonance} fc {fc} at {probe:.1} Hz: measured {:.4} dB, analytic {:.4} dB",
                            db(got),
                            db(want)
                        );
                        // The panel's curve is the same function.
                        let curve = f64::from(
                            filter_response(
                                &[("mode", mi as f32), ("slope", slope), ("resonance", resonance), ("cutoff", fc as f32)],
                                &[probe as f32],
                                SR,
                            )[0],
                        );
                        assert!(want < 1e-4 || (curve - db(want)).abs() < 1e-3, "curve {curve} vs {}", db(want));
                    }
                }
            }
        }
    }
}

#[test]
fn the_drive_is_linear_to_full_scale_and_bounded_beyond() {
    for x in [0.0f32, 0.25, -0.5, 1.0, -1.0] {
        assert_eq!(soft_clip(x), x);
    }
    assert!((soft_clip(1.5) - 1.375).abs() < 1e-6);
    assert_eq!(soft_clip(2.0), 1.5);
    assert_eq!(soft_clip(-100.0), -1.5);
    // Continuous slope at the knee.
    let d = (soft_clip(1.0 + 1e-3) - soft_clip(1.0)) / 1e-3;
    assert!((d - 1.0).abs() < 1e-2, "{d}");
}

// ---------------------------------------------------------------------------------------------
// Envelopes and timing (in frames, never wall-clock)

/// A DC input the 20 kHz low pass passes at unity once settled: the output is the amp envelope.
fn dc(len: usize) -> Vec<f32> {
    vec![0.5; len]
}

fn amp_patch(extra: &[(&str, f32)]) -> Filter {
    let mut p = vec![
        ("cutoff", 20_000.0),
        ("amp_on", 1.0),
        ("amp_attack", 1.0),
        ("amp_decay", 50.0),
        ("amp_sustain", 100.0),
        ("amp_release", 10.0),
        ("amp_velocity", 0.0),
    ];
    p.extend_from_slice(extra);
    filter(&p)
}

#[test]
fn the_amp_envelope_starts_at_the_notes_frame_and_times_in_frames() {
    let mut f = amp_patch(&[]);
    // Settle the filter on the DC (the amp stage silences it meanwhile).
    let pre = render(&mut f, &dc(9600), 256, &[]);
    assert!(pre.iter().all(|v| *v == 0.0), "no note: the amp stage is closed");
    let (start, stop) = (37usize, 4037usize);
    let y = render(&mut f, &dc(9600), 256, &[(start, on(60, 100)), (stop, off(60))]);
    assert!(y[..start].iter().all(|v| *v == 0.0), "silent before the note's frame");
    assert!(y[start] > 0.0, "the envelope starts at the note's frame");
    // The 1 ms attack is 48 frames: frame i of the note is at (i + 1)/48 of full level, then full.
    for i in 0..100 {
        let want = 0.5 * ((i + 1) as f32 / 48.0).min(1.0);
        assert!((y[start + i] - want).abs() < 1e-5, "attack frame {i}: {} vs {want}", y[start + i]);
    }
    // Release: -60 dB after 10 ms (480 frames) from the sustain level.
    assert!((y[stop - 1] - 0.5).abs() < 1e-5);
    let r = y[stop + 479] / 0.5;
    assert!((0.0009..0.0011).contains(&r), "-60 dB at the release time: {r}");
}

#[test]
fn the_filter_envelope_moves_the_cutoff_from_the_notes_frame_only() {
    let noise: Vec<f32> = (0..8192u32).map(|i| ((i.wrapping_mul(2_654_435_761) >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.5).collect();
    let patch = [("cutoff", 150.0), ("env_amount", 100.0), ("env_attack", 2.0), ("slope", 1.0)];
    let mut plain = filter(&patch);
    let mut played = filter(&patch);
    let a = render(&mut plain, &noise, 128, &[]);
    let at = 2001usize;
    let b = render(&mut played, &noise, 128, &[(at, on(64, 127))]);
    let first = a.iter().zip(&b).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(first, Some(at), "the cutoff moves at the note's frame, not before or a control period later");
}

#[test]
fn paraphonic_every_note_retriggers_for_all_and_legato_does_not() {
    for (mode, rises) in [(0.0, true), (1.0, false)] {
        let mut f = amp_patch(&[("amp_sustain", 50.0), ("retrigger", mode)]);
        render(&mut f, &dc(4800), 256, &[]);
        let y = render(&mut f, &dc(9600), 256, &[(10, on(60, 100)), (3000, on(64, 100)), (6000, off(60)), (8000, off(64))]);
        assert!((y[2999] - 0.25).abs() < 1e-4, "at the 50% sustain: {}", y[2999]);
        let peak = y[3000..3200].iter().copied().fold(0.0f32, f32::max);
        assert_eq!(peak > 0.3, rises, "retrigger {mode}: second note peak {peak}");
        // One held note keeps the gate open; the last one lets go.
        assert!(y[7999] > 0.2, "the gate holds while a note is held: {}", y[7999]);
        assert!(y[8000 + 479] < 0.001, "released with the last note: {}", y[8479]);
    }
}

#[test]
fn the_sustain_pedal_holds_the_gate_until_it_comes_up() {
    let mut f = amp_patch(&[]);
    render(&mut f, &dc(4800), 256, &[]);
    let y = render(&mut f, &dc(9600), 256, &[(0, pedal(true)), (10, on(60, 100)), (1000, off(60)), (5000, pedal(false))]);
    assert!((y[4999] - 0.5).abs() < 1e-5, "sustained: {}", y[4999]);
    assert!(y[5000 + 479] < 0.001, "released at the pedal: {}", y[5479]);
}

#[test]
fn all_notes_off_releases_at_once_and_drops_queued_events() {
    let mut f = amp_patch(&[]);
    render(&mut f, &dc(4800), 256, &[]);
    let y = render(&mut f, &dc(2000), 256, &[(0, on(60, 100))]);
    assert!((y[1999] - 0.5).abs() < 1e-5);
    f.event(5, &on(62, 100));
    f.all_notes_off();
    let y = render(&mut f, &dc(2000), 256, &[]);
    assert!(y[479] < 0.001, "released: {}", y[479]);
    assert!(y[1999] == 0.0, "the queued note-on was dropped");
}

#[test]
fn velocity_scales_the_amp_envelope_by_its_amount() {
    let mut f = amp_patch(&[("amp_velocity", 100.0)]);
    render(&mut f, &dc(4800), 256, &[]);
    let y = render(&mut f, &dc(4800), 256, &[(0, on(60, 64))]);
    assert!((y[4799] - 0.5 * 64.0 / 127.0).abs() < 1e-5, "{}", y[4799]);
}

#[test]
fn key_tracking_follows_the_last_note_an_octave_per_octave() {
    // A sine at 1 kHz through a 1 kHz band pass: an octave of key tracking moves the cutoff off it.
    let sine: Vec<f32> = (0..9600).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / SR).sin() * 0.5).collect();
    let level = |key: u8, track: f32| {
        let mut f = filter(&[("mode", 2.0), ("cutoff", 1000.0), ("resonance", 80.0), ("key_track", track)]);
        let y = render(&mut f, &sine, 256, &[(0, on(key, 100))]);
        y[4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    };
    assert!((level(60, 100.0) - 0.5).abs() < 0.01, "middle C leaves the cutoff where it is");
    assert!(level(72, 100.0) < 0.1, "an octave up moves it an octave");
    assert!((level(72, 0.0) - 0.5).abs() < 0.01, "no tracking at 0%");
}

#[test]
fn the_follower_opens_a_low_pass_on_a_loud_input() {
    let sine: Vec<f32> = (0..9600).map(|i| (std::f32::consts::TAU * 2000.0 * i as f32 / SR).sin() * 0.9).collect();
    let tail = |amount: f32| {
        let mut f = filter(&[("cutoff", 200.0), ("follow_amount", amount), ("follow_attack", 1.0)]);
        let y = render(&mut f, &sine, 256, &[]);
        y[4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    };
    assert!(tail(100.0) > 4.0 * tail(0.0), "{} vs {}", tail(100.0), tail(0.0));
}

#[test]
fn a_tempo_synced_lfo_runs_at_the_division() {
    let mut f = filter(&[("lfo_sync", 1.0), ("lfo_division", 4.0), ("lfo_shape", 2.0)]);
    f.set_transport(&Transport { tempo: 120.0, numerator: 4, denominator: 4, ..Transport::default() });
    // A quarter note at 120 BPM is 0.5 s: 24 000 frames per cycle.
    assert!((f.lfo_inc - 1.0 / 24_000.0).abs() < 1e-15, "{}", f.lfo_inc);
    let mut free = filter(&[("lfo_rate", 2.0)]);
    free.set_transport(&Transport { tempo: 90.0, ..Transport::default() });
    assert!((free.lfo_inc - 2.0 / 48_000.0).abs() < 1e-15);
}

#[test]
fn every_lfo_shape_stays_bipolar_and_the_random_one_is_repeatable() {
    let mut a = Lfo::new();
    let mut b = Lfo::new();
    for shape in 0..5 {
        let s = Shape::from_index(shape);
        for _ in 0..10_000 {
            a.advance(0.0013);
            b.advance(0.0013);
            let v = a.value(s);
            assert!((-1.0..=1.0).contains(&v), "{s:?} {v}");
            assert_eq!(v.to_bits(), b.value(s).to_bits());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Host blocks, parameters, presets

/// Any host block size gives the same output bit for bit, with every modulation source on and note
/// events at odd offsets: everything runs on the plug-in's own sample clock.
#[test]
fn host_blocks_split_bit_for_bit_with_notes_at_odd_offsets() {
    let input: Vec<f32> = (0..12_000u32)
        .map(|i| {
            let t = i as f32 / SR;
            0.4 * (std::f32::consts::TAU * 110.0 * t).sin() + 0.2 * ((i.wrapping_mul(1_103_515_245) >> 9) as f32 / (1u32 << 23) as f32 - 0.5)
        })
        .collect();
    let patch = [
        ("slope", 1.0),
        ("cutoff", 400.0),
        ("resonance", 70.0),
        ("drive", 9.0),
        ("env_amount", 60.0),
        ("env_attack", 3.0),
        ("env_decay", 120.0),
        ("env_sustain", 30.0),
        ("lfo_amount", -25.0),
        ("lfo_shape", 4.0),
        ("lfo_rate", 7.0),
        ("lfo_retrigger", 1.0),
        ("follow_amount", 20.0),
        ("key_track", 50.0),
        ("amp_on", 1.0),
        ("amp_attack", 4.0),
        ("amp_sustain", 70.0),
        ("output", -3.0),
    ];
    let events = [
        (37, on(48, 100)),
        (511, on(55, 80)),
        (1000, off(48)),
        (1499, pedal(true)),
        (1601, off(55)),
        (2101, pedal(false)),
        (3001, on(67, 30)),
        (4095, on(67, 90)),
        (5003, off(67)),
        (7777, on(36, 127)),
        (9999, off(36)),
    ];
    let run = |block: usize| {
        let mut f = filter(&patch);
        render(&mut f, &input, block, &events).iter().map(|v| v.to_bits()).collect::<Vec<u32>>()
    };
    let reference = run(512);
    assert!(reference.iter().any(|b| *b != 0), "it sounds");
    for block in [64, 1, 7, 16, 100, 333, 1024] {
        assert!(run(block) == reference, "host blocks of {block} frames differ from 512-frame blocks");
    }
}

/// The automation lanes and saved sessions name parameters by id (`plugin:<slot>:<id>`): the ids
/// and their order never change. Add new ones at the end.
#[test]
fn the_parameter_ids_are_stable() {
    let ids: Vec<&str> = FILTER_INFO.params.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "mode",
            "slope",
            "cutoff",
            "resonance",
            "drive",
            "env_amount",
            "env_attack",
            "env_decay",
            "env_sustain",
            "env_release",
            "env_velocity",
            "retrigger",
            "lfo_amount",
            "lfo_shape",
            "lfo_sync",
            "lfo_rate",
            "lfo_division",
            "lfo_retrigger",
            "follow_amount",
            "follow_attack",
            "follow_release",
            "key_track",
            "amp_on",
            "amp_attack",
            "amp_decay",
            "amp_sustain",
            "amp_release",
            "amp_velocity",
            "output",
        ]
    );
    assert_eq!(crate::plugin_info("filter").map(|i| i.name), Some("Filter"));
}

#[test]
fn defaults_leave_the_signal_through_a_gentle_low_pass_with_no_notes_needed() {
    let sine: Vec<f32> = (0..4800).map(|i| (std::f32::consts::TAU * 100.0 * i as f32 / SR).sin() * 0.5).collect();
    let mut f = filter(&[]);
    let y = render(&mut f, &sine, 256, &[]);
    let peak = y[2400..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.5).abs() < 0.01, "100 Hz passes a 1 kHz low pass: {peak}");
    assert!(f.reads_track_notes());
}

#[test]
fn a_full_queue_still_takes_releases() {
    let mut f = filter(&[]);
    let mut taken = 0;
    while f.event(0, &on(60, 100)) {
        taken += 1;
    }
    assert_eq!(taken, MAX_QUEUE - MAX_QUEUE / 4);
    assert!(f.event(1, &off(60)), "a release always finds room");
    assert!(f.event(1, &ev(Message::Control { cc: CC_ALL_NOTES_OFF, value: 0 })));
    assert!(f.event(1, &ev(Message::PitchBend { value: 0 })), "what it does not read is accepted and ignored");
}

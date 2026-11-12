//! The Resonator instrument (#198 slice 3): at 48 kHz the instrument driven by notes is the crate
//! driven directly with the same strums, bit for bit; host-block independence at 44.1, 48 and
//! 96 kHz; the edge's reported latency against the delay it has; a strummed A4's pitch at every
//! session rate (E63); the accent and the matrix on the part's macros; the shaper's held-note
//! switch (#262); reset.

use super::*;
use crate::Plugin;
use std::f64::consts::TAU;

fn instrument(sr: u32, params: &[(&str, f32)]) -> ResonatorInstrument {
    let mut r = ResonatorInstrument::new();
    for (id, v) in params {
        assert!(r.set_param(id, *v), "{id}");
    }
    r.prepare(sr as f32, 512, 2);
    r
}

#[derive(Debug, Clone, Copy)]
enum Ev {
    On(u8, u8),
    Off(u8),
    Param(&'static str, f32),
    Cc(u8, u8),
    Bend(u16),
}

/// Renders `frames` in host blocks of `block`; each event is delivered in the host block holding
/// its frame, at its offset (a parameter at the block's start: script them on frames both splits
/// start a block at).
fn render(r: &mut ResonatorInstrument, frames: usize, block: usize, script: &[(usize, Ev)]) -> [Vec<f32>; 2] {
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        for (at, ev) in script.iter().filter(|(at, _)| (pos..pos + n).contains(at)) {
            let offset = at - pos;
            match *ev {
                Ev::On(key, vel) => r.note_on(offset, key, vel),
                Ev::Off(key) => r.note_off(offset, key),
                Ev::Param(id, v) => assert!(r.set_param(id, v), "{id}"),
                Ev::Cc(cc, v) => r.midi(offset, [0xB0, cc, v]),
                Ev::Bend(v) => r.midi(offset, [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]),
            }
        }
        r.process(&mut io, n);
        for c in 0..2 {
            out[c].extend_from_slice(&io[c][..n]);
        }
        pos += n;
    }
    out
}

fn same_bits(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()))
}

fn same_from(a: &[f32], b: &[f32], from: usize, to: usize) -> bool {
    a[from..to].iter().zip(&b[from..to]).all(|(x, y)| x.to_bits() == y.to_bits())
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Notes at offsets on and off the 24-frame grid, two in one block (they strum on consecutive
/// blocks), and note-offs, which the resonator does not hear.
const STRUMS: [(usize, Ev); 9] = [
    (0, Ev::On(69, 127)),
    (100, Ev::On(57, 127)),
    (900, Ev::Off(69)),
    (1000, Ev::On(64, 127)),
    (1000, Ev::On(67, 127)),
    (2400, Ev::On(72, 127)),
    (3000, Ev::Off(57)),
    (5000, Ev::On(45, 127)),
    (30_011, Ev::On(81, 127)),
];

/// The blocks the strums of [`STRUMS`] land on at 48 kHz (the first block starting at or after
/// each note's frame; the second of two notes in one block on the next block), and their keys.
const STRUM_BLOCKS: [(usize, u8); 7] = [(0, 69), (5, 57), (42, 64), (43, 67), (100, 72), (209, 45), (1251, 81)];

/// The crate driven directly in 24-frame blocks, as the instrument should drive it: the chord
/// switch on (#521), the internal exciter, the tonic C1 (MIDI 36), the note over it, FM `fm`, the chord from structure, a strum
/// at each of `strums` (block, key). Returns the two outputs.
fn crate_render(model: ResonatorModel, polyphony: usize, patch: Patch, fm: f32, strums: &[(usize, u8)], blocks: usize) -> [Vec<f32>; 2] {
    let mut part = Resonator::new();
    // As the instrument drives it (#521): chord notes keep their pitches.
    part.set_strum_keeps_voice_notes(true);
    part.set_model(model);
    if polyphony != 1 {
        part.set_polyphony(polyphony);
    }
    let mut key = 60u8;
    let mut out = [Vec::new(), Vec::new()];
    for b in 0..blocks {
        let strum = strums.iter().find(|(at, _)| *at == b).map(|(_, k)| *k);
        if let Some(k) = strum {
            key = k;
        }
        let performance = PerformanceState {
            strum: strum.is_some(),
            internal_exciter: true,
            tonic: 36.0,
            note: f32::from(key) - 36.0,
            fm,
            chord: chord_of(patch.structure),
            ..PerformanceState::default()
        };
        let (mut o, mut a) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        part.process(&performance, &patch, &[0.0; BLOCK], &mut o, &mut a);
        out[0].extend_from_slice(&o);
        out[1].extend_from_slice(&a);
    }
    out
}

/// At 48 kHz nothing is converted and the latency is 0: the instrument's left and right outputs
/// are the crate's two outputs, bit for bit, for every model at every polyphony, in host blocks of
/// 64 and 500 frames (level 0 dB, spread 100%, the shaper off, no matrix row; Accent at 0 and at
/// 50% with full velocity; tune through the FM input).
#[test]
fn at_48_khz_the_instrument_is_the_crate_driven_directly() {
    let blocks = 2800;
    let frames = blocks * BLOCK;
    for (m, model) in ResonatorModel::ALL.into_iter().enumerate() {
        for (p, polyphony) in [1usize, 2, 4].into_iter().enumerate() {
            let tune = if m % 2 == 0 { 0.0 } else { 3.0 };
            let accent = if p == 1 { 50.0 } else { 0.0 };
            let params = [
                ("model", m as f32),
                ("polyphony", p as f32),
                ("structure", 70.0),
                ("brightness", 60.0),
                ("damping", 70.0),
                ("position", 30.0),
                ("level", 0.0),
                ("tune", tune),
                ("accent", accent),
                ("spread", 100.0),
            ];
            let patch = Patch { structure: 0.7, brightness: 0.6, damping: 0.7, position: 0.3 };
            let want = crate_render(model, polyphony, patch, tune, &STRUM_BLOCKS, blocks);
            assert!(peak(&want[0]) > 0.01, "{model:?}: the reference is silent");
            for block in [64, 500] {
                let mut r = instrument(48_000, &params);
                assert_eq!(r.latency(), 0);
                let got = render(&mut r, frames, block, &STRUMS);
                assert!(same_bits(&got, &want), "{model:?} at polyphony {polyphony}, host block {block}: not the crate's bits");
            }
        }
    }
}

/// Velocity is the accent: a strum at velocity 64 with Accent 50% plays the crate with Brightness
/// darkened by `0.5 · (1 − 64/127)` of itself from that strum on (by nothing at full velocity).
#[test]
fn velocity_darkens_brightness_by_the_accent() {
    assert_eq!(accented(0.6, 0.5, 1.0).to_bits(), 0.6f32.to_bits());
    assert_eq!(accented(0.6, 0.0, 0.2).to_bits(), 0.6f32.to_bits());
    let v = 64.0f32 / 127.0;
    let b = accented(0.6, 0.5, v);
    assert!((b - 0.6 * (1.0 - 0.5 * (1.0 - v))).abs() < 1e-7);
    let blocks = 2000;
    let mut r = instrument(48_000, &[("polyphony", 0.0), ("brightness", 60.0), ("accent", 50.0), ("level", 0.0), ("spread", 100.0)]);
    let got = render(&mut r, blocks * BLOCK, 256, &[(0, Ev::On(60, 64))]);
    let want = crate_render(ResonatorModel::Modal, 1, Patch { brightness: b, ..Patch::default() }, 0.0, &[(0, 60)], blocks);
    assert!(same_bits(&got, &want), "the accent is Brightness darkened");
}

/// A matrix row moves the part's macros as the knobs would: Velocity to Damping at -50% at full
/// velocity plays the crate with damping lowered by 0.5, and Mod wheel to Structure at +100% with
/// the wheel at the top plays it with structure at the module's ceiling.
#[test]
fn matrix_rows_move_the_parts_macros() {
    let params = [
        ("polyphony", 0.0),
        ("level", 0.0),
        ("spread", 100.0),
        ("damping", 80.0),
        ("structure", 20.0),
        ("mod1_source", src::VELOCITY as f32),
        ("mod1_dest", D_DAMPING as f32),
        ("mod1_amount", -50.0),
        ("mod2_source", src::MOD_WHEEL as f32),
        ("mod2_dest", D_STRUCTURE as f32),
        ("mod2_amount", 100.0),
    ];
    let mut r = instrument(48_000, &params);
    let blocks = 1500;
    let got = render(&mut r, blocks * BLOCK, 128, &[(0, Ev::Cc(1, 127)), (0, Ev::On(60, 127))]);
    let patch = Patch { structure: (0.2f32 + 1.0).min(STRUCTURE_MAX), damping: 0.8f32 - 0.5, ..Patch::default() };
    let want = crate_render(ResonatorModel::Modal, 1, patch, 0.0, &[(0, 60)], blocks);
    assert!(same_bits(&got, &want), "the rows' sums reach the part");
}

/// A script with every kind of change, parameters at frames that start a host block in both
/// splits: model, polyphony, the shaper switched while keys are held and off again, the wheel
/// and the bend.
fn busy_script() -> Vec<(usize, Ev)> {
    vec![
        (0, Ev::On(60, 100)),
        (700, Ev::On(64, 90)),
        (701, Ev::On(67, 40)),
        (9000, Ev::Off(60)),
        (12_288, Ev::Param("model", 1.0)),
        (12_300, Ev::On(55, 127)),
        (20_000, Ev::Bend(12_000)),
        (24_576, Ev::Param("polyphony", 0.0)),
        (24_600, Ev::On(72, 70)),
        (30_000, Ev::Cc(1, 90)),
        (36_864, Ev::Param("shaper", 1.0)),
        (40_000, Ev::Off(64)),
        (40_001, Ev::Off(67)),
        (40_002, Ev::Off(55)),
        (40_003, Ev::Off(72)),
        (45_056, Ev::Param("model", 4.0)),
        (45_100, Ev::On(48, 127)),
        (52_000, Ev::Off(48)),
        (57_344, Ev::Param("shaper", 0.0)),
        (60_000, Ev::On(62, 127)),
        (70_000, Ev::Off(62)),
        (75_000, Ev::On(50, 127)),
    ]
}

/// The shaper's filter and envelopes, LFO 1 to Brightness, the wheel to the cutoff, LFO 2 to Pan.
const BUSY_PARAMS: [(&str, f32); 14] = [
    ("filter_mode", 0.0),
    ("cutoff", 1500.0),
    ("resonance", 40.0),
    ("filter_env_amount", 50.0),
    ("amp_release", 200.0),
    ("mod1_source", 1.0),
    ("mod1_dest", 3.0),
    ("mod1_amount", 30.0),
    ("mod2_source", 10.0),
    ("mod2_dest", 7.0),
    ("mod2_amount", 40.0),
    ("mod3_source", 2.0),
    ("mod3_dest", 14.0),
    ("mod3_amount", 60.0),
];

/// 64- and 512-frame host blocks render the same bits at 44.1, 48 and 96 kHz through every kind of
/// change (the blocks are counted in output frames, and the edge's timing is exact).
#[test]
fn host_block_size_changes_no_bit_at_every_rate() {
    for sr in [44_100u32, 48_000, 96_000] {
        let frames = sr as usize * 2;
        let mut a = instrument(sr, &BUSY_PARAMS);
        let mut b = instrument(sr, &BUSY_PARAMS);
        let ya = render(&mut a, frames, 64, &busy_script());
        let yb = render(&mut b, frames, 512, &busy_script());
        assert!(peak(&ya[0]) > 0.01 && peak(&ya[1]) > 0.01, "{sr} Hz: silent");
        assert!(same_bits(&ya, &yb), "{sr} Hz: 64- and 512-frame host blocks differ");
        assert!(ya.iter().flatten().all(|v| v.is_finite()));
    }
}

/// The greatest common divisor, for the impulse's placement.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The edge from 48 kHz delays an impulse by exactly the latency the instrument reports, at every
/// session rate (an impulse on a codec frame that falls on a session sample); the table is the one
/// the help and ADR 0033 state.
#[test]
fn the_edge_delays_an_impulse_by_its_reported_latency() {
    let kernel = Kernel::new();
    let mut table = Vec::new();
    for sr in [44_100u32, 48_000, 88_200, 96_000, 22_050, 32_000, 176_400, 192_000] {
        let (mut up, latency) = edge(&kernel, sr);
        let step = (48_000 / gcd(48_000, sr)) as usize;
        let codec_at = step * 2000usize.div_ceil(step);
        let at = codec_at * sr as usize / 48_000;
        let mut pushed = 0usize;
        let y: Vec<f32> = (0..at + 2000)
            .map(|_| {
                while !up.ready() {
                    up.push(if pushed == codec_at { [0.5, -0.5] } else { [0.0; 2] });
                    pushed += 1;
                }
                up.pull(&kernel)[0]
            })
            .collect();
        let peak_at = (0..y.len()).max_by(|&i, &j| y[i].abs().total_cmp(&y[j].abs())).unwrap_or(0);
        assert_eq!(peak_at, at + latency, "{sr} Hz");
        assert!(y[peak_at] > 0.15, "{sr} Hz: peak {}", y[peak_at]);
        if sr == 48_000 {
            assert_eq!(y[peak_at].to_bits(), 0.5f32.to_bits(), "48 kHz is a copy");
        }
        println!("{sr} Hz: latency {latency} samples ({:.3} ms)", latency as f64 * 1000.0 / f64::from(sr));
        table.push((sr, latency));
        let r = instrument(sr, &[]);
        assert_eq!(r.latency(), latency, "{sr} Hz: the instrument reports its edge");
    }
    let stated = [(44_100, 35), (48_000, 0), (88_200, 65), (96_000, 70)];
    for (sr, latency) in stated {
        assert!(table.contains(&(sr, latency)), "{sr} Hz: the stated latency {latency} is not the measured one ({table:?})");
    }
}

/// The frequency of the strongest spectral peak within 1.5% of `around_hz` and its log magnitude
/// (a Hann-windowed DFT in 0.1 Hz steps, refined by a parabola), as the Time Stretcher's and the
/// Texture's pitch tests measure.
fn dft_peak(x: &[f64], rate: f64, around_hz: f64) -> (f64, f64) {
    let n = x.len();
    let w: Vec<f64> = x.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos())).collect();
    let mag = |f: f64| {
        let (s, c) = (TAU * f / rate).sin_cos();
        let (mut re, mut im, mut cr, mut ci) = (0.0, 0.0, 1.0f64, 0.0f64);
        for (i, v) in w.iter().enumerate() {
            re += v * cr;
            im += v * ci;
            (cr, ci) = (cr * c - ci * s, cr * s + ci * c);
            if i % 4096 == 4095 {
                let m = (cr * cr + ci * ci).sqrt();
                (cr, ci) = (cr / m, ci / m);
            }
        }
        (re * re + im * im).sqrt().max(1e-30).ln()
    };
    let step = 0.1;
    let (lo, hi) = (around_hz / 1.015, around_hz * 1.015);
    let steps = ((hi - lo) / step) as usize;
    let mags: Vec<f64> = (0..=steps).map(|k| mag(lo + k as f64 * step)).collect();
    let k = (1..steps).max_by(|a, b| mags[*a].total_cmp(&mags[*b])).unwrap_or(1);
    let (a, m, c) = (mags[k - 1], mags[k], mags[k + 1]);
    let d = 0.5 * (a - c) / (a - 2.0 * m + c);
    (lo + (k as f64 + d) * step, m - 0.25 * (a - c) * d)
}

/// The pitch near `expected_hz`, refusing an octave-down alias (the spectrum at half the pitch
/// must be 12 dB under the peak) and an octave-up one (likewise at twice the pitch).
fn pitch_hz(x: &[f64], rate: f64, expected_hz: f64, label: &str) -> f64 {
    let (hz, peak) = dft_peak(x, rate, expected_hz);
    for alias in [0.5, 2.0] {
        let (_, m) = dft_peak(x, rate, hz * alias);
        let below_db = 20.0 * (peak - m) / std::f64::consts::LN_10;
        assert!(below_db > 12.0, "{label}: the spectrum at {alias}x the pitch is only {below_db:.1} dB under the peak");
    }
    hz
}

/// A strummed A4 (MIDI 69) sounds at 440 Hz within 1 cent at 44.1, 48, 88.2 and 96 kHz (E63): the
/// part runs at its own 48 kHz behind the edge whatever the session rate. The modal model with
/// structure 0.25 (harmonic modes; the left output carries the odd ones, so nothing sits an octave
/// up) and damping 100% (no decay), one second from 0.2 s after the strum.
#[test]
fn a_strummed_a4_plays_440_hz_at_every_session_rate() {
    for sr in [44_100u32, 48_000, 88_200, 96_000] {
        let rate = f64::from(sr);
        let mut r = instrument(sr, &[("polyphony", 0.0), ("structure", 25.0), ("damping", 100.0), ("level", 0.0)]);
        let y = render(&mut r, sr as usize * 13 / 10, 256, &[(0, Ev::On(69, 127))]);
        let from = sr as usize / 5;
        let window: Vec<f64> = y[0][from..from + sr as usize].iter().map(|&v| f64::from(v)).collect();
        assert!(window.iter().any(|v| v.abs() > 0.01), "{sr} Hz: silent");
        let label = format!("{sr} Hz");
        let hz = pitch_hz(&window, rate, 440.0, &label);
        let cents = 1200.0 * (hz / 440.0).log2();
        println!("{label}: {hz:.4} Hz ({cents:+.4} cents)");
        assert!(cents.abs() < 1.0, "{label}: {hz} Hz ({cents:+.3} cents)");
    }
}

/// The measurement refuses an octave error: a tone at the expected pitch's half (with nothing at
/// the expected pitch) and one at its double both fail.
#[test]
fn the_pitch_measurement_refuses_octave_aliases() {
    let rate = 48_000.0;
    for f in [440.0, 1760.0] {
        let x: Vec<f64> = (0..24_000).map(|i| (TAU * f * i as f64 / rate).sin() + 0.2 * (TAU * 880.0 * i as f64 / rate).sin()).collect();
        let refused = std::panic::catch_unwind(|| pitch_hz(&x, rate, 880.0, "alias"));
        assert!(refused.is_err(), "{f} Hz with a weak 880 Hz partial passed as 880 Hz");
    }
}

/// The largest step between neighboring samples in `x[range]`.
fn largest_step(x: &[f32], range: std::ops::Range<usize>) -> f32 {
    x[range].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// The shaper switched on while a key is held (#262) crossfades into it over one 12-frame half
/// block, then plays exactly what a render shaped from the start plays (the filter and the
/// envelopes run either way); switched off, it crossfades back and plays the part's bits. The
/// crossfade steps by no more than the two paths do plus a twelfth of the gap between them.
#[test]
fn switching_the_shaper_while_a_key_is_held_crossfades_at_once() {
    let shaped = [("shaper", 1.0), ("cutoff", 800.0), ("resonance", 30.0), ("amp_sustain", 70.0), ("level", 0.0)];
    let mut dry = shaped;
    dry[0] = ("shaper", 0.0);
    let frames = 48_000;
    let notes = [(0, Ev::On(57, 127))];
    let always = render(&mut instrument(48_000, &shaped), frames, 256, &notes);
    let never = render(&mut instrument(48_000, &dry), frames, 256, &notes);
    // On at frame 12 288, a 24-frame block's first frame.
    let at = 12_288;
    let mut on = notes.to_vec();
    on.push((at, Ev::Param("shaper", 1.0)));
    let switched = render(&mut instrument(48_000, &dry), frames, 256, &on);
    for c in 0..2 {
        assert!(same_from(&switched[c], &never[c], 0, at), "channel {c}: before the switch, the part");
        assert!(same_from(&switched[c], &always[c], at + HALF, frames), "channel {c}: after the fade, shaped");
        let step = largest_step(&switched[c], at - 1..at + HALF + 1);
        let own = largest_step(&never[c], at - 1..at + HALF + 1).max(largest_step(&always[c], at - 1..at + HALF + 1));
        let gap = never[c][at..at + HALF].iter().zip(&always[c][at..at + HALF]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        println!("on, channel {c}: largest step {step:.5} (the paths' own {own:.5}, the gap between them {gap:.5})");
        assert!(gap > 0.001, "channel {c}: the paths differ");
        assert!(step <= own + gap / HALF as f32 + 1e-6, "channel {c}: the switch steps by {step}");
    }
    let off = 24_576;
    on.push((off, Ev::Param("shaper", 0.0)));
    let back = render(&mut instrument(48_000, &dry), frames, 256, &on);
    for c in 0..2 {
        assert!(same_from(&back[c], &never[c], off + HALF, frames), "channel {c}: after the fade back, the part");
    }
}

/// With no key held the switch waits for the next strum (the ringing part is not faded into the
/// filter mid-tail), and that strum takes it.
#[test]
fn with_no_key_held_the_shaper_switch_waits_for_the_next_strum() {
    let dry = [("shaper", 0.0), ("cutoff", 600.0), ("level", 0.0)];
    let frames = 48_000;
    let base = [(0, Ev::On(57, 127)), (2400, Ev::Off(57)), (24_000, Ev::On(64, 127))];
    let never = render(&mut instrument(48_000, &dry), frames, 256, &base);
    let mut switched = base.to_vec();
    switched.push((12_288, Ev::Param("shaper", 1.0)));
    let got = render(&mut instrument(48_000, &dry), frames, 256, &switched);
    let strum = 24_000;
    assert!(same_from(&got[0], &never[0], 0, strum), "the tail plays on unshaped");
    assert!(!same_from(&got[0], &never[0], strum + HALF, frames), "the next strum is shaped");
}

/// A shaper switched on before anything sounds applies at once: the first strum is enveloped from
/// its first frame, with no crossfade from the unshaped strike (a 2 s attack starts silent).
#[test]
fn a_shaper_set_before_the_first_strum_applies_without_a_fade() {
    let mut r = instrument(48_000, &[("level", 0.0)]);
    let script = [(0, Ev::Param("shaper", 1.0)), (0, Ev::Param("amp_attack", 2000.0)), (100, Ev::On(60, 127))];
    let y = render(&mut r, 4800, 128, &script);
    assert!(peak(&y[0][..480]) < 0.01, "a 2 s attack starts silent: {}", peak(&y[0][..480]));
    let mut plain = instrument(48_000, &[("level", 0.0)]);
    let p = render(&mut plain, 4800, 128, &[(100, Ev::On(60, 127))]);
    assert!(peak(&p[0][..480]) > 0.05, "unshaped, the strike is loud: {}", peak(&p[0][..480]));
}

/// A reset instrument renders what a new one renders, through the edge (44.1 kHz) and at 48 kHz,
/// after a busy run.
#[test]
fn a_reset_instrument_renders_what_a_new_one_renders() {
    for sr in [44_100u32, 48_000] {
        let mut used = instrument(sr, &BUSY_PARAMS);
        let _ = render(&mut used, sr as usize * 2, 300, &busy_script());
        used.reset();
        let mut fresh = instrument(sr, &BUSY_PARAMS);
        for (id, v) in [("model", 4.0), ("polyphony", 0.0), ("shaper", 0.0)] {
            assert!(used.set_param(id, v) && fresh.set_param(id, v));
        }
        let script = [(0, Ev::On(60, 100)), (5000, Ev::On(67, 127))];
        let a = render(&mut used, 24_000, 256, &script);
        let b = render(&mut fresh, 24_000, 256, &script);
        assert!(peak(&a[0]) > 0.01, "{sr} Hz: silent");
        assert!(same_bits(&a, &b), "{sr} Hz: reset is not new");
    }
}

/// The chord follows structure over the whole table, as the oracle's sweep picks it.
#[test]
fn the_chord_follows_structure() {
    assert_eq!(chord_of(0.0), 0);
    assert_eq!(chord_of(0.5), 5);
    assert_eq!(chord_of(STRUCTURE_MAX), CHORD_COUNT - 1);
    assert_eq!(chord_of(0.04), 0);
    assert_eq!(chord_of(0.06), 1);
}

/// Spread at 0% puts the module's mono mix (the two outputs summed) on both sides, at 50% each side
/// takes half of the other; a mono strip hears the mean of the two sides; hostile values keep the
/// output finite.
#[test]
fn spread_and_a_mono_strip_and_hostile_values() {
    let script = [(0, Ev::On(60, 127)), (300, Ev::On(64, 127))];
    let apart = render(&mut instrument(48_000, &[("spread", 100.0)]), 9600, 256, &script);
    for (spread, feed) in [(0.0f32, 1.0f32), (50.0, 0.5)] {
        let y = render(&mut instrument(48_000, &[("spread", spread)]), 9600, 256, &script);
        for k in 0..9600 {
            let (l, r) = (apart[0][k], apart[1][k]);
            assert!((y[0][k] - (l + feed * r)).abs() <= 1e-6 && (y[1][k] - (r + feed * l)).abs() <= 1e-6, "spread {spread} at {k}");
        }
    }
    assert!(peak(&apart[0]) > 0.01 && peak(&apart[1]) > 0.01, "two strums at polyphony 4: one on each side");
    let mut mono = ResonatorInstrument::new();
    mono.prepare(48_000.0, 256, 1);
    mono.note_on(0, 60, 127);
    let mut stereo = instrument(48_000, &[]);
    stereo.note_on(0, 60, 127);
    let (mut io1, mut io2) = (vec![vec![0.0f32; 256]], vec![vec![0.0f32; 256]; 2]);
    for _ in 0..20 {
        mono.process(&mut io1, 256);
        stereo.process(&mut io2, 256);
        for (m, (l, r)) in io1[0].iter().zip(io2[0].iter().zip(&io2[1])) {
            assert_eq!(m.to_bits(), ((l + r) * 0.5).to_bits());
        }
    }
    let mut h = instrument(48_000, &[("tune", 24.0), ("bend_range", 24.0), ("structure", 100.0), ("damping", 100.0)]);
    for (id, v) in [("brightness", f32::NAN), ("position", f32::INFINITY), ("tune", -1e9)] {
        h.set_param(id, v);
    }
    let script = [(0, Ev::On(127, 127)), (10, Ev::On(0, 127)), (20, Ev::Bend(16_383)), (5000, Ev::Bend(0))];
    let y = render(&mut h, 48_000, 512, &script);
    assert!(y.iter().flatten().all(|v| v.is_finite()), "finite");
}

/// A strummed A4 at 48 kHz, then `cc` (120 or 123) at frame 4800; returns the left channel.
fn strum_then_cc(cc: Option<u8>) -> Vec<f32> {
    let mut r = instrument(48_000, &[]);
    let mut script = vec![(0, Ev::On(69, 127))];
    script.extend(cc.map(|c| (4800, Ev::Cc(c, 0))));
    let [l, _] = render(&mut r, 24_000, 512, &script);
    l
}

#[test]
fn all_sound_off_silences_the_ringing_part_at_once() {
    let on = strum_then_cc(None);
    let off = strum_then_cc(Some(120));
    assert!(peak(&on[4800..9600]) > 0.01, "the strum rings without the panic");
    assert!(off[..4800].iter().zip(&on).all(|(a, b)| a.to_bits() == b.to_bits()), "nothing changes before the CC");
    // The panic takes effect at the next 24-frame block.
    assert!(peak(&off[4800 + 48..]) == 0.0, "silent from two blocks on: peak {}", peak(&off[4800 + 48..]));
}

#[test]
fn all_notes_off_lets_the_part_ring_down() {
    let on = strum_then_cc(None);
    let off = strum_then_cc(Some(123));
    assert!(same_from(&on, &off, 0, on.len()), "All Notes Off leaves the unshaped part's sound alone");
    assert!(peak(&off[4800 + 48..9600]) > 0.01, "and it keeps ringing");
}

/// #292: a note-off pairs with the oldest open note-on of its key. A re-strike before the earlier
/// note's off (one sample before, or on the same sample), then that off: the key stays down until
/// the re-struck note's own off. Swept over host block sizes.
#[test]
fn an_earlier_notes_off_after_a_restrike_keeps_the_key_down() {
    for host in [1usize, 7, 12, 13, 256] {
        for (on, off) in [(5usize, 6usize), (6, 6)] {
            let mut r = instrument(48_000, &[]);
            let mut io = [vec![0.0; host], vec![0.0; host]];
            let mut run = |r: &mut ResonatorInstrument, frames: usize| {
                for at in (0..frames).step_by(host) {
                    r.process(&mut io, host.min(frames - at));
                }
            };
            r.note_on(0, 60, 100);
            run(&mut r, 480);
            r.note_on(on, 60, 91);
            r.note_off(off, 60);
            run(&mut r, 480);
            assert!(r.held[60], "host={host} on={on} off={off}: the re-struck key is down");
            r.note_off(0, 60);
            run(&mut r, 48);
            assert!(!r.held[60], "host={host} on={on} off={off}: its own off lets it go");
        }
    }
}

/// MIDI note `n`'s frequency (A4 = 440 Hz).
fn note_hz(n: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(n) - 69.0) / 12.0)
}

/// The two outputs summed over `len` frames from `from`, as f64.
fn mono(y: &[Vec<f32>; 2], from: usize, len: usize) -> Vec<f64> {
    (from..from + len).map(|i| f64::from(y[0][i]) + f64::from(y[1][i])).collect()
}

/// The strongest spectral peak within 1.5% of `hz` and how far it stands above the spectrum at
/// the window's two edges (dB): a voice ringing at `hz` stands well clear, leakage does not.
fn partial(x: &[f64], hz: f64) -> (f64, f64) {
    let (f, m) = dft_peak(x, 48_000.0, hz);
    let (_, lo) = dft_peak(x, 48_000.0, hz / 1.03);
    let (_, hi) = dft_peak(x, 48_000.0, hz * 1.03);
    let edge = |e: f64| 20.0 * (m - e) / std::f64::consts::LN_10;
    (f, edge(lo).min(edge(hi)))
}

/// #521: each note of `script` (default Resonator, Polyphony 4, 48 kHz) rings at its own pitch in
/// the second after frame `from`: each fundamental measured from the rendered audio (a spectral
/// peak within 1.5% of the note's frequency, standing at least 20 dB clear of its neighborhood
/// and within 2 cents of the note), not from the part's note values. Returns the measurements
/// (note, Hz, cents, dB clear) for the report; asserts only when `check`.
fn chord_pitches(script: &[(usize, Ev)], notes: &[u8], from: usize, check: bool) -> Vec<(u8, f64, f64, f64)> {
    let mut r = instrument(48_000, &[]);
    let y = render(&mut r, from + 48_000, 256, script);
    let x = mono(&y, from, 48_000);
    let mut got = Vec::new();
    for &n in notes {
        let (hz, clear) = partial(&x, note_hz(n));
        let cents = 1200.0 * (hz / note_hz(n)).log2();
        println!("note {n}: {hz:.3} Hz ({cents:+.2} cents), {clear:.1} dB clear");
        got.push((n, hz, cents, clear));
        if check {
            assert!(clear > 20.0 && cents.abs() < 2.0, "note {n}: {hz:.3} Hz ({cents:+.2} cents), {clear:.1} dB clear");
        }
    }
    got
}

/// The measurement itself: each note alone on a fresh instance rings at its pitch, and a note
/// that is not playing does not pass (note 62's neighborhood with only note 48 sounding).
#[test]
fn the_chord_measurement_hears_single_notes_and_refuses_absent_ones() {
    for n in [48u8, 60, 62, 64, 65, 67, 69, 71] {
        chord_pitches(&[(0, Ev::On(n, 100))], &[n], 4800, true);
    }
    let absent = chord_pitches(&[(0, Ev::On(48, 100))], &[62, 65, 69], 4800, false);
    assert!(absent.iter().all(|(_, _, c, d)| *d < 20.0 || c.abs() >= 2.0), "a silent note passed: {absent:?}");
}

/// #521, the issue's case: note 48 held for 2,400 frames, then notes 62, 65 and 69 in the same
/// block. The adapter strums them on three consecutive 24-frame blocks; before the fix the part
/// retuned each just-plucked voice one block later to the note held before the chord.
#[test]
fn a_chord_after_a_held_note_rings_at_its_own_pitches() {
    let script = [(0, Ev::On(48, 100)), (2400, Ev::On(62, 100)), (2400, Ev::On(65, 100)), (2400, Ev::On(69, 100))];
    chord_pitches(&script, &[62, 65, 69], 2400 + 4800, true);
}

/// Two notes one block apart, and a four-note chord on a fresh instance (whose delayed note is
/// still the power-on zero, the tonic).
#[test]
fn notes_one_block_apart_and_a_fresh_four_note_chord_ring_at_their_pitches() {
    chord_pitches(&[(0, Ev::On(60, 100)), (24, Ev::On(64, 100))], &[60, 64], 4800, true);
    let chord = [(0, Ev::On(60, 100)), (0, Ev::On(64, 100)), (0, Ev::On(67, 100)), (0, Ev::On(71, 100))];
    chord_pitches(&chord, &[60, 64, 67, 71], 4800, true);
}

/// Notes 5 ms apart (240 frames, ten blocks) were already right before the fix: the boundary.
#[test]
fn notes_five_ms_apart_ring_at_their_pitches() {
    let script = [(0, Ev::On(48, 100)), (2400, Ev::On(62, 100)), (2640, Ev::On(65, 100)), (2880, Ev::On(69, 100))];
    chord_pitches(&script, &[62, 65, 69], 2880 + 4800, true);
}

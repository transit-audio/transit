//! The Texture Synth instrument (#241 slice 3): at 48 kHz the instrument driven by notes is the
//! crate's synth driven directly with the same SEED gate and PITCH CV, bit for bit, at every
//! quality; host-block independence at 44.1, 48 and 96 kHz; the edge's reported latency against
//! the delay it has; an A4's pitch at every session rate (E63); velocity, the matrix and the
//! shaper's held-note switch (#262); CC 120 and CC 123; reset; the frozen state's round trip.

use super::*;
use crate::Plugin;
use std::f64::consts::TAU;
use transit_texture::stmlib::extract_gate_flags as flags_after;

fn instrument(sr: u32, params: &[(&str, f32)]) -> TextureSynth {
    let mut t = TextureSynth::new();
    for (id, v) in params {
        assert!(t.set_param(id, *v), "{id}");
    }
    t.prepare(sr as f32, 512, 2);
    t
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
fn render(t: &mut TextureSynth, frames: usize, block: usize, script: &[(usize, Ev)]) -> [Vec<f32>; 2] {
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        for (at, ev) in script.iter().filter(|(at, _)| (pos..pos + n).contains(at)) {
            let offset = at - pos;
            match *ev {
                Ev::On(key, vel) => t.note_on(offset, key, vel),
                Ev::Off(key) => t.note_off(offset, key),
                Ev::Param(id, v) => assert!(t.set_param(id, v), "{id}"),
                Ev::Cc(cc, v) => t.midi(offset, [0xB0, cc, v]),
                Ev::Bend(v) => t.midi(offset, [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]),
            }
        }
        t.process(&mut io, n);
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

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// Notes on and off the 12-frame grid: a held note, a second key while it is held (a retrigger),
/// the second released first (legato back to the first, no retrigger), the gate falling, a note
/// off the grid, a note shorter than a block, two notes in one block, and a key released that was
/// never pressed.
const NOTES: [(usize, Ev); 14] = [
    (0, Ev::On(69, 127)),
    (1000, Ev::On(76, 127)),
    (3000, Ev::Off(76)),
    (5000, Ev::Off(69)),
    (9001, Ev::On(57, 127)),
    (20_000, Ev::Off(57)),
    (24_001, Ev::On(81, 127)),
    (24_005, Ev::Off(81)),
    (30_001, Ev::On(45, 127)),
    (30_003, Ev::On(64, 127)),
    (41_000, Ev::Off(64)),
    (41_000, Ev::Off(45)),
    (50_000, Ev::Off(99)),
    (60_007, Ev::On(60, 127)),
];

/// The crate's synth driven in 12-frame blocks as the instrument should drive it: each event takes
/// effect at the first block starting at or after it; the PITCH CV is the sounding key minus 48;
/// the SEED gate is high while a key is held (for one block at least after a note-on), low on the
/// first frame of a block whose note-on finds it high. `controls` sets the rest.
fn crate_render(controls: Controls, resolution: SynthResolution, freeze_from: Option<usize>, blocks: usize) -> [Vec<f32>; 2] {
    let mut synth = Synth::new();
    let mut held: Vec<u8> = Vec::new();
    let (mut key, mut high, mut flags) = (60u8, false, 0u8);
    let mut out = [Vec::new(), Vec::new()];
    for b in 0..blocks {
        let start = b * BLOCK;
        let lo = if b == 0 { 0 } else { start - BLOCK + 1 };
        let mut strike = false;
        for (_, ev) in NOTES.iter().filter(|(at, _)| (lo..=start).contains(at)) {
            match *ev {
                Ev::On(k, _) => {
                    held.retain(|h| *h != k);
                    held.push(k);
                    key = k;
                    strike = true;
                }
                Ev::Off(k) if held.contains(&k) => {
                    held.retain(|h| *h != k);
                    if k == key
                        && let Some(back) = held.last()
                    {
                        key = *back;
                    }
                }
                _ => {}
            }
        }
        let now = !held.is_empty() || strike;
        let retrigger = strike && high;
        let mut seed = [0u8; BLOCK];
        for (k, f) in seed.iter_mut().enumerate() {
            flags = flags_after(flags, now && !(retrigger && k == 0));
            *f = flags;
        }
        high = now;
        let mut c = controls;
        c.pitch.cv = f32::from(key) - 48.0;
        c.pitch.has_cv = true;
        let freeze = [freeze_from.is_some_and(|f| start >= f); BLOCK];
        let mut o = [[0.0f32; 2]; BLOCK];
        synth.render(&c, resolution, &freeze, Some(&seed), &mut o);
        for f in o {
            out[0].push(f[0]);
            out[1].push(f[1]);
        }
    }
    out
}

/// The instrument's default controls as the firmware reads them (no matrix, the pitch knob 0).
fn default_controls(density: f32, button: SeedButton) -> Controls {
    let knob = |base: f32, random: f32| Parameter { base, attenuverter: random, cv: 0.0, has_cv: false };
    Controls {
        time: knob(0.5, 0.0),
        size: knob(0.75, 0.0),
        shape: knob(0.5, 0.0),
        pitch: Parameter { base: 0.0, attenuverter: 0.0, cv: 0.0, has_cv: true },
        density,
        density_cv: 0.0,
        feedback: 0.25,
        dry_wet: 0.5,
        reverb: 0.3,
        seed_button: button,
        monophonic: false,
        output_gate: false,
    }
}

/// At 48 kHz nothing is converted and the latency is 0: the instrument's output is the crate's
/// synth driven directly with the same gate and CV, bit for bit, at every quality, for the regular
/// train, the taps, the random train and the latched divider, with Freeze on from 64000, in host
/// blocks of 64 and 500 frames (level 0 dB, full velocity, the shaper off, no matrix row).
#[test]
fn at_48_khz_the_instrument_is_the_crate_driven_directly() {
    let blocks = 6000;
    let frames = blocks * BLOCK;
    let cases = [(25.0f32, SeedButton::Off, false), (50.0, SeedButton::Off, true), (75.0, SeedButton::Off, false), (30.0, SeedButton::Locked, true)];
    for (q, resolution) in SynthResolution::ALL.into_iter().enumerate() {
        for (pct, button, freeze) in cases {
            let density = pct / 100.0;
            let latch = if button == SeedButton::Locked { 1.0 } else { 0.0 };
            let params = [("quality", q as f32), ("density", pct), ("dry_wet", 50.0), ("seed_latch", latch), ("level", 0.0)];
            let mut script = NOTES.to_vec();
            if freeze {
                script.push((64_000, Ev::Param("freeze", 1.0)));
            }
            let want = crate_render(default_controls(density, button), resolution, freeze.then_some(64_000), blocks);
            assert!(peak(&want[0]) > 0.01, "{resolution:?} {density}: silent reference");
            for block in [64, 500] {
                let mut t = instrument(48_000, &params);
                assert_eq!(t.latency(), 0);
                let got = render(&mut t, frames, block, &script);
                assert!(same_bits(&got, &want), "{resolution:?} density {density} {button:?} freeze {freeze}, {block}-frame blocks");
            }
        }
    }
}

/// The parameters a busy render moves.
const BUSY_PARAMS: [(&str, f32); 6] =
    [("density", 30.0), ("time_random", 40.0), ("pitch_random", -20.0), ("reverb", 50.0), ("dry_wet", 70.0), ("velocity", 80.0)];

/// Notes, controllers and parameter changes (each on a frame both 64- and 512-frame splits start a
/// block at): quality, the shaper, a matrix row, Freeze, Seed, bend and the wheel.
fn busy_script() -> Vec<(usize, Ev)> {
    let mut s = NOTES.to_vec();
    s.extend([
        (4096, Ev::Param("shaper", 1.0)),
        (8192, Ev::Param("mod1_source", 10.0)),
        (8192, Ev::Param("mod1_dest", 15.0)),
        (8192, Ev::Param("mod1_amount", 60.0)),
        (9500, Ev::Cc(1, 100)),
        (12_288, Ev::Param("quality", 1.0)),
        (16_384, Ev::Param("freeze", 1.0)),
        (18_000, Ev::Bend(12_000)),
        (28_672, Ev::Param("freeze", 0.0)),
        (32_768, Ev::Param("seed", 1.0)),
        (45_056, Ev::Param("seed", 0.0)),
        (49_152, Ev::Param("shaper", 0.0)),
        (53_248, Ev::Param("quality", 0.0)),
        (57_344, Ev::Param("seed_latch", 1.0)),
        (61_000, Ev::Cc(64, 127)),
    ]);
    s
}

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
/// session rate; the table is the one the help and ADR 0033 state.
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
        if sr == 48_000 {
            assert_eq!(y[peak_at].to_bits(), 0.5f32.to_bits(), "48 kHz is a copy");
        }
        table.push((sr, latency));
        assert_eq!(instrument(sr, &[]).latency(), latency, "{sr} Hz: the instrument reports its edge");
    }
    let stated = [(22_050, 35), (32_000, 35), (44_100, 35), (48_000, 0), (88_200, 65), (96_000, 70), (176_400, 129), (192_000, 140)];
    assert_eq!(table.iter().copied().collect::<std::collections::BTreeMap<_, _>>(), stated.into_iter().collect(), "the stated latency table");
}

/// The frequency of the strongest spectral peak within 1.5% of `around_hz` and its log magnitude
/// (a Hann-windowed DFT in 0.1 Hz steps, refined by a parabola), as the Texture's and the
/// Resonator's pitch tests measure.
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

/// The firmware's A4: the oscillator's root (130.81 Hz) 21 semitones up.
const A4_HZ: f64 = 130.81 * 3.363_585_661_014_858;

/// MIDI 69 sounds at 440 Hz within 1 cent at 44.1, 48, 88.2 and 96 kHz (E63): a single grain (all
/// wet, Density at noon so the note fires one grain, Size at the top so it lasts 4 s, Shape at
/// noon) and the oscillator (all dry), one second from 0.3 s after the note. The synth runs at its
/// own 48 kHz behind the edge whatever the session rate.
#[test]
fn an_a4_plays_440_hz_at_every_session_rate() {
    assert!((1200.0 * (A4_HZ / 440.0).log2()).abs() < 0.1, "the firmware's root is C3");
    for (what, wet) in [("grain", 100.0), ("oscillator", 0.0)] {
        for sr in [44_100u32, 48_000, 88_200, 96_000] {
            let rate = f64::from(sr);
            let params = [("dry_wet", wet), ("density", 50.0), ("size", 100.0), ("feedback", 0.0), ("time", 0.0), ("reverb", 0.0), ("level", 0.0)];
            let mut t = instrument(sr, &params);
            let y = render(&mut t, sr as usize * 14 / 10, 256, &[(0, Ev::On(69, 127))]);
            let from = sr as usize * 3 / 10;
            let window: Vec<f64> = y[0][from..from + sr as usize].iter().map(|&v| f64::from(v)).collect();
            assert!(window.iter().any(|v| v.abs() > 0.01), "{what} at {sr} Hz: silent");
            let label = format!("{what} at {sr} Hz");
            let hz = pitch_hz(&window, rate, 440.0, &label);
            let cents = 1200.0 * (hz / 440.0).log2();
            println!("{label}: {hz:.4} Hz ({cents:+.4} cents)");
            assert!(cents.abs() < 1.0, "{label}: {hz} Hz ({cents:+.3} cents)");
        }
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

/// Velocity scales the output from the note on: at Velocity 50% a velocity of 64 plays
/// `1 − 0.5 · (1 − 64/127)` of the full-velocity render from the note's first block on (the step
/// ramps over that block; the firmware's output is the same either way); at Velocity 0 every
/// velocity plays the knob.
#[test]
fn velocity_sets_the_level_from_the_note_on() {
    let g = velocity_gain(0.5, 64.0 / 127.0);
    assert!((g - (1.0 - 0.5 * (63.0 / 127.0))).abs() < 1e-7);
    assert_eq!(velocity_gain(0.5, 1.0).to_bits(), 1.0f32.to_bits());
    assert_eq!(velocity_gain(0.0, 0.1).to_bits(), 1.0f32.to_bits());
    let render_at = |vel: u8, amount: f32| {
        let mut t = instrument(48_000, &[("level", 0.0), ("velocity", amount)]);
        render(&mut t, 24_000, 256, &[(0, Ev::On(60, vel))])
    };
    let full = render_at(127, 50.0);
    let soft = render_at(64, 50.0);
    for (a, b) in full[0][BLOCK..].iter().zip(&soft[0][BLOCK..]) {
        assert!((b - a * g).abs() <= 1e-6 * (1.0 + a.abs()), "{b} against {a} × {g}");
    }
    assert!(same_bits(&render_at(127, 0.0), &render_at(20, 0.0)), "Velocity 0: velocity changes nothing");
}

/// A matrix row moves the module's controls as the knob would: the mod wheel at full through a
/// 50% row to Density plays what the crate plays with Density 50% higher, bit for bit (the row's
/// sum is exactly 0.5); Bank, Dry/Wet and Reverb take the destinations past Pan.
#[test]
fn matrix_rows_move_the_modules_controls() {
    let blocks = 3000;
    for (dest, field) in [(5.0, "density"), (15.0, "bank"), (16.0, "dry_wet"), (17.0, "reverb")] {
        let params = [("level", 0.0), ("dry_wet", 40.0), ("mod1_source", 10.0), ("mod1_dest", dest), ("mod1_amount", 50.0)];
        let mut t = instrument(48_000, &params);
        let got = render(&mut t, blocks * BLOCK, 256, &[(0, Ev::Cc(1, 127)), (0, Ev::On(69, 127))]);
        let mut c = default_controls(0.25, SeedButton::Off);
        c.dry_wet = 40.0 / 100.0;
        // The knob plus the row's 0.5, in the instrument's arithmetic.
        match field {
            "density" => c.density = 25.0 / 100.0 + 0.5,
            "bank" => c.feedback = 25.0 / 100.0 + 0.5,
            "dry_wet" => c.dry_wet = 40.0 / 100.0 + 0.5,
            _ => c.reverb = 30.0 / 100.0 + 0.5,
        }
        // The crate with one held note from frame 0 (the script's first note, held to the end).
        let mut synth = Synth::new();
        let mut want = [Vec::new(), Vec::new()];
        let mut flags = 0u8;
        c.pitch.cv = 21.0;
        for _ in 0..blocks {
            let mut seed = [0u8; BLOCK];
            for f in &mut seed {
                flags = flags_after(flags, true);
                *f = flags;
            }
            let mut o = [[0.0f32; 2]; BLOCK];
            synth.render(&c, SynthResolution::Bits16, &[false; BLOCK], Some(&seed), &mut o);
            for f in o {
                want[0].push(f[0]);
                want[1].push(f[1]);
            }
        }
        assert!(same_bits(&got, &want), "{field}");
    }
}

/// The largest step between neighboring samples in `x[range]`.
fn largest_step(x: &[f32], range: std::ops::Range<usize>) -> f32 {
    x[range].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// The shaper switched on while a key is held (#262) crossfades into it over one 12-frame block,
/// then plays exactly what a render shaped from the start plays (the filter and the envelopes run
/// either way); switched off, it crossfades back to the synth's bits.
#[test]
fn switching_the_shaper_while_a_key_is_held_crossfades_at_once() {
    let shaped_params = [("shaper", 1.0), ("cutoff", 900.0), ("resonance", 30.0), ("amp_sustain", 60.0), ("level", 0.0)];
    let plain = |p: &[(&'static str, f32)]| p.iter().filter(|(id, _)| *id != "shaper").copied().collect::<Vec<_>>();
    // On frames where a 256-frame host block and a 12-frame block both start.
    let on_at = 24_576;
    let off_at = 49_152;
    let frames = 73_728;
    let mut shaped = instrument(48_000, &shaped_params);
    let ys = render(&mut shaped, frames, 256, &[(0, Ev::On(57, 127))]);
    let mut dry = instrument(48_000, &plain(&shaped_params));
    let yd = render(&mut dry, frames, 256, &[(0, Ev::On(57, 127))]);
    let mut switched = instrument(48_000, &plain(&shaped_params));
    let y = render(&mut switched, frames, 256, &[(0, Ev::On(57, 127)), (on_at, Ev::Param("shaper", 1.0)), (off_at, Ev::Param("shaper", 0.0))]);
    for c in 0..2 {
        assert!(y[c][..on_at].iter().zip(&yd[c]).all(|(a, b)| a.to_bits() == b.to_bits()), "before the switch: the synth's bits");
        let after_in = on_at + BLOCK;
        assert!(y[c][after_in..off_at].iter().zip(&ys[c][after_in..off_at]).all(|(a, b)| a.to_bits() == b.to_bits()), "shaped after the fade");
        let after_out = off_at + BLOCK;
        assert!(y[c][after_out..].iter().zip(&yd[c][after_out..]).all(|(a, b)| a.to_bits() == b.to_bits()), "the synth's bits after the fade");
        let own = largest_step(&ys[c], on_at - 256..on_at + 256).max(largest_step(&yd[c], on_at - 256..on_at + 256));
        let gap = (on_at..on_at + BLOCK).map(|i| (ys[c][i] - yd[c][i]).abs()).fold(0.0f32, f32::max);
        let step = largest_step(&y[c], on_at - 1..on_at + BLOCK + 1);
        assert!(step <= own + gap / BLOCK as f32 + 1e-6, "the crossfade steps {step} (own {own}, gap {gap})");
    }
}

/// With no key held the switch waits for the next note; before the first note it applies at once.
#[test]
fn with_no_key_held_the_shaper_switch_waits_for_the_next_note() {
    let params = [("cutoff", 700.0), ("level", 0.0), ("amp_release", 5.0)];
    let mut t = instrument(48_000, &params);
    let script = [(0, Ev::On(60, 127)), (6000, Ev::Off(60)), (12_000, Ev::Param("shaper", 1.0)), (24_000, Ev::On(64, 127))];
    let y = render(&mut t, 36_000, 256, &script);
    let mut dry = instrument(48_000, &params);
    let yd = render(&mut dry, 36_000, 256, &script[..2]);
    assert!(y[0][..24_000].iter().zip(&yd[0]).all(|(a, b)| a.to_bits() == b.to_bits()), "the switch waits for the note");
    let mut early = instrument(48_000, &[("cutoff", 700.0), ("level", 0.0), ("amp_release", 5.0), ("shaper", 1.0)]);
    let mut late = instrument(48_000, &params);
    let a = render(&mut early, 12_000, 256, &[(100, Ev::On(60, 127))]);
    let b = render(&mut late, 12_000, 256, &[(0, Ev::Param("shaper", 1.0)), (100, Ev::On(60, 127))]);
    assert!(same_bits(&a, &b), "a switch before the first note applies at once");
}

/// A reset instrument renders what a new one renders.
#[test]
fn a_reset_instrument_renders_what_a_new_one_renders() {
    let mut used = instrument(44_100, &BUSY_PARAMS);
    let _ = render(&mut used, 30_000, 256, &busy_script());
    used.reset();
    for (id, v) in [("shaper", 0.0), ("mod1_source", 0.0), ("quality", 3.0), ("freeze", 0.0), ("seed", 0.0), ("seed_latch", 0.0)] {
        assert!(used.set_param(id, v));
    }
    used.reset();
    let mut fresh = instrument(44_100, &BUSY_PARAMS);
    let script = &NOTES[..8];
    assert!(same_bits(&render(&mut used, 30_000, 256, script), &render(&mut fresh, 30_000, 256, script)));
}

/// All Sound Off (CC 120) silences at once (the synth back to power-on, the reverb's memory
/// cleared); All Notes Off (CC 123) lowers the gate and lets the grains and the reverb play out:
/// with the dry oscillator off (all wet) that is the render where the note is released then.
#[test]
fn all_sound_off_silences_at_once_and_all_notes_off_lets_the_grains_play_out() {
    let params = [("dry_wet", 100.0), ("size", 90.0), ("reverb", 60.0)];
    let at = 24_000;
    let run = |extra: Option<Ev>| {
        let mut t = instrument(48_000, &params);
        let mut script = vec![(0, Ev::On(60, 127))];
        if let Some(e) = extra {
            script.push((at, e));
        }
        render(&mut t, 48_000, 256, &script)
    };
    let off = run(Some(Ev::Cc(120, 0)));
    assert!(peak(&off[0][..at]) > 0.01);
    assert!(off.iter().all(|c| c[at + BLOCK..].iter().all(|v| *v == 0.0)), "silent from the next block on");
    let notes_off = run(Some(Ev::Cc(123, 0)));
    let released = run(Some(Ev::Off(60)));
    assert!(same_bits(&notes_off, &released), "CC 123 is the note released");
    assert!(rms(&notes_off[0][at..at + 9600]) > 0.001, "the grains and the reverb play out");
}

/// All Sound Off also silences the dry oscillator (Dry/Wet below 100% with the shaper off, which
/// has no gate): the output stays zero until the next note.
#[test]
fn all_sound_off_silences_the_dry_oscillator_until_the_next_note() {
    let mut t = instrument(48_000, &[("dry_wet", 50.0), ("shaper", 0.0), ("reverb", 0.0)]);
    let at = 24_000;
    let out = render(&mut t, 72_000, 256, &[(0, Ev::On(60, 127)), (at, Ev::Cc(120, 0)), (48_000, Ev::On(64, 100))]);
    assert!(rms(&out[0][at - 4800..at]) > 0.001, "the dry oscillator sounds before");
    assert!(out.iter().all(|c| c[at + 256 + 2 * BLOCK..48_000].iter().all(|v| *v == 0.0)), "zero after CC 120 until a note");
    assert!(rms(&out[0][49_000..72_000]) > 0.001, "a note-on sounds again");
}

/// The frozen state: with Freeze on, a state taken between blocks restores into a new instance
/// that renders on bit for bit at the same host rate (44.1 and 48 kHz, the shaper off), with no
/// note held; with Freeze off there is none; an empty state is nothing and changes nothing; a
/// damaged or foreign one is refused and leaves the instance as it was.
#[test]
fn a_frozen_cloud_saves_and_restores_bit_for_bit() {
    for sr in [48_000u32, 44_100] {
        let params = [("density", 20.0), ("size", 95.0), ("dry_wet", 80.0), ("reverb", 40.0)];
        let mut live = instrument(sr, &params);
        let _ = render(&mut live, 30_000, 500, &[(0, Ev::On(57, 100)), (12_000, Ev::Param("freeze", 1.0)), (20_000, Ev::Off(57))]);
        let state = live.save_state().expect("frozen");
        assert!(state.len() > 64 * 1024 && state.len() <= state::capacity(), "{} bytes", state.len());
        let mut restored = TextureSynth::new();
        assert!(restored.load_state(&state));
        restored.prepare(sr as f32, 333, 2);
        restored.reset();
        let a = render(&mut live, 48_000, 500, &[]);
        let b = render(&mut restored, 48_000, 333, &[]);
        assert!(peak(&a[0]) > 0.01, "{sr} Hz: the frozen cloud sounds");
        for c in 0..2 {
            if let Some(i) = (0..a[c].len()).find(|&i| a[c][i].to_bits() != b[c][i].to_bits()) {
                panic!("{sr} Hz ch {c}: first difference at {i}: {} vs {}", a[c][i], b[c][i]);
            }
        }
        assert!(same_bits(&a, &b), "{sr} Hz: the restored cloud plays on bit for bit");
        assert_eq!(restored.param("freeze"), Some(1.0));
        assert_eq!(restored.param("density"), Some(20.0));

        let mut thawed = instrument(sr, &params);
        assert!(thawed.save_state().is_none(), "nothing frozen, nothing saved");
        assert!(thawed.load_state(&[]), "an empty state is nothing");
        let mut damaged = state.clone();
        damaged[0] ^= 1;
        assert!(!thawed.load_state(&damaged), "a foreign magic is refused");
        assert!(!thawed.load_state(&state[..state.len() - 1]), "a short state is refused");
        let mut bad_param = state.clone();
        let at = 16 + 4 * DENSITY;
        bad_param[at..at + 4].copy_from_slice(&500.0f32.to_le_bytes());
        assert!(!thawed.load_state(&bad_param), "a parameter out of range is refused");
        assert_eq!(thawed.param("freeze"), Some(0.0), "a refused state leaves the instance as it was");
    }
}

#[test]
fn initial_params_snap_level_without_resetting_a_restored_cloud() {
    for sr in [48_000u32, 44_100] {
        let mut live = instrument(sr, &[("density", 20.0), ("size", 95.0), ("dry_wet", 80.0), ("reverb", 40.0)]);
        let _ = render(&mut live, 30_000, 500, &[(0, Ev::On(57, 100)), (12_000, Ev::Param("freeze", 1.0)), (20_000, Ev::Off(57))]);
        let state = live.save_state().expect("frozen cloud");
        let mut restored = TextureSynth::new();
        assert!(restored.load_state(&state));
        restored.prepare(sr as f32, 333, 2);
        assert!(restored.set_param("level", -18.0));
        assert_ne!(restored.level.value(), db_to_gain(-18.0), "the restored level is the old one");
        restored.snap_initial_params();
        assert_eq!(restored.level.value(), db_to_gain(-18.0), "the first sample starts at the stored level");
        assert!(!restored.level.settling());

        // The continuing instance is the independent cloud/converter reference. Change only its
        // level, bypassing the hook under test; a reset of the restored cloud would fail the audio.
        live.set_param("level", -18.0);
        live.level.set(db_to_gain(-18.0));
        live.level.snap();
        let expected = render(&mut live, 12_000, 500, &[]);
        let actual = render(&mut restored, 12_000, 333, &[]);
        assert!(peak(&expected[0]) > 0.001, "the cloud remains audible at {sr} Hz");
        assert!(same_bits(&actual, &expected), "the cloud and converter survive at {sr} Hz");
    }
}

/// A mono strip hears the mean of the two outputs; hostile parameter values clamp; NaN controller
/// values are not reachable (MIDI bytes), and every output is finite.
#[test]
fn a_mono_strip_and_hostile_values() {
    let mut stereo = instrument(48_000, &[]);
    let ys = render(&mut stereo, 9600, 256, &[(0, Ev::On(60, 127))]);
    let mut mono = TextureSynth::new();
    mono.prepare(48_000.0, 512, 1);
    let mut io = vec![vec![0.0f32; 256]];
    mono.note_on(0, 60, 127);
    let mut ym = Vec::new();
    for _ in 0..9600 / 256 {
        mono.process(&mut io, 256);
        ym.extend_from_slice(&io[0]);
    }
    for (i, m) in ym.iter().enumerate() {
        assert_eq!(m.to_bits(), ((ys[0][i] + ys[1][i]) * 0.5).to_bits());
    }
    let mut t = instrument(48_000, &[]);
    for id in ["density", "pitch", "level", "mod1_amount", "quality", "bend_range"] {
        assert!(t.set_param(id, f32::INFINITY));
        assert!(t.set_param(id, f32::NAN));
        assert!(t.set_param(id, -1.0e9));
    }
    let y = render(&mut t, 24_000, 256, &[(0, Ev::On(127, 127)), (100, Ev::On(0, 1)), (200, Ev::Bend(16_383))]);
    assert!(y.iter().flatten().all(|v| v.is_finite()));
    assert_eq!(pitch_cv(127, 24.0), 60.0, "the CV stops at the module's +60");
    assert_eq!(pitch_cv(0, -24.0), -60.0);
    assert_eq!(pitch_cv(69, 0.0).to_bits(), 21.0f32.to_bits());
}

//! The Cross Modulator (#200): the firmware-exact path at 96 kHz against the C++ oracle (keyed and
//! with the oscillator), host-block independence at every rate, the latency it reports against the
//! delay it has, the oscillator's pitch at every session rate (E63's class: an absolute frequency,
//! octave aliases refused), the input routing (key, swap, oscillator modulation), the dry path,
//! and the two channels' independence.

use super::path::{Path, RATE, latency_at};
use super::*;
use crate::Plugin;
use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::modulator::{self as oracle, MODULATOR_BLOCK_SIZE};

const RATES: [u32; 6] = [32_000, 44_100, 48_000, 88_200, 96_000, 192_000];

fn device(sr: u32, params: &[(&str, f32)]) -> CrossModulator {
    let mut m = CrossModulator::new();
    for (id, v) in params {
        assert!(m.set_param(id, *v), "{id}");
    }
    m.prepare(sr as f32, 512, 2);
    m
}

/// Integer-generated int16 signals at 96 kHz as floats on the int16 grid (so the codec's
/// conversion is exact): `track` a triangle with an envelope, `key` a sawtooth plus a slow
/// triangle, both with a little noise, the key silent for a stretch.
fn grid_signals(frames: usize) -> ([Vec<f32>; 2], [Vec<f32>; 2]) {
    let mut seed = 0x0200_7E57_u32;
    let mut track = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut key = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for n in 0..frames {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = i32::from((seed >> 16) as u16 as i16) >> 5;
        let n32 = n as u32;
        let tri = |p: u32| ((n32 % p) as i32 * 65_536 / p as i32 - 32_768).abs() * 2 - 32_768;
        let saw = |p: u32| (n32 % p) as i32 * 65_536 / p as i32 - 32_768;
        let env = ((n32 % 9600) as i32).min(9600 - (n32 % 9600) as i32) * 8;
        let t = [(tri(293) / 2 + noise) * env / 38_400, (tri(311) / 2 - noise) * env / 38_400];
        let silent = (30_000..36_000).contains(&n);
        let k = if silent { [0, 0] } else { [saw(437) * 3 / 8 + tri(2903) / 5, saw(433) * 3 / 8 - tri(2801) / 5] };
        for c in 0..2 {
            track[c].push(t[c].clamp(-32_768, 32_767) as f32 / 32_768.0);
            key[c].push(k[c].clamp(-32_768, 32_767) as f32 / 32_768.0);
        }
    }
    (track, key)
}

/// Renders `input` (with `key` when given) in host blocks of `block`, calling `at(frame, device)`
/// before each block.
fn render(
    m: &mut CrossModulator,
    input: &[Vec<f32>; 2],
    key: Option<&[Vec<f32>; 2]>,
    block: usize,
    mut at: impl FnMut(usize, &mut CrossModulator),
) -> [Vec<f32>; 2] {
    let frames = input[0].len();
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut kb = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        at(pos, m);
        for c in 0..2 {
            io[c][..n].copy_from_slice(&input[c][pos..pos + n]);
        }
        match key {
            Some(k) => {
                for c in 0..2 {
                    kb[c][..n].copy_from_slice(&k[c][pos..pos + n]);
                }
                m.process_with_key(&mut io, &kb, n);
            }
            None => m.process(&mut io, n),
        }
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

fn oracle_carrier(c: CarrierShape) -> oracle::CarrierShape {
    match c {
        CarrierShape::External => oracle::CarrierShape::External,
        CarrierShape::Sine => oracle::CarrierShape::Sine,
        CarrierShape::Triangle => oracle::CarrierShape::Triangle,
        CarrierShape::Saw => oracle::CarrierShape::Saw,
    }
}

/// The oracle over one channel: `input_1` and `input_2` (on the int16 grid) in 60-frame blocks,
/// seeded as the device's channel `c`, with the device's firmware parameters.
fn oracle_render(p: &Parameters, input_1: &[f32], input_2: &[f32], seed: u32) -> Vec<i16> {
    let session = Session::lock();
    session.seed(seed);
    let mut m = session.modulator().unwrap();
    let q = oracle::Parameters { drive: p.drive, algorithm: p.algorithm, timbre: p.timbre, note: p.note, carrier: oracle_carrier(p.carrier) };
    let to16 = |x: f32| (x * 32_768.0) as i16;
    let mut out = Vec::with_capacity(input_1.len());
    for (a, b) in input_1.chunks(MODULATOR_BLOCK_SIZE).zip(input_2.chunks(MODULATOR_BLOCK_SIZE)) {
        let frames: Vec<[i16; 2]> = a.iter().zip(b).map(|(x, y)| [to16(*x), to16(*y)]).collect();
        let mut o = vec![[0_i16; 2]; frames.len()];
        m.process(&q, &frames, &mut o).unwrap();
        out.extend(o.iter().map(|f| f[0]));
    }
    out
}

/// The device's float output as the module's codec would write it: truncated and clipped to int16.
fn codec(y: f32) -> i16 {
    ((y * 32_768.0) as i32).clamp(-32_768, 32_767) as i16
}

/// At 96 kHz nothing is converted: the device's output, 59 frames late, is the firmware's main
/// output, which on the codec's int16 grid is the oracle's bit for bit, in every routing (keyed,
/// keyed and swapped, keyed with the oscillator modulated by the key, and with no key at all).
#[test]
fn at_96_khz_the_output_is_the_oracle_bit_for_bit() {
    let frames = 48_000;
    let (track, key) = grid_signals(frames + 60);
    let zero = vec![0.0f32; frames + 60];
    for (name, params, keyed, swapped) in [
        ("keyed diode ring", vec![("algorithm", 2.0), ("timbre", 70.0)], true, false),
        ("keyed vocoder", vec![("algorithm", 6.6), ("timbre", 35.0), ("carrier_level", 90.0)], true, false),
        ("keyed morph, swapped", vec![("algorithm", 4.4), ("swap", 1.0), ("modulator_level", 40.0)], true, true),
        ("keyed, oscillator modulated by the key", vec![("algorithm", 0.5), ("carrier", 1.0), ("shape", 1.0)], true, false),
        ("no key: the oscillator", vec![("algorithm", 3.25), ("shape", 2.0), ("frequency", 330.0)], false, false),
        ("no key: the noise carrier", vec![("algorithm", 6.8), ("shape", 2.0)], false, false),
    ] {
        let mut m = device(RATE, &params);
        assert_eq!(m.latency(), BLOCK_SIZE - 1);
        let got = render(&mut m, &track, keyed.then_some(&key), 256, |_, _| {});
        let p = m.firmware_parameters(keyed);
        for c in 0..2 {
            let (one, two): (&[f32], &[f32]) = match (keyed, swapped) {
                (true, false) => (&key[c], &track[c]),
                (true, true) => (&track[c], &key[c]),
                (false, _) => (&zero, &track[c]),
            };
            let want = oracle_render(&p, &one[..frames], &two[..frames], SEEDS[c]);
            let late = &got[c][BLOCK_SIZE - 1..BLOCK_SIZE - 1 + frames];
            let bad = late.iter().zip(&want).position(|(y, w)| codec(*y) != *w);
            assert!(bad.is_none(), "{name}, channel {c}: first difference at frame {bad:?}");
            assert!(want.iter().any(|w| w.unsigned_abs() > 1000), "{name}, channel {c}: the oracle sounds");
        }
    }
}

/// The output for the same input and parameter values is bit-identical whatever the host block
/// size, keyed or not, with parameter changes at frames both splits share; and no output sample is
/// read before the firmware rendered it.
#[test]
fn host_block_size_does_not_change_a_bit() {
    for sr in [44_100, 48_000, 96_000] {
        let frames = sr as usize;
        let (track, key) = grid_signals(frames);
        let at = |pos: usize, m: &mut CrossModulator| match pos {
            8192 => assert!(m.set_param("algorithm", 5.5)),
            16_384 => assert!(m.set_param("timbre", 10.0)),
            24_576 => assert!(m.set_param("algorithm", 6.9)),
            32_768 => assert!(m.set_param("dry_wet", 60.0)),
            _ => {}
        };
        for keyed in [true, false] {
            let k = keyed.then_some(&key);
            let mut a = device(sr, &[]);
            let mut b = device(sr, &[]);
            let mut c = device(sr, &[]);
            let ya = render(&mut a, &track, k, 64, at);
            let yb = render(&mut b, &track, k, 512, at);
            let yc = render(&mut c, &track, k, 4096, at);
            assert!(same_bits(&ya, &yb) && same_bits(&ya, &yc), "{sr} Hz, keyed {keyed}");
            assert_eq!(a.starved() + b.starved() + c.starved(), 0, "{sr} Hz");
            assert!(ya[0].iter().any(|v| v.abs() > 0.05), "{sr} Hz: it sounds");
        }
    }
}

/// The latency each path reports is the delay it has: an impulse through the path with the
/// firmware replaced by a copy of the track comes out exactly that many session samples late, at
/// every rate (at its full level up to 96 kHz).
#[test]
fn the_reported_latency_is_the_impulse_delay() {
    let kernel = Kernel::new();
    for sr in RATES {
        let mut path = Path::new(&kernel, sr);
        let latency = path.latency();
        assert_eq!(latency, latency_at(sr).0);
        let n = latency + 2000;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x = if i == 100 { 1.0 } else { 0.0 };
            out.push(path.tick(&kernel, [x, -x], [0.0; 2], &mut |f: &[Frame], o: &mut [[f32; 2]]| {
                for (y, x) in o.iter_mut().zip(f) {
                    *y = [x[0], x[1]];
                }
            }));
        }
        let peak = (0..n).max_by(|&a, &b| out[a][0].abs().total_cmp(&out[b][0].abs())).unwrap();
        assert_eq!(peak, 100 + latency, "{sr} Hz");
        // Above 96 kHz half the impulse's band is above the firmware's Nyquist and is filtered out
        // on the way down, so the peak is about half.
        let level = if sr > RATE { 0.4 } else { 0.8 };
        assert!(out[peak][0] > level && out[peak][1] < -level, "{sr} Hz: {:?}", out[peak]);
        assert_eq!(path.starved(), 0, "{sr} Hz");
        println!("{sr} Hz: latency {latency} samples ({:.2} ms)", latency as f64 * 1000.0 / f64::from(sr));
    }
    // The table the ADR note quotes.
    let table: Vec<usize> = RATES.iter().map(|sr| latency_at(*sr).0).collect();
    assert_eq!(table[4], 59, "96 kHz converts nothing: the block less one frame");
}

/// The oscillator's frequency through the converters at every session rate is the firmware's own
/// (its phase increment at 96 kHz) to 0.05 cent: measured from zero crossings over a second, an
/// absolute frequency, so an octave alias (or a converter at the wrong ratio) fails it.
#[test]
fn the_oscillator_pitch_holds_at_every_session_rate() {
    for frequency in [110.0, 440.0, 1760.0] {
        let note = note_of(frequency);
        let increment = transit_modulator_increment(note);
        let firmware_hz = increment * 96_000.0;
        for sr in [44_100, 48_000, 96_000] {
            // No key: the sine carrier; the crossfade at timbre 0 is the carrier alone.
            let mut m = device(sr, &[("algorithm", 0.0), ("timbre", 0.0), ("shape", 0.0), ("frequency", frequency)]);
            let frames = 2 * sr as usize;
            let silent = [vec![0.0f32; frames], vec![0.0f32; frames]];
            let y = render(&mut m, &silent, None, 512, |_, _| {});
            let x = &y[0][sr as usize / 2..];
            let mut crossings = Vec::new();
            for i in 1..x.len() {
                if x[i - 1] < 0.0 && x[i] >= 0.0 {
                    let f = f64::from(x[i - 1]) / f64::from(x[i - 1] - x[i]);
                    crossings.push(i as f64 - 1.0 + f);
                }
            }
            let (first, last) = (crossings[1], crossings[crossings.len() - 2]);
            let measured = (crossings.len() - 3) as f64 * f64::from(sr) / (last - first);
            let cents = 1200.0 * (measured / firmware_hz).log2();
            println!("{frequency} Hz at {sr} Hz: firmware {firmware_hz:.4} Hz, measured {measured:.4} Hz ({cents:+.4} cent)");
            assert!(cents.abs() < 0.05, "{frequency} Hz at {sr} Hz: {cents} cent");
            assert!(x.iter().any(|v| v.abs() > 0.2), "it sounds");
        }
    }
}

/// The firmware's phase increment at `note` (the pitch table's lookup, as the oscillator does it).
fn transit_modulator_increment(note: f32) -> f64 {
    // A modulator rendering the sine for one block reports nothing; read the frequency from a
    // render at 96 kHz instead: zero crossings over two seconds of the firmware alone.
    let mut m = Modulator::with_seed(SEEDS[0]);
    let p = Parameters { drive: [0.0, 0.0], algorithm: 0.0, timbre: 0.0, note, carrier: CarrierShape::Sine };
    let zeros = [0.0f32; BLOCK_SIZE];
    let mut x = Vec::new();
    for _ in 0..3200 {
        let (mut main, mut aux) = ([0.0f32; BLOCK_SIZE], [0.0f32; BLOCK_SIZE]);
        m.process_float(&p, &zeros, &zeros, &mut main, &mut aux);
        x.extend_from_slice(&aux);
    }
    // The aux output is the oscillator itself, before the 6x converters.
    let mut crossings = Vec::new();
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            crossings.push(i as f64 - 1.0 + f64::from(x[i - 1]) / f64::from(x[i - 1] - x[i]));
        }
    }
    let (first, last) = (crossings[1], crossings[crossings.len() - 2]);
    (crossings.len() - 3) as f64 / (last - first)
}

/// The routing: a key changes the output (it carries); with `Carrier: Oscillator` the key moves the
/// oscillator instead (different again from no key); `Swap Inputs` changes it; with no key, swap
/// does nothing.
#[test]
fn the_key_carries_swaps_and_modulates_the_oscillator() {
    let sr = 48_000;
    let (track, key) = grid_signals(24_000);
    let run = |params: &[(&str, f32)], k: Option<&[Vec<f32>; 2]>| render(&mut device(sr, params), &track, k, 512, |_, _| {});
    let keyed = run(&[], Some(&key));
    let unkeyed = run(&[], None);
    let osc_keyed = run(&[("carrier", 1.0)], Some(&key));
    let swapped = run(&[("swap", 1.0)], Some(&key));
    let unkeyed_swapped = run(&[("swap", 1.0)], None);
    assert!(!same_bits(&keyed, &unkeyed), "the key carries");
    assert!(!same_bits(&osc_keyed, &unkeyed) && !same_bits(&osc_keyed, &keyed), "the key modulates the oscillator");
    assert!(!same_bits(&swapped, &keyed), "swap changes the roles");
    assert!(same_bits(&unkeyed_swapped, &unkeyed), "with no key there is nothing to swap");
    assert_eq!(MODULATOR_INFO.id, "cross_modulator");
    assert!(crate::builtin_takes_key(MODULATOR_INFO.id) && CrossModulator::new().has_key_input());
}

/// `Dry/Wet` at 0 is the track itself, delayed by exactly the latency the device reports.
#[test]
fn dry_is_the_track_delayed_by_the_latency() {
    for sr in [44_100, 96_000] {
        let (track, key) = grid_signals(12_000);
        let mut m = device(sr, &[("dry_wet", 0.0)]);
        let l = m.latency();
        let y = render(&mut m, &track, Some(&key), 333, |_, _| {});
        for c in 0..2 {
            assert!(y[c][..l].iter().all(|v| *v == 0.0));
            assert!(y[c][l..].iter().zip(&track[c]).all(|(a, b)| a.to_bits() == b.to_bits()), "{sr} Hz, channel {c}");
        }
    }
}

/// The two channels are two firmware instances: each channel's output depends on its own track and
/// key channel only.
#[test]
fn the_channels_are_independent() {
    let (track, key) = grid_signals(12_000);
    let a = render(&mut device(48_000, &[]), &track, Some(&key), 512, |_, _| {});
    let mut other = track.clone();
    other[1].iter_mut().for_each(|v| *v *= 0.5);
    let b = render(&mut device(48_000, &[]), &other, Some(&key), 512, |_, _| {});
    assert!(a[0].iter().zip(&b[0]).all(|(x, y)| x.to_bits() == y.to_bits()), "left untouched by the right");
    assert!(a[1].iter().zip(&b[1]).any(|(x, y)| x != y), "right follows its input");
}

/// `reset` is a fresh instance; a mono strip runs one channel.
#[test]
fn reset_is_a_fresh_instance_and_mono_runs_one_channel() {
    let (track, key) = grid_signals(12_000);
    let fresh = render(&mut device(44_100, &[]), &track, Some(&key), 512, |_, _| {});
    let mut m = device(44_100, &[]);
    let _ = render(&mut m, &track, Some(&key), 512, |_, _| {});
    m.reset();
    let again = render(&mut m, &track, Some(&key), 512, |_, _| {});
    assert!(same_bits(&fresh, &again));
    let mut mono = CrossModulator::new();
    mono.prepare(44_100.0, 512, 1);
    let mut io = vec![track[0][..512].to_vec()];
    mono.process_with_key(&mut io, &[key[0][..512].to_vec()], 512);
    assert!(io[0].iter().all(|v| v.is_finite()));
}

#[test]
fn the_algorithm_names_and_the_note_mapping() {
    assert_eq!(algorithm_name(0.0), "Crossfade");
    assert_eq!(algorithm_name(2.4), "Diode Ring");
    assert_eq!(algorithm_name(8.0), "Vocoder (freeze)");
    assert!((note_of(440.0) - 69.0).abs() < 1e-4);
    assert!((note_of(FREQUENCY_RANGE.0) - 36.0).abs() < 1e-3 && (note_of(FREQUENCY_RANGE.1) - 96.0).abs() < 1e-3);
}

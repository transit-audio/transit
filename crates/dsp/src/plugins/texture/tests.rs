//! The Texture insert (#241): host-block independence at every rate, the latency it reports against
//! the delay it has, the firmware-exact path at 48 kHz against the crate driven directly, a grain's
//! pitch at every session rate (E63), and the restart fade around a reconfiguration.

use super::*;
use crate::Plugin;
use std::f64::consts::TAU;

const RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

fn device(sr: u32, params: &[(&str, f32)]) -> TextureFx {
    let mut t = TextureFx::new();
    for (id, v) in params {
        assert!(t.set_param(id, *v), "{id}");
    }
    t.prepare(sr as f32, 512, 2);
    t
}

/// A test signal at `sr`: two detuned tones and a burst of noise every half second.
fn signal(sr: u32, frames: usize) -> [Vec<f32>; 2] {
    let mut seed = 0x1234_5678u32;
    let mut l = Vec::with_capacity(frames);
    let mut r = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / f64::from(sr);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = if (t * 2.0).fract() < 0.05 { (seed >> 8) as f32 / (1 << 24) as f32 - 0.5 } else { 0.0 };
        l.push(0.3 * (TAU * 220.0 * t).sin() as f32 + noise);
        r.push(0.3 * (TAU * 331.0 * t).sin() as f32 - noise);
    }
    [l, r]
}

/// Renders `input` in host blocks of `block`, calling `at(frame, plugin)` before each block.
fn render(t: &mut TextureFx, input: &[Vec<f32>; 2], block: usize, mut at: impl FnMut(usize, &mut TextureFx)) -> [Vec<f32>; 2] {
    let frames = input[0].len();
    let mut out = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut io = vec![vec![0.0f32; block]; 2];
    let mut pos = 0;
    while pos < frames {
        let n = block.min(frames - pos);
        at(pos, t);
        for c in 0..2 {
            io[c][..n].copy_from_slice(&input[c][pos..pos + n]);
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

/// Parameter changes at frames that are block boundaries of both splits: the freeze, a seed press
/// with the latch off, a pitch move.
fn script(frame: usize, t: &mut TextureFx) {
    match frame {
        65_536 => assert!(t.set_param("freeze", 1.0)),
        98_304 => {
            assert!(t.set_param("seed_latch", 0.0));
            assert!(t.set_param("density", 50.0));
            assert!(t.set_param("seed", 1.0));
        }
        102_400 => assert!(t.set_param("seed", 0.0)),
        114_688 => {
            assert!(t.set_param("freeze", 0.0));
            assert!(t.set_param("pitch", -5.0));
            assert!(t.set_param("seed_latch", 1.0));
            assert!(t.set_param("density", 80.0));
        }
        _ => {}
    }
}

#[test]
fn host_block_size_changes_no_bit_at_44_1_and_48_khz_for_every_quality() {
    for sr in [44_100, 48_000] {
        for quality in 0..4 {
            let params =
                [("quality", quality as f32), ("dry_wet", 70.0), ("feedback", 35.0), ("reverb", 30.0), ("time_random", 40.0), ("pitch", 7.0)];
            let x = signal(sr, 131_072);
            let mut a = device(sr, &params);
            let mut b = device(sr, &params);
            let ya = render(&mut a, &x, 64, script);
            let yb = render(&mut b, &x, 512, script);
            assert!(ya[0].iter().any(|v| v.abs() > 0.01), "{sr} Hz quality {quality}: silent");
            assert!(same_bits(&ya, &yb), "{sr} Hz quality {quality}: 64- and 512-frame host blocks differ");
            assert_eq!((a.starved(), b.starved()), (0, 0), "{sr} Hz quality {quality}");
        }
    }
}

/// Each codec rate's path with a pass-through in place of the firmware delays an impulse by
/// exactly the latency it reports, at every session rate, and never reads ahead of the firmware.
#[test]
fn each_path_delays_an_impulse_by_its_reported_latency() {
    let kernel = Kernel::new();
    for sr in RATES.into_iter().chain([22_050, 176_400, 192_000, 44_056]) {
        for media in [Media::DirtyTape, Media::VintageDigital, Media::Digital] {
            let mut path = Path::new(&kernel, sr, media);
            let at = 3000;
            let y: Vec<f32> =
                (0..9000).map(|n| path.tick(&kernel, if n == at { [0.5, -0.5] } else { [0.0; 2] }, &mut |i, o| o.copy_from_slice(i))[0]).collect();
            let peak = (0..y.len()).max_by(|&i, &j| y[i].abs().total_cmp(&y[j].abs())).unwrap_or(0);
            assert_eq!(peak, at + path.latency(), "{sr} Hz, {} Hz codec", media.sample_rate());
            let gain = (f64::from(media.sample_rate()) / f64::from(sr)).min(1.0) as f32;
            assert!(y[peak] > 0.4 * gain, "{sr} Hz, {} Hz codec: peak {}", media.sample_rate(), y[peak]);
            assert_eq!(path.starved(), 0, "{sr} Hz, {} Hz codec", media.sample_rate());
        }
    }
}

/// The dry path (dry/wet 0, no reverb, the input stage at unity) delays an impulse by the latency
/// the device reports: the output's peak lands there at every rate, for the digital, clean tape and
/// vintage digital qualities. Dirty tape adds the module's tape wow and flutter after it, a
/// modulated delay of 1 to about 172 codec samples (an effect, not compensated): its peak comes
/// that much later.
#[test]
fn the_reported_latency_is_the_measured_impulse_delay() {
    let at = 4000;
    for sr in RATES {
        for (quality, media) in Media::ALL.into_iter().enumerate() {
            let mut t = device(sr, &[("quality", quality as f32), ("dry_wet", 0.0), ("input_gain", 1.0), ("density", 50.0), ("seed_latch", 0.0)]);
            let mut x = [vec![0.0f32; 16_000], vec![0.0f32; 16_000]];
            x[0][at] = 0.25;
            x[1][at] = -0.25;
            let y = render(&mut t, &x, 333, |_, _| {});
            let peak = (0..y[0].len()).max_by(|&i, &j| y[0][i].abs().total_cmp(&y[0][j].abs())).unwrap_or(0);
            let latency = t.latency();
            println!(
                "{sr} Hz {media:?} ({} Hz codec): latency {latency} samples ({:.3} ms)",
                media.sample_rate(),
                latency as f64 * 1000.0 / f64::from(sr)
            );
            if media == Media::DirtyTape {
                let codec = f64::from(sr) / 24_000.0;
                let wow = (peak - at - latency) as f64 / codec;
                assert!((0.5..=175.0).contains(&wow), "{sr} Hz {media:?}: peak at {peak}, reported {latency}: {wow} codec samples of wow");
            } else {
                assert_eq!(peak, at + latency, "{sr} Hz {media:?}: peak at {peak}, reported {latency}");
            }
            assert!(
                y[0][peak].abs() > 0.02 && y[0][peak] * y[1][peak] < 0.0,
                "{sr} Hz {media:?}: the impulse came through ({}, {})",
                y[0][peak],
                y[1][peak]
            );
            assert_eq!(t.starved(), 0);
        }
    }
}

/// A 48 kHz session with the digital or clean tape quality converts nothing: the device's output,
/// 11 frames late (the firmware's block less one), is the crate's input stage and effect chain
/// driven directly in the module's 12-frame blocks, bit for bit, through parameter changes.
#[test]
fn at_48_khz_digital_and_clean_tape_are_the_crate_driven_directly() {
    for media in [Media::Digital, Media::CleanTape] {
        for host_block in [64, 480] {
            let params = [("quality", media.index() as f32), ("dry_wet", 60.0), ("feedback", 30.0), ("reverb", 25.0), ("size_random", 30.0)];
            let mut dev = device(48_000, &params);
            assert_eq!(dev.latency(), 11);
            let frames = 96_000;
            let x = signal(48_000, frames);
            let change = 48_000;
            let y = render(&mut dev, &x, host_block, |f, t| {
                if f == change {
                    assert!(t.set_param("freeze", 1.0));
                    assert!(t.set_param("input_gain", 1.0));
                    assert!(t.set_param("input_level", 6.0));
                }
            });
            // The reference: the same parts, by hand.
            let mut reference = TextureFx::new();
            for (id, v) in params {
                reference.set_param(id, v);
            }
            let mut texture = Texture::new(BufferSize::FIRMWARE, media, false);
            let mut stage = InputStage::new(48_000.0, clipping(media));
            let mut expected = vec![[0.0f32; 2]; frames];
            for (k, (i, o)) in (0..frames).collect::<Vec<_>>().chunks(12).zip(expected.chunks_mut(12)).enumerate() {
                if k * 12 == change {
                    reference.set_param("freeze", 1.0);
                }
                let gain = if k * 12 >= change { manual_gain_setting(6.0) } else { 0 };
                let raw: Vec<[i32; 2]> = i.iter().map(|&n| [x[0][n], x[1][n]].map(|s| (s * 2_147_483_648.0) as i32)).collect();
                let mut leveled = vec![[0.0f32; 2]; i.len()];
                stage.process(true, gain, [false, false], &raw, &mut leveled);
                let freeze = vec![k * 12 >= change; i.len()];
                texture.process(&reference.controls, &freeze, None, &leveled, o);
            }
            let late = 11;
            for n in 0..frames {
                let want = if n < late { [0.0; 2] } else { expected[n - late] };
                assert!(
                    y[0][n].to_bits() == want[0].to_bits() && y[1][n].to_bits() == want[1].to_bits(),
                    "{media:?}, {host_block}-frame blocks: frame {n} is ({}, {}), the crate gives ({}, {})",
                    y[0][n],
                    y[1][n],
                    want[0],
                    want[1]
                );
            }
            assert!(expected.iter().any(|f| f[0].abs() > 0.01));
        }
    }
}

// --- A grain's pitch (E63) -------------------------------------------------------------------

/// The frequency of the strongest spectral peak within 1.5% of `around_hz`, and its log
/// magnitude: a Hann-windowed DFT scanned in 0.1 Hz steps, refined by a parabola through the log
/// magnitudes (as `crates/macro-osc/tests/voice_pitch.rs` measures).
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

/// One grain, a fifth up over a recorded 440 Hz tone, sounds at 440 Hz times the firmware's speed
/// at 44.1, 48 and 96 kHz for every quality, within 0.05 cent: the codec rate stays the module's
/// whatever the session rate. The firmware's speed is its table's ratio for +7 semitones truncated
/// to its 22.10 fixed-point grain increment (1534/1024, 0.30 cent under the equal-tempered fifth). Dirty tape plays through the module's
/// tape wow and flutter (a delay of `1 + (105·wow + 17·flutter)·amount` codec samples, wow at 0.23 Hz
/// and flutter at 1.99 Hz): its pitch wobbles by up to about 13 cents around the grain's (5.4 from
/// the wow, 7.6 from the flutter), so it is held within 15 cents of the expected pitch and within
/// 0.5 cent of what it measures at 48 kHz (the press lands on the same codec frame at every rate).
#[test]
fn a_single_grain_plays_its_pitch_at_every_session_rate() {
    let ratio = f64::from(transit_texture::stmlib::semitones_to_ratio(transit_texture::float::F(7.0)).0);
    let want = 440.0 * (ratio * 1024.0).trunc() / 1024.0;
    assert_eq!((ratio * 1024.0).trunc(), 1534.0);
    let mut worst = 0.0f64;
    let mut tape = Vec::new();
    for sr in [44_100, 48_000, 96_000] {
        for (quality, media) in Media::ALL.into_iter().enumerate() {
            let rate = f64::from(sr);
            // No grains until the seed press: the latch off and the density at noon (taps only).
            let mut t = device(
                sr,
                &[
                    ("quality", quality as f32),
                    ("dry_wet", 100.0),
                    ("seed_latch", 0.0),
                    ("density", 50.0),
                    ("size", 90.0),
                    ("time", 20.0),
                    ("pitch", 7.0),
                    ("input_gain", 1.0),
                ],
            );
            // Time 20% is 0.8 s back at the digital quality, 3.2 s at dirty tape (a buffer of 4 s
            // holds 16 s of 8-bit audio at 24 kHz): five seconds of tone fill that before the press.
            let frames = (rate * 6.5) as usize;
            let tone: Vec<f32> = (0..frames).map(|i| 0.4 * (TAU * 440.0 * i as f64 / rate).sin() as f32).collect();
            // 5 s exactly: a boundary of 100-frame host blocks at every rate here.
            let press = sr as usize * 5;
            let y = render(&mut t, &[tone.clone(), tone], 100, |f, t| {
                if f == press {
                    t.set_param("seed", 1.0);
                } else if f == press + 2000 {
                    t.set_param("seed", 0.0);
                }
            });
            // The grain lasts about 1.5 s (Size 90%); measure 0.25 s to 0.75 s into it.
            let from = press + t.latency() + (rate * 0.25) as usize;
            let window: Vec<f64> = y[0][from..from + (rate * 0.5) as usize].iter().map(|&v| f64::from(v)).collect();
            assert!(window.iter().any(|v| v.abs() > 0.02), "{sr} Hz {media:?}: no grain");
            let label = format!("{sr} Hz {media:?}");
            let hz = pitch_hz(&window, rate, want, &label);
            let cents = 1200.0 * (hz / want).log2();
            println!("{label}: {hz:.4} Hz against {want:.4} ({cents:+.4} cents)");
            if media == Media::DirtyTape {
                assert!(cents.abs() < 15.0, "{label}: {hz} Hz against {want} Hz ({cents:+.3} cents)");
                tape.push((sr, hz));
            } else {
                assert!(cents.abs() < 0.05, "{label}: {hz} Hz against {want} Hz ({cents:+.3} cents)");
                worst = worst.max(cents.abs());
            }
        }
    }
    let at48 = tape.iter().find(|(sr, _)| *sr == 48_000).map_or(want, |t| t.1);
    for (sr, hz) in tape {
        let cents = 1200.0 * (hz / at48).log2();
        assert!(cents.abs() < 0.5, "dirty tape at {sr} Hz: {hz} Hz against {at48} Hz at 48 kHz ({cents:+.3} cents)");
    }
    println!("worst {worst:.4} cents (dirty tape aside)");
}

/// The measurement refuses an octave error: a tone at the expected pitch's half (with nothing at
/// the expected pitch) and one at its double both fail.
#[test]
fn the_pitch_measurement_refuses_octave_aliases() {
    let rate = 48_000.0;
    for f in [330.0, 1320.0] {
        let x: Vec<f64> = (0..24_000).map(|i| (TAU * f * i as f64 / rate).sin() + 0.2 * (TAU * 660.0 * i as f64 / rate).sin()).collect();
        let refused = std::panic::catch_unwind(|| pitch_hz(&x, rate, 660.0, "alias"));
        assert!(refused.is_err(), "{f} Hz with a weak 660 Hz partial passed as 660 Hz");
    }
    let clean: Vec<f64> = (0..24_000).map(|i| (TAU * 660.0 * i as f64 / rate).sin()).collect();
    assert!((pitch_hz(&clean, rate, 660.0, "clean") - 660.0).abs() < 0.01);
}

// --- Reconfiguration -------------------------------------------------------------------------

/// A buffer change fades the output out over 10 ms, rebuilds the chain in place at the bottom of
/// the fade (exactly silent there), and fades back in; a quality change also moves the codec rate
/// and the latency. Both land at the same frames whatever the host block size.
#[test]
fn a_reconfiguration_fades_out_restarts_and_fades_back_in() {
    let sr = 44_100;
    let x = signal(sr, 65_536);
    let change = 16_384;
    let fade = ms_to_samples(RESTART_FADE_MS, sr as f32) as usize;
    let edit = |f: usize, t: &mut TextureFx| {
        if f == change {
            assert!(t.set_param("buffer", 4.0));
        } else if f == 2 * change {
            assert!(t.set_param("quality", 0.0));
        }
    };
    let mut a = device(sr, &[("dry_wet", 30.0)]);
    let digital = a.latency();
    let ya = render(&mut a, &x, 64, edit);
    let mut b = device(sr, &[("dry_wet", 30.0)]);
    let yb = render(&mut b, &x, 512, edit);
    assert!(same_bits(&ya, &yb));
    assert_eq!(a.texture.buffer_size(), BufferSize::MAX);
    assert_eq!((a.texture.media(), a.codec_rate()), (Media::DirtyTape, 24_000));
    assert_ne!(a.latency(), digital, "the dirty tape's path is longer");
    for restart in [change + fade, 2 * change + fade] {
        // Fading out: the level falls; at the restart it is exactly zero.
        assert_eq!((ya[0][restart], ya[1][restart]), (0.0, 0.0), "silent at the restart frame {restart}");
        let before: f32 = ya[0][restart - fade..restart - fade + 64].iter().map(|v| v.abs()).sum();
        assert!(before > 0.0, "sound before the fade");
    }
    // After the fade-in the device plays again.
    assert!(ya[0][change + 3 * fade..2 * change].iter().any(|v| v.abs() > 0.01));
    assert!(ya[0][2 * change + 3 * fade..].iter().any(|v| v.abs() > 0.01));
}

/// A session's stored values reach the device on its first block, after `prepare`: a new setup
/// then applies at once, with no fade, and the render is the one of a device built with them.
#[test]
fn a_setup_given_before_the_first_block_applies_without_a_fade() {
    let params = [("quality", 1.0), ("buffer", 2.0), ("mono_input", 1.0), ("dry_wet", 80.0)];
    let x = signal(44_100, 20_000);
    let mut before = device(44_100, &params);
    let mut after = device(44_100, &[]);
    for (id, v) in params {
        assert!(after.set_param(id, v));
    }
    let a = render(&mut before, &x, 128, |_, _| {});
    let b = render(&mut after, &x, 128, |_, _| {});
    assert!(same_bits(&a, &b));
    assert_eq!((after.texture.media(), after.texture.buffer_size().seconds(), after.texture.mono()), (Media::CleanTape, 6.0, true));
}

#[test]
fn the_manual_gain_setting_inverts_the_firmware_curve() {
    assert_eq!(manual_gain_setting(0.0), 1, "the lowest setting: unity within a part in 10^8");
    assert_eq!(manual_gain_setting(32.0), 255);
    for db in [6.0f32, 12.0, 20.0, 30.0] {
        let s = f32::from(manual_gain_setting(db)) / 255.0;
        let g = 1.0 + 39.0 * s.powi(4);
        assert!((20.0 * g.log10() - db).abs() < 0.3, "{db} dB -> setting {s}: {} dB", 20.0 * g.log10());
    }
}

#[test]
fn a_mono_strip_hears_the_left_input_and_gets_the_two_outputs_averaged() {
    let mut t = TextureFx::new();
    t.prepare(48_000.0, 256, 1);
    let mut io = vec![vec![0.0f32; 256]];
    io[0][10] = 0.5;
    let mut out = Vec::new();
    for _ in 0..4 {
        t.process(&mut io, 256);
        out.extend_from_slice(&io[0]);
        io[0].fill(0.0);
    }
    assert!(out.iter().all(|v| v.is_finite()));
    let peak = (0..out.len()).max_by(|&i, &j| out[i].abs().total_cmp(&out[j].abs())).unwrap_or(0);
    assert_eq!(peak, 10 + t.latency());
}

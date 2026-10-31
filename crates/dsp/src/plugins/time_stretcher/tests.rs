//! The Time Stretcher insert (#199): host-block independence at every rate, the latency it reports
//! against the delay it has, the firmware-exact path at 32 kHz against the crate driven directly, a
//! shifted sine's pitch at every session rate (E63), the quality restart, and the edges.

use super::*;
use crate::Plugin;
use std::f64::consts::TAU;

const RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

fn device(sr: u32, params: &[(&str, f32)]) -> TimeStretcher {
    let mut t = TimeStretcher::new();
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
fn render(t: &mut TimeStretcher, input: &[Vec<f32>; 2], block: usize, mut at: impl FnMut(usize, &mut TimeStretcher)) -> [Vec<f32>; 2] {
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

/// Parameter changes at frames that are block boundaries of both splits: freeze, a trigger press
/// and release, a pitch and position move, the window, the tone.
fn script(frame: usize, t: &mut TimeStretcher) {
    match frame {
        32_768 => assert!(t.set_param("pitch", -5.0)),
        65_536 => assert!(t.set_param("freeze", 1.0)),
        73_728 => {
            assert!(t.set_param("trigger", 1.0));
            assert!(t.set_param("position", 60.0));
        }
        77_824 => assert!(t.set_param("trigger", 0.0)),
        98_304 => {
            assert!(t.set_param("freeze", 0.0));
            assert!(t.set_param("window", 30.0));
            assert!(t.set_param("tone", 20.0));
            assert!(t.set_param("fine", 37.0));
        }
        _ => {}
    }
}

#[test]
fn host_block_size_changes_no_bit_at_every_rate_and_quality() {
    for sr in [44_100, 48_000, 96_000] {
        for quality in 0..4 {
            let params = [("quality", quality as f32), ("dry_wet", 70.0), ("feedback", 35.0), ("reverb", 30.0), ("diffusion", 40.0), ("pitch", 7.0)];
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

/// The 32 kHz path with a pass-through in place of the firmware delays an impulse by exactly the
/// latency it reports, at every session rate, and never reads ahead of the firmware; the table is
/// the one the help and ADR 0033 state.
#[test]
fn the_path_delays_an_impulse_by_its_reported_latency() {
    let kernel = Kernel::new();
    let mut table = Vec::new();
    for sr in [32_000].into_iter().chain(RATES).chain([22_050, 176_400, 192_000, 44_056]) {
        let mut path = Path::new(&kernel, sr, CODEC_RATE, MAX_BLOCK_SIZE);
        let at = 3000;
        let y: Vec<f32> =
            (0..9000).map(|n| path.tick(&kernel, if n == at { [0.5, -0.5] } else { [0.0; 2] }, &mut |i, o| o.copy_from_slice(i))[0]).collect();
        let peak = (0..y.len()).max_by(|&i, &j| y[i].abs().total_cmp(&y[j].abs())).unwrap_or(0);
        assert_eq!(peak, at + path.latency(), "{sr} Hz");
        let gain = (f64::from(CODEC_RATE) / f64::from(sr)).min(1.0) as f32;
        assert!(y[peak] > 0.4 * gain, "{sr} Hz: peak {}", y[peak]);
        assert_eq!(path.starved(), 0, "{sr} Hz");
        println!("{sr} Hz: latency {} samples ({:.3} ms)", path.latency(), path.latency() as f64 * 1000.0 / f64::from(sr));
        table.push((sr, path.latency()));
    }
    let stated = [(32_000, 31), (44_100, 138), (48_000, 151), (88_200, 276), (96_000, 301)];
    for (sr, latency) in stated {
        assert!(table.contains(&(sr, latency)), "{sr} Hz: the stated latency {latency} is not the measured one ({table:?})");
    }
}

/// The dry path (dry/wet 0, no reverb or feedback) delays an impulse by the latency the device
/// reports and leaves it at its input level (the output makeup), at every rate and quality.
#[test]
fn the_reported_latency_is_the_measured_impulse_delay() {
    let at = 4000;
    for sr in RATES {
        for quality in 0..4 {
            let mut t = device(sr, &[("quality", quality as f32), ("dry_wet", 0.0)]);
            let mut x = [vec![0.0f32; 16_000], vec![0.0f32; 16_000]];
            x[0][at] = 0.25;
            x[1][at] = -0.25;
            let y = render(&mut t, &x, 333, |_, _| {});
            let peak = (0..y[0].len()).max_by(|&i, &j| y[0][i].abs().total_cmp(&y[0][j].abs())).unwrap_or(0);
            assert_eq!(peak, at + t.latency(), "{sr} Hz quality {quality}");
            assert!(y[0][peak] > 0.0 && y[1][peak] < 0.0, "{sr} Hz quality {quality}: the impulse came through");
            assert_eq!(t.starved(), 0);
        }
    }
    // At 32 kHz nothing is converted: the impulse comes out whole, at its level within the
    // clipper's curve and the int16 steps.
    let mut t = device(32_000, &[("dry_wet", 0.0)]);
    let mut x = [vec![0.0f32; 8000], vec![0.0f32; 8000]];
    x[0][at] = 0.25;
    x[1][at] = 0.25;
    let y = render(&mut t, &x, 100, |_, _| {});
    assert_eq!(t.latency(), 31);
    let v = y[0][at + 31];
    assert!((v - 0.25).abs() < 0.002, "the dry level: {v}");
}

/// A 32 kHz session converts nothing: the device's output, 31 frames late (the block less one), is
/// the crate driven directly in the firmware's 32-frame blocks times the output makeup, bit for bit,
/// through parameter changes and a trigger.
#[test]
fn at_32_khz_the_device_is_the_crate_driven_directly() {
    for host_block in [64, 500] {
        let params = [("dry_wet", 60.0), ("feedback", 30.0), ("reverb", 25.0), ("pitch", -7.0), ("diffusion", 50.0), ("tone", 35.0)];
        let mut dev = device(32_000, &params);
        let frames = 96_000;
        let x = signal(32_000, frames);
        let change = 48_000;
        let y = render(&mut dev, &x, host_block, |f, t| {
            if f == change {
                assert!(t.set_param("freeze", 1.0));
                assert!(t.set_param("trigger", 1.0));
                assert!(t.set_param("input_level", 6.0));
            }
        });
        let mut reference = TimeStretcher::new();
        for (id, v) in params {
            reference.set_param(id, v);
        }
        let mut granular = Granular::new_in_mode(BufferSize::FIRMWARE, Quality::Stereo, PlaybackMode::Stretch);
        let mut expected = vec![[0.0f32; 2]; frames];
        for (k, out) in expected.chunks_mut(MAX_BLOCK_SIZE).enumerate() {
            let start = k * MAX_BLOCK_SIZE;
            let after = start >= change;
            let gain = if after { 10f32.powf(6.0 / 20.0) } else { 1.0 };
            let input: Vec<Frame> = (start..start + out.len()).map(|n| [x[0][n], x[1][n]].map(|s| to_codec(s * gain))).collect();
            let mut p = reference.parameters();
            p.freeze = after;
            p.trigger = after && start < change + MAX_BLOCK_SIZE;
            let mut codec = vec![[0i16; 2]; out.len()];
            granular.process(&p, &input, &mut codec);
            for (o, c) in out.iter_mut().zip(&codec) {
                *o = c.map(|s| f32::from(s) / 32_768.0 * OUTPUT_GAIN);
            }
        }
        let late = 31;
        for n in 0..frames {
            let want = if n < late { [0.0; 2] } else { expected[n - late] };
            assert!(
                y[0][n].to_bits() == want[0].to_bits() && y[1][n].to_bits() == want[1].to_bits(),
                "{host_block}-frame blocks: frame {n} is ({}, {}), the crate gives ({}, {})",
                y[0][n],
                y[1][n],
                want[0],
                want[1]
            );
        }
        assert!(expected.iter().any(|f| f[0].abs() > 0.01));
    }
}

// --- Pitch (E63) -----------------------------------------------------------------------------

/// The frequency of the strongest spectral peak within 1.5% of `around_hz`, and its log
/// magnitude: a Hann-windowed DFT scanned in 0.1 Hz steps, refined by a parabola through the log
/// magnitudes (as the Texture's and the Macro Oscillator's pitch tests measure).
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

/// A 440 Hz sine shifted by +12 semitones sounds at 880 Hz within 1 cent at 44.1, 48 and 96 kHz
/// for every quality (the firmware's ratio for 12 semitones is exactly 2, and its windows read at
/// 2.0 in 16.16 fixed point): the codec rate stays the module's 32 kHz whatever the session rate.
/// -12 and +7 are measured too, against the firmware's table ratio: each window reads at the ratio,
/// but each new window starts where the correlator's search, on a grid of 1.25 to 2 samples (more
/// when transposing up), found the best match, so the splices shift the phase and the measured peak
/// sits a few cents off when a period of the shifted tone is not a whole number of the grid's
/// steps. Those are held within 5 cents of the ratio and within 0.5 cent of what they measure at
/// 48 kHz (the firmware runs at 32 kHz at every session rate).
#[test]
fn a_shifted_sine_plays_its_pitch_at_every_session_rate() {
    let mut worst = 0.0f64;
    let mut others = Vec::new();
    for sr in [44_100, 48_000, 96_000] {
        for quality in 0..4 {
            for semitones in [12.0f32, -12.0, 7.0] {
                let rate = f64::from(sr);
                let ratio = f64::from(transit_granular::stmlib::semitones_to_ratio(transit_granular::float::F(semitones)).0);
                let want = 440.0 * ratio;
                let mut t = device(sr, &[("quality", quality as f32), ("pitch", semitones), ("window", 80.0)]);
                let frames = (rate * 2.0) as usize;
                let tone: Vec<f32> = (0..frames).map(|i| 0.25 * (TAU * 440.0 * i as f64 / rate).sin() as f32).collect();
                let y = render(&mut t, &[tone.clone(), tone], 256, |_, _| {});
                // After a second (the pitch slews by at most an octave per window), half a second.
                let from = sr as usize;
                let window: Vec<f64> = y[0][from..from + sr as usize / 2].iter().map(|&v| f64::from(v)).collect();
                assert!(window.iter().any(|v| v.abs() > 0.05), "{sr} Hz quality {quality} {semitones:+}: silent");
                let label = format!("{sr} Hz quality {quality} {semitones:+} st");
                let hz = pitch_hz(&window, rate, want, &label);
                let cents = 1200.0 * (hz / want).log2();
                println!("{label}: {hz:.4} Hz against {want:.4} ({cents:+.4} cents)");
                #[allow(clippy::float_cmp)] // Exact constants.
                if semitones == 12.0 {
                    assert!(cents.abs() < 1.0, "{label}: {hz} Hz against {want} Hz ({cents:+.3} cents)");
                    worst = worst.max(cents.abs());
                } else {
                    assert!(cents.abs() < 5.0, "{label}: {hz} Hz against {want} Hz ({cents:+.3} cents)");
                    others.push((sr, quality, semitones.to_bits(), hz));
                }
            }
        }
    }
    for &(sr, quality, semitones, hz) in &others {
        let at48 = others.iter().find(|o| o.0 == 48_000 && o.1 == quality && o.2 == semitones).map_or(hz, |o| o.3);
        let cents = 1200.0 * (hz / at48).log2();
        assert!(cents.abs() < 0.5, "{sr} Hz quality {quality}: {hz} Hz against {at48} Hz at 48 kHz ({cents:+.3} cents)");
    }
    println!("+12: worst {worst:.4} cents");
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

// --- Reconfiguration -------------------------------------------------------------------------

/// A quality change fades the output out over 10 ms, restarts the firmware at the bottom of the fade
/// (exactly silent there), and fades back in; it lands at the same frames whatever the host block
/// size, and the latency stays.
#[test]
fn a_quality_change_fades_out_restarts_and_fades_back_in() {
    let sr = 44_100;
    let x = signal(sr, 65_536);
    let change = 16_384;
    let fade = ms_to_samples(RESTART_FADE_MS, sr as f32) as usize;
    let edit = |f: usize, t: &mut TimeStretcher| {
        if f == change {
            assert!(t.set_param("quality", 3.0));
        } else if f == 2 * change {
            assert!(t.set_param("quality", 1.0));
        }
    };
    let mut a = device(sr, &[("dry_wet", 30.0)]);
    let latency = a.latency();
    let ya = render(&mut a, &x, 64, edit);
    let mut b = device(sr, &[("dry_wet", 30.0)]);
    let yb = render(&mut b, &x, 512, edit);
    assert!(same_bits(&ya, &yb));
    assert_eq!((a.granular.quality(), a.latency()), (Quality::Mono, latency));
    for restart in [change + fade, 2 * change + fade] {
        assert_eq!((ya[0][restart], ya[1][restart]), (0.0, 0.0), "silent at the restart frame {restart}");
        let before: f32 = ya[0][restart - fade..restart - fade + 64].iter().map(|v| v.abs()).sum();
        assert!(before > 0.0, "sound before the fade");
    }
    assert!(ya[0][change + 3 * fade..2 * change].iter().any(|v| v.abs() > 0.01));
    assert!(ya[0][2 * change + 3 * fade..].iter().any(|v| v.abs() > 0.01));
}

/// A quality that comes back to the running one inside the fade cancels the restart: the output
/// dips and returns, the recording stays, and once the fade is over the output is the untouched
/// device's again, bit for bit.
#[test]
fn a_quality_returned_inside_the_fade_cancels_the_restart() {
    let sr = 44_100;
    let x = signal(sr, 49_152);
    let change = 16_384;
    let back = change + 192;
    let fade = ms_to_samples(RESTART_FADE_MS, sr as f32) as usize;
    let tail = |y: &[Vec<f32>; 2], from: usize, to: usize| [y[0][from..to].to_vec(), y[1][from..to].to_vec()];
    let mut a = device(sr, &[("dry_wet", 40.0), ("feedback", 30.0)]);
    let ya = render(&mut a, &x, 64, |_, _| {});
    let mut b = device(sr, &[("dry_wet", 40.0), ("feedback", 30.0)]);
    let yb = render(&mut b, &x, 64, |f, t| {
        if f == change {
            assert!(t.set_param("quality", 2.0));
        } else if f == back {
            assert!(t.set_param("quality", 0.0));
        }
    });
    let end = x[0].len();
    assert!(same_bits(&tail(&ya, 0, change), &tail(&yb, 0, change)));
    let dip = (change..change + 2 * fade).filter(|&n| ya[0][n].abs() > 0.01).map(|n| yb[0][n].abs() / ya[0][n].abs()).fold(1.0f32, f32::min);
    assert!(dip < 0.9 && dip > 0.0, "the output dips ({dip}) and never reaches the restart's silence");
    let after = change + 2 * fade;
    assert!(same_bits(&tail(&ya, after, end), &tail(&yb, after, end)), "the firmware restarted");
    assert_eq!((b.quality, b.fade), (a.quality, Fade::Steady));
}

/// A session's stored values reach the device on its first block, after `prepare`: a new quality
/// then applies at once, with no fade, and the render is the one of a device built with it.
#[test]
fn a_quality_given_before_the_first_block_applies_without_a_fade() {
    let params = [("quality", 2.0), ("dry_wet", 80.0), ("pitch", 5.0)];
    let x = signal(44_100, 20_000);
    let mut before = device(44_100, &params);
    let mut after = device(44_100, &[]);
    for (id, v) in params {
        assert!(after.set_param(id, v));
    }
    let a = render(&mut before, &x, 128, |_, _| {});
    let b = render(&mut after, &x, 128, |_, _| {});
    assert!(same_bits(&a, &b));
    assert_eq!(after.granular.quality(), Quality::StereoLowFidelity);
}

/// `reset` returns the device to a fresh one: the same render after a dirty run.
#[test]
fn a_reset_device_renders_what_a_fresh_one_renders() {
    let params = [("dry_wet", 70.0), ("feedback", 50.0), ("reverb", 40.0), ("pitch", 12.0), ("quality", 1.0)];
    let x = signal(48_000, 30_000);
    let mut dirty = device(48_000, &params);
    let _ = render(&mut dirty, &x, 256, |f, t| {
        if f == 10_240 {
            t.set_param("freeze", 1.0);
            t.set_param("trigger", 1.0);
        }
    });
    dirty.set_param("freeze", 0.0);
    dirty.set_param("trigger", 0.0);
    dirty.reset();
    let mut fresh = device(48_000, &params);
    let a = render(&mut dirty, &x, 256, |_, _| {});
    let b = render(&mut fresh, &x, 256, |_, _| {});
    assert!(same_bits(&a, &b));
}

/// Sixty seconds of silence after a loud run come out with no subnormal sample, through the
/// converters (44.1 and 96 kHz) and without them (32 kHz), and through a restart's fade over the
/// quiet tail.
#[test]
fn silence_after_a_loud_run_has_no_subnormal_output() {
    for (sr, quality) in [(44_100u32, 0.0), (96_000, 3.0), (32_000, 1.0)] {
        let mut t = device(sr, &[("quality", quality), ("dry_wet", 60.0), ("feedback", 40.0), ("reverb", 50.0), ("diffusion", 50.0)]);
        let loud = signal(sr, sr as usize);
        let y = render(&mut t, &loud, 256, |_, _| {});
        assert!(y[0].iter().any(|v| v.abs() > 0.05), "{sr} Hz quality {quality}: loud");
        let mut io = vec![vec![0.0f32; 4096]; 2];
        let mut subnormal = 0usize;
        let blocks = 60 * sr as usize / 4096;
        for k in 0..blocks {
            if k == blocks / 2 {
                assert!(t.set_param("quality", 2.0));
            }
            io.iter_mut().for_each(|c| c.fill(0.0));
            t.process(&mut io, 4096);
            subnormal += io.iter().flatten().filter(|v| **v != 0.0 && !v.is_normal()).count();
        }
        assert_eq!(subnormal, 0, "{sr} Hz quality {quality}");
    }
}

/// The trigger is the rising edge of the gate, once per firmware block: holding it fires once.
#[test]
fn holding_the_trigger_fires_once() {
    let x = signal(48_000, 48_000);
    let freeze = |f: usize, t: &mut TimeStretcher| {
        if f == 32_768 {
            t.set_param("freeze", 1.0);
        }
    };
    let press = |held: usize| {
        let mut t = device(48_000, &[("position", 20.0)]);
        render(&mut t, &x, 128, |f, t| {
            freeze(f, t);
            if f == 36_864 {
                t.set_param("trigger", 1.0);
            } else if f == 36_864 + held {
                t.set_param("trigger", 0.0);
            }
        })
    };
    // Held for 1024 or 8192 frames: the same single trigger, so the same output.
    assert!(same_bits(&press(1024), &press(8192)));
    let mut t = device(48_000, &[("position", 20.0)]);
    let untouched = render(&mut t, &x, 128, freeze);
    assert!(untouched[0][36_864..].iter().any(|v| v.abs() > 0.01), "the frozen recording plays");
    assert!(!same_bits(&press(1024), &untouched), "the trigger moves the read");
}

#[test]
fn a_mono_strip_hears_the_left_input_and_gets_the_two_outputs_averaged() {
    let mut t = TimeStretcher::new();
    t.set_param("dry_wet", 0.0);
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

#[test]
fn hostile_input_stays_finite() {
    let mut t = device(44_100, &[("feedback", 100.0), ("reverb", 100.0), ("input_level", 18.0)]);
    let mut io = vec![vec![0.0f32; 512]; 2];
    for k in 0..200 {
        for (n, v) in io.iter_mut().flatten().enumerate() {
            *v = match (k + n) % 5 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => 4.0,
                3 => -4.0,
                _ => 0.0,
            };
        }
        t.process(&mut io, 512);
        assert!(io.iter().flatten().all(|v| v.is_finite() && v.abs() <= OUTPUT_GAIN), "block {k}");
    }
}

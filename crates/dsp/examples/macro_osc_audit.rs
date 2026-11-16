//! Offline signal measurements; run with `cargo run --release --locked -p transit-dsp --example macro_osc_audit`.
//! No audio/MIDI devices, product-stage access, or real-time assertions.
use std::error::Error;
use std::f64::consts::TAU;
use transit_dsp::Plugin;
use transit_dsp::plugins::macro_osc::{MODELS, MacroOscillator};
use transit_math::{f64 as m, fft};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const HOST: usize = 128;
const N: usize = 131_072;
const MOD_N: usize = 524_288;
const KEYS: [u8; 5] = [24, 48, 72, 96, 127];
const REPRESENTATIVES: [usize; 3] = [8, 10, 19];

fn db(power: f64) -> f64 {
    10.0 * m::log10(power)
}

fn ideal(key: u8) -> f64 {
    440.0 * m::exp2((f64::from(key) - 69.0) / 12.0)
}

// Eligibility describes these two knob settings, not every sound the model can make.
fn exclusion(model: usize, bright: bool) -> Option<&'static str> {
    match model {
        0 if bright => Some("wave/sub mixture; subharmonics"),
        2..=4 => Some("patch-dependent six-operator ratios, detuning and envelopes"),
        6 | 7 | 14 => Some("multiple chord pitches / arpeggiation"),
        10 => Some("noninteger FM ratio (default 12.16 semitones); feedback at maximum"),
        15 => Some("speech/formants with possible unvoiced content"),
        16 => Some("detuned stochastic grains"),
        17 | 18 | 22 | 23 => Some("noise or inharmonic percussion"),
        19 | 20 => Some("inharmonic physical modes"),
        21 => Some("pitch envelope and nonlinear percussion"),
        _ => None,
    }
}

fn set(p: &mut MacroOscillator, name: &str, value: f32) -> Result<()> {
    if !p.set_param(name, value) {
        return Err(format!("unknown parameter {name}").into());
    }
    Ok(())
}

fn render(model: usize, sr: u32, keys: &[u8], extra: &[(&str, f32)], skip: usize, len: usize) -> Result<Vec<f64>> {
    let mut p = MacroOscillator::new();
    set(&mut p, "model", model as f32)?;
    for &(name, value) in extra {
        set(&mut p, name, value)?;
    }
    p.prepare(sr as f32, HOST, 2);
    for &key in keys {
        p.note_on(0, key, 127);
    }
    let mut io = vec![vec![0.0; HOST]; 2];
    let mut out = Vec::with_capacity(len);
    // Always drive full 128-frame calls, including the last call. Capture by sample index.
    for start in (0..skip + len).step_by(HOST) {
        for ch in &mut io {
            ch.fill(0.0);
        }
        p.process(&mut io, HOST);
        for (i, &v) in io[0].iter().enumerate() {
            if !v.is_finite() || !io[1][i].is_finite() {
                return Err(format!("nonfinite output: model {model}, rate {sr}, frame {}", start + i).into());
            }
            if (skip..skip + len).contains(&(start + i)) {
                out.push(f64::from(v));
            }
        }
    }
    Ok(out)
}

struct Spectrum {
    power: Vec<f64>,
    bin: f64,
}

impl Spectrum {
    fn new(x: &[f64], sr: u32) -> Result<Self> {
        let n = x.len();
        let mut table = vec![fft::Complex::default(); n / 2];
        fft::twiddles::<f64>(&mut table).map_err(|e| e.to_string())?;
        let mut windowed = Vec::with_capacity(n);
        let mut w2 = 0.0;
        for (i, &y) in x.iter().enumerate() {
            let a = TAU * i as f64 / n as f64;
            let w = 0.35875 - 0.48829 * m::cos(a) + 0.14128 * m::cos(2.0 * a) - 0.01168 * m::cos(3.0 * a);
            w2 += w * w;
            windowed.push(y * w);
        }
        let mut bins = vec![fft::Complex::default(); n / 2 + 1];
        fft::forward_real(&table, &windowed, &mut bins).map_err(|e| e.to_string())?;
        let power = bins
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let sides = if i == 0 || i == n / 2 { 1.0 } else { 2.0 };
                sides * (c.re * c.re + c.im * c.im) / (n as f64 * w2)
            })
            .collect();
        Ok(Self { power, bin: f64::from(sr) / n as f64 })
    }

    fn frequency(&self, i: usize) -> f64 {
        if i == 0 || i + 1 >= self.power.len() {
            return i as f64 * self.bin;
        }
        let a = m::ln(self.power[i - 1].max(1e-300));
        let b = m::ln(self.power[i].max(1e-300));
        let c = m::ln(self.power[i + 1].max(1e-300));
        let denominator = a - 2.0 * b + c;
        let offset = if denominator.abs() > 1e-20 { (0.5 * (a - c) / denominator).clamp(-0.5, 0.5) } else { 0.0 };
        (i as f64 + offset) * self.bin
    }

    fn at(&self, hz: f64) -> usize {
        (m::round(hz / self.bin) as usize).min(self.power.len() - 1)
    }

    fn band(&self, hz: f64, bins: usize) -> f64 {
        let i = self.at(hz);
        self.power[i.saturating_sub(bins)..=(i + bins).min(self.power.len() - 1)].iter().sum()
    }

    fn fundamental(&self, expected: f64) -> (f64, f64) {
        let lo = self.at(expected * 0.97).max(1);
        let hi = self.at(expected * 1.03).max(lo);
        let i = (lo..=hi).max_by(|&a, &b| self.power[a].total_cmp(&self.power[b])).unwrap_or(lo);
        let power = self.band(i as f64 * self.bin, 4);
        (if power > 0.0 { self.frequency(i) } else { f64::NAN }, power)
    }

    fn weak_reference(&self, power: f64) -> bool {
        power <= 1e-12 || power <= self.power.iter().sum::<f64>() * 1e-8
    }

    fn harmonic(&self, hz: f64, f0: f64, guard: f64) -> bool {
        let k = m::round(hz / f0).max(1.0);
        (hz - k * f0).abs() <= (guard * self.bin).max(0.001 * k * f0)
    }

    fn residual(&self, f0: f64, guard: f64) -> f64 {
        self.power.iter().enumerate().skip(6).filter(|(i, _)| !self.harmonic(*i as f64 * self.bin, f0, guard)).map(|(_, p)| p).sum()
    }

    // Peak centers only: no calling the skirt of an excluded tone a separate component.
    fn strongest(&self, accept: impl Fn(f64) -> bool) -> (f64, f64) {
        let mut best = (f64::NAN, 0.0);
        for i in 2..self.power.len() - 1 {
            if self.power[i] <= 0.0 || self.power[i] < self.power[i - 1] || self.power[i] < self.power[i + 1] {
                continue;
            }
            let hz = self.frequency(i);
            if !(20.0..20_000.0).contains(&hz) || !accept(hz) {
                continue;
            }
            // A sidelobe outside a guard must never integrate back into the excluded main lobe.
            // Apply the same selection to every integrated bin, including the audible-band limit.
            let power = (i.saturating_sub(4)..=(i + 4).min(self.power.len() - 1))
                .filter(|&j| {
                    let f = j as f64 * self.bin;
                    (20.0..20_000.0).contains(&f) && accept(f)
                })
                .map(|j| self.power[j])
                .sum();
            if power > best.1 {
                best = (hz, power);
            }
        }
        best
    }
}

fn bright_params(bright: bool) -> Vec<(&'static str, f32)> {
    if bright { vec![("harmonics", 100.0), ("timbre", 100.0), ("morph", 100.0)] } else { vec![] }
}

fn noise_section(models: &[usize], sr: u32) -> Result<()> {
    println!("\n### Residual and level scaling, {sr} Hz\n");
    println!(
        "| Model | Reference | f0 Hz | Fundamental dBFS | Residual dBc | Residual dBFS | Low fundamental dBFS | Low residual dBc | Low residual dBFS | Residual change dB | Guard 5->8 change dB |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for &model in models {
        let x = render(model, sr, &[60], &[], sr as usize, N)?;
        let s = Spectrum::new(&x, sr)?;
        let (f0, fundamental) = s.fundamental(ideal(60));
        let weak = s.weak_reference(fundamental);
        let mask_f0 = if weak { ideal(60) } else { f0 };
        let rest = s.residual(mask_f0, 5.0);
        let low = render(model, sr, &[60], &[("level", -46.0)], sr as usize, N)?;
        let low = Spectrum::new(&low, sr)?;
        let (_, low_fundamental) = low.fundamental(ideal(60));
        let low_rest = low.residual(mask_f0, 5.0);
        let reference = if weak {
            "missing/weak fundamental proxy"
        } else if exclusion(model, false).is_some() {
            "nominal-band proxy"
        } else {
            "tonal candidate"
        };
        println!(
            "| {} | {reference} | {f0:.3} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.3} | {:.3} |",
            MODELS[model],
            db(fundamental),
            db(rest / fundamental),
            db(rest),
            db(low_fundamental),
            db(low_rest / low_fundamental),
            db(low_rest),
            db(low_rest / rest),
            db(s.residual(mask_f0, 8.0) / rest)
        );
    }
    Ok(())
}

fn alias_section(models: &[usize], sr: u32) -> Result<()> {
    println!("\n### Keyboard / tuning, {sr} Hz\n");
    println!("| Model | Knobs | MIDI key | Ideal Hz | Candidate Hz | Cents | Reference dBFS | Strongest outside harmonics dBc | Hz | Status |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---|");
    for &model in models {
        for bright in [false, true] {
            for key in KEYS {
                let x = render(model, sr, &[key], &bright_params(bright), sr as usize, N)?;
                let s = Spectrum::new(&x, sr)?;
                let expected = ideal(key);
                // Source-derived search centers, not substituted measurements: the voice caps
                // pitch at key 120, and Grain's carrier is further capped at one eighth of sr.
                let center = ideal(key.min(120));
                let center = if model == 11 { center.min(f64::from(sr) / 8.0) } else { center };
                let (f0, fundamental) = s.fundamental(center);
                let cents = 1200.0 * m::log2(f0 / expected);
                let knobs = if bright { "maximum" } else { "default" };
                let weak = s.weak_reference(fundamental);
                let status =
                    if weak { "missing/weak fundamental; no ratio" } else { exclusion(model, bright).unwrap_or("candidate, not proven alias") };
                let (level, hz) = if exclusion(model, bright).is_none() && !weak {
                    let (hz, power) = s.strongest(|f| !s.harmonic(f, f0, 5.0));
                    (format!("{:.2}", db(power / fundamental)), format!("{hz:.2}"))
                } else {
                    ("n/a".into(), "n/a".into())
                };
                println!(
                    "| {} | {knobs} | {key} | {expected:.3} | {f0:.3} | {cents:.3} | {:.2} | {level} | {hz} | {status} |",
                    MODELS[model],
                    db(fundamental)
                );
            }
        }
    }
    Ok(())
}

fn probe_params() -> Vec<(&'static str, f32)> {
    // Ratio 1, no phase modulation: a sine-like main output. Retain the public shaper.
    vec![
        ("harmonics", 100.0 * 30.0 / 128.0),
        ("timbre", 0.0),
        ("morph", 50.0),
        ("tune", (12.0 * m::log2(1000.0 / ideal(72))) as f32),
        ("shaper", 1.0),
        ("cutoff", 4000.0),
    ]
}

fn images(s: &Spectrum, carrier: f64, rate: f64) -> [f64; 4] {
    std::array::from_fn(|k| {
        let block_rate = 4000.0 * (k + 1) as f64;
        // Include center, the LFO's first two sidebands, and the window guard.
        let width = 2.0 * rate + 5.0 * s.bin;
        s.strongest(|hz| (hz - (carrier + block_rate)).abs() <= width || (hz - (carrier - block_rate).abs()).abs() <= width).1
    })
}

fn modulation_section() -> Result<()> {
    println!("\n### Modulation stepping, 48000 Hz\n");
    println!(
        "| Destination | Shape | Hz | Amount % | Carrier dBFS | Image 1 dBc | Image 2 dBc | Image 3 dBc | Image 4 dBc | Baseline max image dBc | Off-comb dBc | Off-comb Hz |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (dest, name) in [(1, "Pitch"), (2, "Harmonics"), (3, "Timbre"), (4, "Morph"), (6, "Level"), (14, "Pan (left)"), (7, "Cutoff")] {
        let mut base = probe_params();
        // Ratio and feedback need nonzero FM index to have an effect.
        if dest == 2 || dest == 4 {
            base.push(("timbre", 20.0));
        }
        let baseline = render(10, 48_000, &[72], &base, 48_000, MOD_N)?;
        let baseline = Spectrum::new(&baseline, 48_000)?;
        let (carrier, base_carrier) = baseline.fundamental(1000.0);
        for (shape, rate) in [(0, 1.0), (0, 10.0), (0, 40.0), (3, 40.0)] {
            let mut params = base.clone();
            let amount = if dest == 1 { 0.1 } else { 10.0 };
            params.extend([
                ("mod1_source", 1.0),
                ("mod1_dest", dest as f32),
                ("mod1_amount", amount),
                ("lfo1_rate", rate),
                ("lfo1_shape", shape as f32),
            ]);
            let x = render(10, 48_000, &[72], &params, 48_000, MOD_N)?;
            let s = Spectrum::new(&x, 48_000)?;
            let reference = s.band(carrier, 4);
            let im = images(&s, carrier, f64::from(rate));
            let baseline_max = images(&baseline, carrier, f64::from(rate)).into_iter().fold(0.0, f64::max);
            let (off, hz) = if shape == 3 {
                let (hz, power) = s.strongest(|hz| {
                    let n = m::round((hz - carrier) / f64::from(rate));
                    (hz - carrier - n * f64::from(rate)).abs() > 5.0 * s.bin
                });
                (format!("{:.2}", db(power / reference)), format!("{hz:.2}"))
            } else {
                ("n/a".into(), "n/a".into())
            };
            let shape = if shape == 0 { "sine" } else { "square" };
            println!(
                "| {name} | {shape} | {rate:.0} | {amount:.1} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {off} | {hz} |",
                db(reference),
                db(im[0] / reference),
                db(im[1] / reference),
                db(im[2] / reference),
                db(im[3] / reference),
                db(baseline_max / base_carrier)
            );
        }
    }
    Ok(())
}

fn shaper_section() -> Result<()> {
    println!("\n### Shaper filter and drive, 48000 Hz\n");
    println!("| Filter excitation | Carrier dBFS | Image 1 dBc | Image 2 dBc | Image 3 dBc | Image 4 dBc |");
    println!("|---|---:|---:|---:|---:|---:|");
    for kind in ["static", "envelope", "LFO sine 40 Hz"] {
        let mut params = probe_params();
        params.push(("cutoff", 1000.0));
        if kind == "envelope" {
            params.extend([("filter_env_amount", 50.0), ("filter_attack", 2000.0), ("filter_decay", 2000.0), ("filter_sustain", 0.0)]);
        } else if kind != "static" {
            params.extend([("mod1_source", 1.0), ("mod1_dest", 7.0), ("mod1_amount", 50.0), ("lfo1_rate", 40.0)]);
        }
        let x = render(10, 48_000, &[72], &params, 0, N)?;
        let s = Spectrum::new(&x, 48_000)?;
        let (f0, reference) = s.fundamental(1000.0);
        let im = images(&s, f0, 40.0);
        println!(
            "| {kind} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
            db(reference),
            db(im[0] / reference),
            db(im[1] / reference),
            db(im[2] / reference),
            db(im[3] / reference)
        );
    }
    println!("\n| FM sine, MIDI key | Drive dB | Fundamental dBFS | Non-harmonic dBc | Hz |\n|---:|---:|---:|---:|---:|");
    for key in [72, 96] {
        for drive in [0.0, 24.0] {
            let params = [("harmonics", 100.0 * 30.0 / 128.0), ("timbre", 0.0), ("morph", 50.0), ("shaper", 1.0), ("drive", drive)];
            let x = render(10, 48_000, &[key], &params, 48_000, N)?;
            let s = Spectrum::new(&x, 48_000)?;
            let (f0, reference) = s.fundamental(ideal(key));
            let (hz, power) = s.strongest(|hz| !s.harmonic(hz, f0, 5.0));
            println!("| {key} | {drive:.0} | {:.2} | {:.2} | {hz:.2} |", db(reference), db(power / reference));
        }
    }
    Ok(())
}

fn levels(x: &[f64]) -> (f64, f64, usize) {
    let peak = x.iter().fold(0.0_f64, |a, &b| a.max(b.abs()));
    let power = x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64;
    (db(peak * peak), db(power), x.iter().filter(|v| v.abs() >= 1.0).count())
}

fn headroom_section() -> Result<()> {
    println!("\n### Level and headroom, 48000 Hz\n");
    println!("| Model | Held peak dBFS | Held RMS dBFS | Held full-scale samples | Chord peak dBFS | Chord RMS dBFS | Chord full-scale samples |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for (model, name) in MODELS.iter().enumerate() {
        let x = render(model, 48_000, &[60], &[], 0, N)?;
        let chord = render(model, 48_000, &[48, 52, 55, 59, 60, 64, 67, 71], &[], 0, N)?;
        let (p, r, c) = levels(&x);
        let (cp, cr, cc) = levels(&chord);
        println!("| {name} | {p:.2} | {r:.2} | {c} | {cp:.2} | {cr:.2} | {cc} |");
    }
    Ok(())
}

fn sensitivity_section() -> Result<()> {
    println!("\n### Capture sensitivity (same start, default C4; maximum knobs C7)\n");
    println!("| Model | N | C4 residual dBc | C7 maximum non-harmonic dBc | Hz |\n|---|---:|---:|---:|---:|");
    for model in [1, 5, 8, 9, 11, 12] {
        for len in [N, 2 * N] {
            let x = render(model, 48_000, &[60], &[], 48_000, len)?;
            let s = Spectrum::new(&x, 48_000)?;
            let (f0, p) = s.fundamental(ideal(60));
            let mask_f0 = if s.weak_reference(p) { ideal(60) } else { f0 };
            let rest = db(s.residual(mask_f0, 5.0) / p);
            let x = render(model, 48_000, &[96], &bright_params(true), 48_000, len)?;
            let s = Spectrum::new(&x, 48_000)?;
            let (f0, p) = s.fundamental(ideal(96));
            let (hz, a) = s.strongest(|hz| !s.harmonic(hz, f0, 5.0));
            println!("| {} | {len} | {rest:.2} | {:.2} | {hz:.2} |", MODELS[model], db(a / p));
        }
    }
    Ok(())
}

fn calibration_section() -> Result<()> {
    println!("\n### Analysis calibration (synthetic signals, not instrument measurements)\n");
    println!(
        "Half-peak sine at ideal middle C; then the same sine with rounding at step 1/32768, before a unity output gain. The low row reduces the sine by 40 dB BEFORE rounding. Same FFT and harmonic mask as section1; this contrasts with the instrument's post-conversion level knob."
    );
    println!(
        "\n| Signal | Fundamental dBFS | Residual dBc | Residual dBFS | Frequency error cents | Strongest non-harmonic dBc | Hz |\n|---|---:|---:|---:|---:|---:|---:|"
    );
    for (name, gain, quantize) in [("sine", 0.5, false), ("rounded sine", 0.5, true), ("rounded low sine", 0.005, true)] {
        let x: Vec<_> = (0..N)
            .map(|i| {
                let y = gain * m::sin(TAU * ideal(60) * i as f64 / 48_000.0);
                if quantize { m::round(y * 32768.0) / 32768.0 } else { y }
            })
            .collect();
        let s = Spectrum::new(&x, 48_000)?;
        let (f0, power) = s.fundamental(ideal(60));
        let rest = s.residual(f0, 5.0);
        let (hz, peak) = s.strongest(|hz| !s.harmonic(hz, f0, 5.0));
        println!(
            "| {name} | {:.3} | {:.3} | {:.3} | {:.4} | {:.3} | {hz:.3} |",
            db(power),
            db(rest / power),
            db(rest),
            1200.0 * m::log2(f0 / ideal(60)),
            db(peak / power)
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    println!("## Program output: Macro Oscillator sound-quality audit\n");
    println!(
        "No arguments; fresh instrument per capture, voice seed=0x21+voice index; Plugin::note_on then Plugin::process, stereo, 128-frame calls, left output analyzed. No audio device. Velocity 127; eight voices; main output only; level -6 dB; macros 50%; shaper off unless specified. Scientific note names: C1=24, C3=48, C4/middle C=60, C5=72, C7=96; highest accepted MIDI key=127. A4=440 Hz."
    );
    println!(
        "\nFFT and all analysis transcendental math: transit_math f64. Periodic four-term Blackman-Harris window (0.35875, 0.48829, 0.14128, 0.01168), approximately -92 dB peak sidelobes. N=131072: bin spacing 0.366211 Hz at 48k, 0.336456 Hz at 44.1k, 0.732422 Hz at 96k. Modulation N=524288: 0.091553 Hz at 48k (10.923 s). Main-lobe width is about eight bins, not one bin. Fixed captures, no adaptive shortening."
    );
    println!(
        "\nPower bins = one-sided |FFT|^2/(N*sum(window^2)); RMS dBFS=10 log10(power), so a unit-peak sine is -3.0103 dBFS. Fundamental candidate = strongest bin within +/-3% of the search center; log-parabolic frequency interpolation; its power integrates +/-4 bins. Center is ideal note frequency, except keyboard rows use ideal(min(key,120)) and Grain additionally min(center,sr/8), following source clamps; cents always compare the measured candidate to the requested note. These centers are source-derived, never substituted for a measured frequency. A missing/inharmonic fundamental makes this only a nominal-band proxy, NOT a tuning measurement. A reference at or below -120 dBFS or at least 80 dB below total windowed power is labeled weak: no keyboard ratio; section1 uses the ideal frequency for its mask and labels its ratio a proxy. No denominator floor: -inf means zero power, NaN means undefined ratio/frequency, n/a means excluded. Non-harmonic residual integrates all bins above DC+5 through Nyquist except +/-max(5 bins,0.1% of harmonic Hz) of candidate harmonics. Strongest components use local maxima in 20 Hz..20 kHz, integrated +/-4 bins WITH the same selection/exclusion applied to every bin; frequency is interpolated. This prevents a sidelobe's integral from including the excluded main lobe. Components closer than a main lobe are unresolved. Non-harmonic does NOT establish alias causation."
    );
    println!(
        "\nSection 1: 1 s settling then N samples; level -6 versus -46 dB (after int16 conversion). Low-level comparison cannot isolate quantization. Ideal rounded undithered 16-bit step 1/32768 predicts q^2/12=-101.10 dBFS noise, 98.09 dB SNR for a full-scale sine; after -6 dB gain -107.10 dBFS. Actual truncation is signal-correlated, not white rounding noise; measured residual also includes intended model content and leakage. Guard 5->8 is a same-capture sensitivity check, with the 0.1% floor retained."
    );
    println!(
        "\nSection 2: same settling/capture, defaults versus all three macros at 100% (a reproducible maximum-control stress, not a proven maximum of perceptual brightness; patch/chord/ratio selectors are not monotonic brightness controls). Ineligible settings below have no alias number. Candidate Hz/cents remain a reference-band observation for these settings."
    );
    println!("\n| Model | Harmonic exclusion not valid when |\n|---|---|");
    for (i, name) in MODELS.iter().enumerate() {
        if let Some(reason) = exclusion(i, false).or_else(|| exclusion(i, true)) {
            println!("| {name} | {}{reason} |", if exclusion(i, false).is_none() { "maximum only: " } else { "both: " });
        }
    }
    println!(
        "\nSection 3: FM model, ratio=1 (Harmonics=23.4375%), Timbre=0, Morph=50%, note72 tuned to 1000 Hz; shaper on, cutoff4000 Hz, default filter/envelopes. Harmonics and Morph destinations use Timbre=20% so ratio/feedback modulation has an effect. Matrix row1=LFO1, via Off, linear; pitch depth0.1% (+/-2.4 cents), other depths10%; retriggered sine1/10/40 Hz and square40 Hz. 1 s settling, MOD_N capture. Carrier integrates +/-4 bins around baseline measured frequency. Image k searches local maxima within +/-(2*LFO rate+5 bins) of |carrier +/- k*4000|, k=1..4; baseline maximum over the same bands is printed, no spectral subtraction. Harmonic distortion can coincide with image bands. Off-comb square excludes +/-5 bins of carrier+n*40 (integer n). At 4000/40=100, control images overlap legitimate LFO harmonics: this cannot separate square-shape aliasing. Timbre/ratio/pitch modulation also creates intended sidebands."
    );
    println!(
        "\nSection 4: same FM sine, cutoff1000 Hz; static baseline, envelope +3 octaves attack2000/decay2000 ms/sustain0, or sine40 Hz +/-3 octaves. Capture starts at note-on, N samples; image bands use +/-80 Hz plus 5 bins. Transient/envelope spectra are not isolated stepping. Drive0/24 dB on sine-like FM at MIDI72/96, default cutoff20000, shaper on, 1 s settling, N samples; harmonic exclusion as section2."
    );
    println!(
        "\nSection 5: Virtual analog, FM, Inharmonic string repeat sections1/2 at 44100/96000 Hz. All three host rates are directly supported: code gives zero rate_offset, so this does not exercise the non-native-rate pitch-offset branch. Section6: first N samples from note-on, default settings/full velocity; single60 and chord48,52,55,59,60,64,67,71; sample peak, unwindowed RMS, count abs(sample)>=1 (floating output, not a claim of clipping). No releases/voice stealing measured."
    );
    let models: Vec<_> = (0..MODELS.len()).collect();
    noise_section(&models, 48_000)?;
    alias_section(&models, 48_000)?;
    modulation_section()?;
    shaper_section()?;
    for sr in [44_100, 96_000] {
        noise_section(&REPRESENTATIVES, sr)?;
        alias_section(&REPRESENTATIVES, sr)?;
    }
    headroom_section()?;
    sensitivity_section()?;
    calibration_section()?;
    Ok(())
}

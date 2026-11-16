use super::*;

pub(super) const FLOOR_DB: f64 = -130.0;

// Seven-term cosine window. Keep the coefficients here so the calibration is reproducible.
fn window(i: usize, n: usize) -> f64 {
    const C: [f64; 7] =
        [0.2712203605850388, -0.4334446123274422, 0.21800412, -0.0657853432956061, 0.010761867305342, -0.000770012710581, 0.000013680883059];
    let a = TAU * i as f64 / n as f64;
    C.iter().enumerate().map(|(k, c)| c * m::cos(k as f64 * a)).sum()
}

pub(super) fn shown(power: f64, reference: f64) -> String {
    if !power.is_finite() || !reference.is_finite() || reference <= 0.0 {
        "unresolved".into()
    } else if power <= reference * 1e-13 {
        "below floor".into()
    } else {
        format!("{:.2}", db(power / reference))
    }
}

// Two separately windowed half-record complex demodulations. The phase advance refines
// the FFT estimate without the k-fold bias of a log-parabolic peak in a fold-comb search.
pub(super) fn refine(x: &[f64], sr: u32, guess: f64) -> f64 {
    let half = x.len() / 2;
    let mut z = [(0.0, 0.0); 2];
    for (part, chunk) in x.chunks_exact(half).take(2).enumerate() {
        for (i, &y) in chunk.iter().enumerate() {
            let angle = TAU * guess * (part * half + i) as f64 / f64::from(sr);
            let y = y * window(i, half);
            z[part].0 += y * m::cos(angle);
            z[part].1 -= y * m::sin(angle);
        }
    }
    let re = z[1].0 * z[0].0 + z[1].1 * z[0].1;
    let im = z[1].1 * z[0].0 - z[1].0 * z[0].1;
    guess + m::atan2(im, re) * f64::from(sr) / (TAU * half as f64)
}

pub(super) fn calibration() -> Result<()> {
    println!("\n### 6. Analyzer calibration and comparison model\n");
    println!(
        "Seven-term cosine window, +/-8-bin integration, >=9-bin harmonic guards (also 0.1% of harmonic frequency). One-sided power normalization is |FFT|^2/(N*sum(w^2)), doubled away from DC/Nyquist. Floor policy: -130 dB relative to the strongest harmonic, including numerical zeros. Calibration is printed raw to establish that bound; findings below it say below floor. Frequency uses two half-record phase demodulations after FFT peak location. No zero residual is a finding."
    );
    println!(
        "\n| Signal | N | f0 Hz | Residual dBc (raw calibration) | Strongest non-harmonic dBc (raw calibration) | Error RMS dBFS |\n|---|---:|---:|---:|---:|---:|"
    );
    for n in [N, MOD_N] {
        for quantized in [false, true] {
            let ideal_signal: Vec<_> = (0..n).map(|i| 0.5 * m::sin(TAU * ideal(60) * i as f64 / 48000.0)).collect();
            let x: Vec<_> = ideal_signal.iter().map(|&y| if quantized { (y * 32768.0).trunc() / 32768.0 } else { y }).collect();
            let s = Spectrum::new(&x, 48000)?;
            let f0 = refine(&x, 48000, s.fundamental(ideal(60)).0);
            let p = s.band(f0, 8);
            let (_, peak) = s.strongest(|f| !s.harmonic(f, f0, 9.0));
            let error: f64 = x.iter().zip(&ideal_signal).map(|(a, b)| (a - b) * (a - b)).sum::<f64>() / n as f64;
            println!(
                "| {} | {n} | {f0:.4} | {:.3} | {:.3} | {} |",
                if quantized { "truncated sine" } else { "sine" },
                db(s.residual(f0, 9.0) / p),
                db(peak / p),
                if quantized { format!("{:.3}", db(error)) } else { "below floor".into() }
            );
        }
    }
    println!(
        "Truncation comparison q^2/3, q=1/32768: {:.3} dBFS before instrument Level; {:.3} dBFS after -6 dB. This uniform fractional-error model is not a white-noise assumption; real error is correlated with the signal.",
        db(1.0 / (3.0 * 32768.0 * 32768.0)),
        db(1.0 / (3.0 * 32768.0 * 32768.0)) - 6.0
    );
    Ok(())
}

pub(super) struct Spectrum {
    pub(super) power: Vec<f64>,
    pub(super) bin: f64,
}

impl Spectrum {
    pub(super) fn edge(&self, expected: f64) -> bool {
        let lo = self.at(expected * 0.97).max(1);
        let hi = self.at(expected * 1.03).max(lo);
        let i = (lo..=hi).max_by(|&a, &b| self.power[a].total_cmp(&self.power[b])).unwrap_or(lo);
        i == lo || i == hi
    }

    pub(super) fn tonal(&self, f0: f64) -> f64 {
        self.power.iter().enumerate().skip(10).filter(|(i, _)| self.harmonic(*i as f64 * self.bin, f0, 9.0)).map(|(_, p)| p).sum()
    }

    pub(super) fn strongest_harmonic(&self, f0: f64) -> f64 {
        (1..=(self.power.len() as f64 * self.bin / f0) as usize).map(|h| self.band(h as f64 * f0, 8)).fold(0.0, f64::max)
    }
    pub(super) fn new(x: &[f64], sr: u32) -> Result<Self> {
        let n = x.len();
        let mut table = vec![fft::Complex::default(); n / 2];
        fft::twiddles::<f64>(&mut table).map_err(|e| e.to_string())?;
        let mut windowed = Vec::with_capacity(n);
        let mut w2 = 0.0;
        for (i, &y) in x.iter().enumerate() {
            let w = window(i, n);
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

    pub(super) fn frequency(&self, i: usize) -> f64 {
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

    pub(super) fn at(&self, hz: f64) -> usize {
        (m::round(hz / self.bin) as usize).min(self.power.len() - 1)
    }

    pub(super) fn band(&self, hz: f64, bins: usize) -> f64 {
        let i = self.at(hz);
        self.power[i.saturating_sub(bins)..=(i + bins).min(self.power.len() - 1)].iter().sum()
    }

    pub(super) fn fundamental(&self, expected: f64) -> (f64, f64) {
        let lo = self.at(expected * 0.97).max(1);
        let hi = self.at(expected * 1.03).max(lo);
        let i = (lo..=hi).max_by(|&a, &b| self.power[a].total_cmp(&self.power[b])).unwrap_or(lo);
        let power = self.band(i as f64 * self.bin, 8);
        (if power > 0.0 { self.frequency(i) } else { f64::NAN }, power)
    }

    pub(super) fn weak_reference(&self, power: f64) -> bool {
        power <= 1e-12 || power <= self.power.iter().sum::<f64>() * 1e-8
    }

    pub(super) fn harmonic(&self, hz: f64, f0: f64, guard: f64) -> bool {
        let k = m::round(hz / f0).max(1.0);
        (hz - k * f0).abs() <= (guard.max(9.0) * self.bin).max(0.001 * k * f0)
    }

    pub(super) fn residual(&self, f0: f64, guard: f64) -> f64 {
        self.power.iter().enumerate().skip(10).filter(|(i, _)| !self.harmonic(*i as f64 * self.bin, f0, guard)).map(|(_, p)| p).sum()
    }

    // Peak centers only: no calling the skirt of an excluded tone a separate component.
    pub(super) fn strongest(&self, accept: impl Fn(f64) -> bool) -> (f64, f64) {
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
            let power = (i.saturating_sub(8)..=(i + 8).min(self.power.len() - 1))
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

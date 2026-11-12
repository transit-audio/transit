//! The comparison of decision 2.2: plain bilinear (RBJ cookbook) biquads, prewarped at the band
//! frequency, as the existing EQs design them. Test-only, platform math.

use crate::{BandType, Slope, butterworth};

/// A z-domain biquad `(b0 + b1 z⁻¹ + b2 z⁻²)/(a0 + a1 z⁻¹ + a2 z⁻²)`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Biquad {
    pub(crate) b: [f64; 3],
    pub(crate) a: [f64; 3],
}

impl Biquad {
    pub(crate) fn power(&self, w: f64) -> f64 {
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b[0] + self.b[1] * c1 + self.b[2] * c2;
        let ni = -(self.b[1] * s1 + self.b[2] * s2);
        let dr = self.a[0] + self.a[1] * c1 + self.a[2] * c2;
        let di = -(self.a[1] * s1 + self.a[2] * s2);
        (nr * nr + ni * ni) / (dr * dr + di * di)
    }
}

/// The RBJ design of one band as a cascade.
pub(crate) fn design(kind: BandType, slope: Slope, f0: f64, gain_db: f64, q: f64, fs: f64) -> Vec<Biquad> {
    let w0 = std::f64::consts::TAU * f0 / fs;
    let (cw, sw) = (w0.cos(), w0.sin());
    let a = 10f64.powf(gain_db / 40.0);
    let alpha = |q: f64| sw / (2.0 * q);
    let one = |b: [f64; 3], a: [f64; 3]| vec![Biquad { b, a }];
    match kind {
        BandType::Bell => {
            let al = alpha(q);
            one([1.0 + al * a, -2.0 * cw, 1.0 - al * a], [1.0 + al / a, -2.0 * cw, 1.0 - al / a])
        }
        BandType::Notch => {
            let al = alpha(q);
            one([1.0, -2.0 * cw, 1.0], [1.0 + al, -2.0 * cw, 1.0 - al])
        }
        BandType::BandPass => {
            let al = alpha(q);
            one([al, 0.0, -al], [1.0 + al, -2.0 * cw, 1.0 - al])
        }
        BandType::LowShelf => {
            let sq = 2.0 * a.sqrt() * alpha(q);
            one(
                [a * ((a + 1.0) - (a - 1.0) * cw + sq), 2.0 * a * ((a - 1.0) - (a + 1.0) * cw), a * ((a + 1.0) - (a - 1.0) * cw - sq)],
                [(a + 1.0) + (a - 1.0) * cw + sq, -2.0 * ((a - 1.0) + (a + 1.0) * cw), (a + 1.0) + (a - 1.0) * cw - sq],
            )
        }
        BandType::HighShelf | BandType::TiltShelf => {
            let sq = 2.0 * a.sqrt() * alpha(q);
            let k = if kind == BandType::TiltShelf { 1.0 / a } else { 1.0 };
            one(
                [k * a * ((a + 1.0) + (a - 1.0) * cw + sq), -2.0 * k * a * ((a - 1.0) + (a + 1.0) * cw), k * a * ((a + 1.0) + (a - 1.0) * cw - sq)],
                [(a + 1.0) - (a - 1.0) * cw + sq, 2.0 * ((a - 1.0) - (a + 1.0) * cw), (a + 1.0) - (a - 1.0) * cw - sq],
            )
        }
        BandType::LowCut | BandType::HighCut => {
            let high = kind == BandType::LowCut;
            let mut v: Vec<Biquad> = butterworth::QS[slope.index()].iter().map(|&q| cut_section(w0, q, high)).collect();
            if slope.order() % 2 == 1 {
                let k = (0.5 * w0).tan();
                let a = [1.0, (k - 1.0) / (k + 1.0), 0.0];
                let b = if high { [1.0 / (1.0 + k), -1.0 / (1.0 + k), 0.0] } else { [k / (1.0 + k), k / (1.0 + k), 0.0] };
                v.push(Biquad { b, a });
            }
            v
        }
        BandType::FlatTilt => Vec::new(),
    }
}

/// One RBJ cut section of Q `q`; `high` is a high pass.
pub(crate) fn cut_section(w0: f64, q: f64, high: bool) -> Biquad {
    let (cw, sw) = (w0.cos(), w0.sin());
    let al = sw / (2.0 * q);
    let a = [1.0 + al, -2.0 * cw, 1.0 - al];
    let b = if high { [(1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0] } else { [(1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0] };
    Biquad { b, a }
}

/// The cascade's power at `w` radians per sample.
pub(crate) fn power(v: &[Biquad], w: f64) -> f64 {
    v.iter().map(|b| b.power(w)).product()
}

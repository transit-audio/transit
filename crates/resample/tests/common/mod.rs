//! Helpers the reader's tests share: deterministic noise and an exact reference kernel written apart
//! from the crate's.

#![allow(dead_code)]

/// Deterministic noise in [-1, 1), never -0.0 (a 32-bit LCG; integer arithmetic, the same everywhere).
pub fn noise(seed: u32, n: usize) -> Vec<f32> {
    let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let v = ((s >> 8) as f32) / (1u32 << 23) as f32 - 1.0;
            if v == 0.0 { 0.5 } else { v }
        })
        .collect()
}

/// The exact kernel, written apart from the crate (platform `sin` in binary64, a literal β, an `I0`
/// series summed until its terms vanish), for the reference comparisons.
pub fn exact_kernel(u: f64) -> f64 {
    let a = u.abs();
    if a == 0.0 {
        return 1.0;
    }
    if a >= 16.0 {
        return 0.0;
    }
    let beta = 7.85726_f64;
    let i0 = |z: f64| {
        let mut sum = 1.0_f64;
        let mut term = 1.0_f64;
        let mut k = 1.0_f64;
        loop {
            term *= (z / (2.0 * k)) * (z / (2.0 * k));
            sum += term;
            if term < 1e-22 * sum {
                break sum;
            }
            k += 1.0;
        }
    };
    let x = std::f64::consts::PI * a;
    (x.sin() / x) * i0(beta * (1.0 - (a / 16.0) * (a / 16.0)).sqrt()) / i0(beta)
}

/// The exact reader in binary64: `Σ s_k·h(|x − k|/m)/m` over the taps with the exact kernel.
pub fn exact_read(src: &[f32], x: f64, q: f64) -> f64 {
    let m = q.max(1.0);
    let lo = (x - 16.0 * m).floor() as i64 - 1;
    let hi = (x + 16.0 * m).ceil() as i64 + 1;
    let mut acc = 0.0;
    for k in lo..=hi {
        let s = if k < 0 { 0.0 } else { src.get(k as usize).copied().unwrap_or(0.0) };
        acc += f64::from(s) * exact_kernel((x - k as f64) / m) / m;
    }
    acc
}

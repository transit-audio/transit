//! A double-double reference for 10^(x/20) (about 100 good bits), used to decide which way an `f32`
//! result must round when the `f64` path lands close to a rounding midpoint. Validated against the
//! committed arbitrary-precision grid table by the gain tests.

/// `hi + lo`, with `|lo|` at most half an ulp of `hi`.
#[derive(Clone, Copy, Debug)]
pub struct Dd(pub f64, pub f64);

fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bb = s - a;
    (s, (a - (s - bb)) + (b - bb))
}

fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd(s, b - (s - a))
}

fn two_prod(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    (p, a.mul_add(b, -p))
}

fn add(x: Dd, y: Dd) -> Dd {
    let (s, e) = two_sum(x.0, y.0);
    let (t, f) = two_sum(x.1, y.1);
    let d = quick_two_sum(s, e + t);
    quick_two_sum(d.0, d.1 + f)
}

fn mul(x: Dd, y: Dd) -> Dd {
    let (p, e) = two_prod(x.0, y.0);
    quick_two_sum(p, e + (x.0 * y.1 + x.1 * y.0))
}

fn div_f64(x: Dd, b: f64) -> Dd {
    let q = x.0 / b;
    let (p, e) = two_prod(q, b);
    quick_two_sum(q, (x.0 - p - e + x.1) / b)
}

/// log2(10)/20 and ln 2 as double-doubles (the low parts computed offline with arbitrary precision).
const DB_TO_LOG2: Dd = Dd(0.166_096_404_744_368_13, -8.345_257_784_509_387e-18);
const LN_2: Dd = Dd(core::f64::consts::LN_2, 2.319_046_813_846_299_6e-17);

/// 10^(x/20) for the exact value of `x`, to about 100 bits.
pub fn db_to_gain(x: f32) -> Dd {
    let (p, e) = two_prod(DB_TO_LOG2.0, f64::from(x));
    let t = quick_two_sum(p, e + DB_TO_LOG2.1 * f64::from(x));
    let k = t.0.round();
    let g = mul(add(t, Dd(-k, 0.0)), LN_2);
    let mut term = Dd(1.0, 0.0);
    let mut sum = term;
    for n in 1..=32 {
        term = div_f64(mul(term, g), f64::from(n));
        sum = add(sum, term);
    }
    let scale = f64::from_bits(((k as i64 + 1023) as u64) << 52);
    Dd(sum.0 * scale, sum.1 * scale)
}

/// The sign of `v - m` for a double-double `v` and an `f64` `m` within a factor of two of it.
fn cmp(v: Dd, m: f64) -> f64 {
    (v.0 - m) + v.1
}

/// The correctly rounded (nearest, ties to even) `f32` of a positive normal double-double.
pub fn round_f32(v: Dd) -> f32 {
    let r = v.0 as f32;
    let b = r.to_bits();
    let (lo, hi) = (f32::from_bits(b - 1), f32::from_bits(b + 1));
    let even = |a: f32, c: f32| if a.to_bits() & 1 == 0 { a } else { c };
    let up = cmp(v, (f64::from(r) + f64::from(hi)) / 2.0);
    if up > 0.0 {
        return hi;
    }
    if up == 0.0 {
        return even(r, hi);
    }
    let down = cmp(v, (f64::from(r) + f64::from(lo)) / 2.0);
    if down < 0.0 {
        return lo;
    }
    if down == 0.0 {
        return even(r, lo);
    }
    r
}

/// Distance of a positive `f64` from the nearest `f32` rounding midpoint, relative to its value.
pub fn midpoint_distance(y: f64) -> f64 {
    let r = y as f32;
    let b = r.to_bits();
    let (lo, hi) = (f64::from(f32::from_bits(b - 1)), f64::from(f32::from_bits(b + 1)));
    let rf = f64::from(r);
    (y - (rf + lo) / 2.0).abs().min((y - (rf + hi) / 2.0).abs()) / y
}

//! Argument sets, the output hash and ulp helpers shared by the seam's tests (ADR 0055, 2.3).
//!
//! Every argument is built with exact IEEE operations (`+`, `-`, `*`, `/`, integer bit patterns), so
//! the sets are the same on every platform and the pinned output hashes are portable.

#![allow(dead_code)]

pub mod dd;

/// Points spread over each function's useful domain (ADR 0055, 2.3).
pub const N: usize = 4096;

/// FNV-1a 64 (the existing goldens' hash).
pub struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    /// An `f32` output; every NaN hashes as one canonical pattern, because Rust leaves a NaN's sign and
    /// payload unspecified (x86 and AArch64 produce different default NaNs).
    pub fn f32(&mut self, x: f32) {
        self.bytes(&canon32(x).to_le_bytes());
    }
    pub fn f64(&mut self, x: f64) {
        self.bytes(&canon64(x).to_le_bytes());
    }
}

pub fn canon32(x: f32) -> u32 {
    if x.is_nan() { 0x7fc0_0000 } else { x.to_bits() }
}

pub fn canon64(x: f64) -> u64 {
    if x.is_nan() { 0x7ff8_0000_0000_0000 } else { x.to_bits() }
}

pub fn lin32(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| lo + (hi - lo) * (i as f32 / (n - 1) as f32)).collect()
}

pub fn lin64(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| lo + (hi - lo) * (i as f64 / (n - 1) as f64)).collect()
}

/// `n` values evenly spaced in bit pattern between two positive values: a geometric spread.
pub fn bits32(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    let (a, b) = (u64::from(lo.to_bits()), u64::from(hi.to_bits()));
    (0..n as u64).map(|i| f32::from_bits((a + (b - a) * i / (n as u64 - 1)) as u32)).collect()
}

pub fn bits64(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (a, b) = (u128::from(lo.to_bits()), u128::from(hi.to_bits()));
    (0..n as u128).map(|i| f64::from_bits((a + (b - a) * i / (n as u128 - 1)) as u64)).collect()
}

/// Zeros of both signs, the smallest normal, subnormals, the largest finite values, infinities, NaN
/// and a few small exact values.
pub fn specials32() -> Vec<f32> {
    let mut v = vec![0.0, f32::MIN_POSITIVE, f32::from_bits(1), f32::from_bits(0x007f_ffff), f32::MAX, f32::INFINITY, 1.0, 0.5, 2.0];
    v.extend(v.clone().into_iter().map(|x| -x));
    v.push(f32::NAN);
    v
}

pub fn specials64() -> Vec<f64> {
    let mut v = vec![0.0, f64::MIN_POSITIVE, f64::from_bits(1), f64::from_bits(0x000f_ffff_ffff_ffff), f64::MAX, f64::INFINITY, 1.0, 0.5, 2.0];
    v.extend(v.clone().into_iter().map(|x| -x));
    v.push(f64::NAN);
    v
}

/// The argument set of a one-argument `f32` function: its domain spread plus the specials; `sin`,
/// `cos` and `tan` also get arguments up to 1e6, where range reduction works hardest.
pub fn args32(name: &str) -> Vec<f32> {
    let mut v = match name {
        "sin" | "cos" | "tan" => {
            let mut v = lin32(-100.0, 100.0, N);
            v.extend(bits32(100.0, 1.0e6, 256));
            v
        }
        "exp" => lin32(-104.0, 89.0, N),
        "exp2" => lin32(-150.0, 128.0, N),
        "ln" | "log2" | "log10" | "sqrt" => bits32(f32::MIN_POSITIVE, f32::MAX, N),
        "tanh" => lin32(-12.0, 12.0, N),
        "floor" | "ceil" | "round" | "trunc" => {
            let mut v = lin32(-100.0, 100.0, N);
            v.extend((-64..64).map(|k| k as f32 + 0.5));
            v
        }
        _ => panic!("no argument set for {name}"),
    };
    v.extend(specials32());
    v
}

pub fn args64(name: &str) -> Vec<f64> {
    let mut v = match name {
        "sin" | "cos" | "tan" => {
            let mut v = lin64(-100.0, 100.0, N);
            v.extend(bits64(100.0, 1.0e6, 256));
            v
        }
        "exp" => lin64(-746.0, 710.0, N),
        "exp2" => lin64(-1075.0, 1024.0, N),
        "ln" | "log2" | "log10" | "sqrt" => bits64(f64::MIN_POSITIVE, f64::MAX, N),
        "floor" | "ceil" | "round" | "trunc" => {
            let mut v = lin64(-100.0, 100.0, N);
            v.extend((-64..64).map(|k| k as f64 + 0.5));
            v
        }
        _ => panic!("no argument set for {name}"),
    };
    v.extend(specials64());
    v
}

fn grid<T: Copy>(a: &[T], b: &[T]) -> Vec<(T, T)> {
    a.iter().flat_map(|&x| b.iter().map(move |&y| (x, y))).collect()
}

/// The argument pairs of a two-argument `f32` function (`powf`: base, exponent; `atan2`: y, x): a 64 by
/// 64 grid plus every pair of specials.
pub fn pairs32(name: &str) -> Vec<(f32, f32)> {
    let mut v = match name {
        "powf" => grid(&bits32(1.0e-3, 1.0e3, 64), &lin32(-8.0, 8.0, 64)),
        "atan2" => grid(&lin32(-10.0, 10.0, 64), &lin32(-10.0, 10.0, 64)),
        _ => panic!("no argument set for {name}"),
    };
    v.extend(grid(&specials32(), &specials32()));
    v
}

pub fn pairs64(name: &str) -> Vec<(f64, f64)> {
    let mut v = match name {
        "pow" => grid(&bits64(1.0e-3, 1.0e3, 64), &lin64(-8.0, 8.0, 64)),
        "atan2" => grid(&lin64(-10.0, 10.0, 64), &lin64(-10.0, 10.0, 64)),
        _ => panic!("no argument set for {name}"),
    };
    v.extend(grid(&specials64(), &specials64()));
    v
}

/// `mul_add` arguments: a 16-cubed grid plus the specials in every position.
pub fn triples32() -> Vec<(f32, f32, f32)> {
    let l = lin32(-10.0, 10.0, 16);
    let mut v: Vec<_> = grid(&l, &l).into_iter().flat_map(|(a, b)| l.iter().map(move |&c| (a, b, c))).collect();
    let s = specials32();
    v.extend(grid(&s, &s).into_iter().flat_map(|(a, b)| s.iter().map(move |&c| (a, b, c))));
    v
}

pub fn triples64() -> Vec<(f64, f64, f64)> {
    let l = lin64(-10.0, 10.0, 16);
    let mut v: Vec<_> = grid(&l, &l).into_iter().flat_map(|(a, b)| l.iter().map(move |&c| (a, b, c))).collect();
    let s = specials64();
    v.extend(grid(&s, &s).into_iter().flat_map(|(a, b)| s.iter().map(move |&c| (a, b, c))));
    v
}

/// The spacing of `f32` values at the magnitude of `y` (2^-149 below the smallest normal).
pub fn ulp32(y: f64) -> f64 {
    let a = y.abs();
    if a < f64::from(f32::MIN_POSITIVE) {
        return f64::from(f32::from_bits(1));
    }
    let e = ((a.to_bits() >> 52) & 0x7ff) as i64 - 1023;
    f64::from_bits(((e - 23 + 1023) as u64) << 52)
}

/// Bits as a monotonically ordered integer, so the difference of two values counts the floats between.
pub fn ord32(x: f32) -> i64 {
    let b = x.to_bits();
    if b >> 31 == 1 { -i64::from(b & 0x7fff_ffff) } else { i64::from(b) }
}

pub fn ord64(x: f64) -> i128 {
    let b = x.to_bits();
    if b >> 63 == 1 { -i128::from(b & 0x7fff_ffff_ffff_ffff) } else { i128::from(b) }
}

/// Parses whitespace-separated hexadecimal `u32` values, skipping `#` comment lines and any line
/// whose first word is not hexadecimal (block headers).
pub fn hex_words(text: &str) -> Vec<Vec<u32>> {
    let mut blocks = vec![Vec::new()];
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        if line.starts_with("law ") {
            blocks.push(Vec::new());
            continue;
        }
        let block = blocks.last_mut().unwrap();
        block.extend(line.split_whitespace().map(|w| u32::from_str_radix(w, 16).expect("hex word")));
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

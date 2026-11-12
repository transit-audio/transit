//! Gain match (ADR 0054 decision 7): computed from the curve, never measured from the audio.
//!
//! `G_match = −10·log10(P)` dB, clamped to ±12 dB, with `P` the weighted mean power of the
//! prototype composite over the 1,914 frequencies `f_i = 20·2^(i/192)` (20 Hz to 19.97 kHz), the
//! weights `|K(f_i)|²` the pinned literals of [`crate::weights`]. With placed bands `P` is the mean
//! of the channel curves' powers. The prototype is a rational function, so the work calls no
//! transcendental beyond `exp2` (the grid and the gains) and `log2` (the power to dB).
//!
//! On the processor's clock the composite is kept per point: each 16-sample control tick evaluates
//! `120/m` points of one stale band's table from the targets captured at the start of its refresh,
//! then those points of the four channel composites and their weighted partial sums. Stale bands are
//! refreshed one at a time in band-number order starting after the band just refreshed (round robin);
//! a refresh, once started, completes. The sum is kept as fixed 30-point partials added in a fixed
//! order, so [`gain_match_db`] reproduces the scheduled value bit for bit.

use alloc::vec::Vec;

use crate::design::Used;
use crate::math;
use crate::prototype::Prototype;
use crate::weights::{POINTS, WEIGHTS};
use crate::{BANDS, BandParams, CurveChannel, GAIN_MATCH_MAX_DB, Placement};

/// Grid points evaluated per control tick in the 48 kHz rate class; `120/m` in class `m`.
pub(crate) const SHARE: usize = 120;
/// Points per partial sum (a divisor of every share).
const CHUNK: usize = 30;
const CHUNKS: usize = POINTS.div_ceil(CHUNK);
/// The four channel curves in their fixed order.
const CURVES: [CurveChannel; 4] = [CurveChannel::Left, CurveChannel::Right, CurveChannel::Mid, CurveChannel::Side];

/// The grid frequency `f_i = 20·2^(i/192)` Hz.
fn grid_freq(i: usize) -> f64 {
    20.0 * math::exp2(i as f64 / 192.0)
}

/// What a band contributes, captured from its stored values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Captured {
    proto: Prototype,
    place: Placement,
    contributes: bool,
}

impl Captured {
    fn new(p: &BandParams, fs: f64) -> Self {
        let used = Used::from_stored(p.freq_hz, p.gain_db, p.q, fs);
        let identity = p.kind.identity_at_zero_gain() && used.gain_db == 0.0;
        Self { proto: Prototype::new(p.kind, p.slope, used.f0, used.gain_db, used.q), place: p.place, contributes: p.audible() && !identity }
    }

    #[inline]
    fn power(&self, f: f64) -> f64 {
        if self.contributes { self.proto.power(f) } else { 1.0 }
    }
}

/// The weighted power of a curve from its partials.
fn curve_power(partials: &[f64], weight_total: f64) -> f64 {
    let mut s = 0.0;
    for &p in partials {
        s += p;
    }
    s / weight_total
}

/// The total weight, summed in the partials' order.
fn weight_total() -> f64 {
    let mut total = 0.0;
    for j in 0..CHUNKS {
        let mut s = 0.0;
        for &w in &WEIGHTS[j * CHUNK..((j + 1) * CHUNK).min(POINTS)] {
            s += w;
        }
        total += s;
    }
    total
}

/// The match from the four curve powers: on mono the one channel's (Stereo and Mid bands); with no
/// placed band the single composite; with left/right or mid/side bands the mean of that pair; with
/// bands in both domains the mean of all four.
fn select(p: [f64; 4], captured: &[Captured], channels: usize) -> f64 {
    let placed = |a: Placement, b: Placement| captured.iter().any(|c| c.contributes && (c.place == a || c.place == b));
    let lr = placed(Placement::Left, Placement::Right);
    let ms = placed(Placement::Mid, Placement::Side);
    let power = if channels < 2 {
        p[2]
    } else {
        match (lr, ms) {
            (false, false) => p[0],
            (true, false) => (p[0] + p[1]) * 0.5,
            (false, true) => (p[2] + p[3]) * 0.5,
            (true, true) => (p[0] + p[1] + p[2] + p[3]) * 0.25,
        }
    };
    (-math::power_to_db(power)).clamp(-GAIN_MATCH_MAX_DB, GAIN_MATCH_MAX_DB)
}

/// The gain match in dB of a set of stored bands (up to [`BANDS`]) at `sample_rate` with `channels`
/// channels: what the processor converges to once its refresh is current, bit for bit. The panel calls
/// it on the UI thread (it allocates).
pub fn gain_match_db(bands: &[BandParams], sample_rate: f64, channels: usize) -> f64 {
    let fs = crate::sane_rate(sample_rate);
    let captured: Vec<Captured> = bands.iter().take(BANDS).map(|b| Captured::new(b, fs)).collect();
    let mut values = alloc::vec![0.0; captured.len()];
    let mut totals = [0.0; 4];
    for j in 0..CHUNKS {
        let mut partial = [0.0; 4];
        let lo = j * CHUNK;
        for (i, &w) in WEIGHTS.iter().enumerate().take((lo + CHUNK).min(POINTS)).skip(lo) {
            let f = grid_freq(i);
            for (v, c) in values.iter_mut().zip(&captured) {
                *v = c.power(f);
            }
            for (k, curve) in CURVES.iter().enumerate() {
                let mut prod = 1.0;
                for (v, c) in values.iter().zip(&captured) {
                    if c.contributes && curve.includes(c.place) {
                        prod *= *v;
                    }
                }
                partial[k] += w * prod;
            }
        }
        for k in 0..4 {
            totals[k] += partial[k];
        }
    }
    let wt = weight_total();
    select(totals.map(|t| t / wt), &captured, channels)
}

/// The match of a given composite power per grid point (one curve, stereo), summed as the scheduler
/// sums: the function the examples of the Evidence drive directly.
#[cfg(test)]
pub(crate) fn match_of_powers(power: &[f64]) -> f64 {
    let mut total = 0.0;
    for j in 0..CHUNKS {
        let (lo, hi) = (j * CHUNK, ((j + 1) * CHUNK).min(POINTS));
        let mut s = 0.0;
        for (w, p) in WEIGHTS[lo..hi].iter().zip(&power[lo..hi]) {
            s += w * p;
        }
        total += s;
    }
    (-math::power_to_db(total / weight_total())).clamp(-GAIN_MATCH_MAX_DB, GAIN_MATCH_MAX_DB)
}

/// The grid frequencies, as the scheduler computes them.
#[cfg(test)]
pub(crate) fn grid() -> Vec<f64> {
    (0..POINTS).map(grid_freq).collect()
}

/// The scheduled computation inside the processor.
#[derive(Clone, Debug)]
pub(crate) struct Scheduler {
    freqs: Vec<f64>,
    tables: Vec<f64>,
    captured: [Captured; BANDS],
    pub(crate) stale: [bool; BANDS],
    pub(crate) current: Option<usize>,
    pos: usize,
    last: usize,
    composite: Vec<f64>,
    partials: Vec<f64>,
    weight_total: f64,
    pub(crate) match_db: f64,
}

impl Scheduler {
    pub(crate) fn new() -> Self {
        Self {
            freqs: (0..POINTS).map(grid_freq).collect(),
            tables: alloc::vec![1.0; BANDS * POINTS],
            captured: [IDLE; BANDS],
            stale: [false; BANDS],
            current: None,
            pos: 0,
            last: BANDS - 1,
            composite: alloc::vec![1.0; 4 * POINTS],
            partials: alloc::vec![0.0; 4 * CHUNKS],
            weight_total: weight_total(),
            match_db: 0.0,
        }
    }

    pub(crate) fn mark_stale(&mut self, band: usize) {
        if band < BANDS {
            self.stale[band] = true;
        }
    }

    /// Computes everything at once from the stored values (prepare).
    pub(crate) fn compute_all(&mut self, target: impl Fn(usize) -> BandParams, fs: f64, channels: usize) {
        for b in 0..BANDS {
            self.captured[b] = Captured::new(&target(b), fs);
            self.fill(b, 0, POINTS);
        }
        self.stale = [false; BANDS];
        self.current = None;
        self.pos = 0;
        self.last = BANDS - 1;
        self.update_points(0, POINTS);
        self.match_db = self.compute_match(channels);
    }

    fn fill(&mut self, b: usize, from: usize, to: usize) {
        let c = self.captured[b];
        for (t, &f) in self.tables[b * POINTS + from..b * POINTS + to].iter_mut().zip(&self.freqs[from..to]) {
            *t = c.power(f);
        }
    }

    /// Recomputes the composites at points `from..to` and the partials of the chunks they cover.
    fn update_points(&mut self, from: usize, to: usize) {
        for i in from..to {
            for (k, curve) in CURVES.iter().enumerate() {
                let mut prod = 1.0;
                for (b, c) in self.captured.iter().enumerate() {
                    if c.contributes && curve.includes(c.place) {
                        prod *= self.tables[b * POINTS + i];
                    }
                }
                self.composite[k * POINTS + i] = prod;
            }
        }
        for j in from / CHUNK..to.div_ceil(CHUNK) {
            let (lo, hi) = (j * CHUNK, ((j + 1) * CHUNK).min(POINTS));
            for k in 0..4 {
                let mut s = 0.0;
                for (w, c) in WEIGHTS[lo..hi].iter().zip(&self.composite[k * POINTS + lo..k * POINTS + hi]) {
                    s += w * c;
                }
                self.partials[k * CHUNKS + j] = s;
            }
        }
    }

    fn compute_match(&self, channels: usize) -> f64 {
        let mut p = [0.0; 4];
        for (k, v) in p.iter_mut().enumerate() {
            *v = curve_power(&self.partials[k * CHUNKS..(k + 1) * CHUNKS], self.weight_total);
        }
        select(p, &self.captured, channels)
    }

    /// One control tick: `share` points of the band being refreshed. True when a refresh completed
    /// (the match changed).
    pub(crate) fn tick(&mut self, target: impl Fn(usize) -> BandParams, fs: f64, channels: usize, share: usize) -> bool {
        let b = match self.current {
            Some(b) => b,
            None => {
                let Some(b) = (1..=BANDS).map(|d| (self.last + d) % BANDS).find(|&b| self.stale[b]) else {
                    return false;
                };
                self.stale[b] = false;
                self.captured[b] = Captured::new(&target(b), fs);
                self.current = Some(b);
                self.pos = 0;
                b
            }
        };
        let to = (self.pos + share).min(POINTS);
        self.fill(b, self.pos, to);
        self.update_points(self.pos, to);
        self.pos = to;
        if to == POINTS {
            self.current = None;
            self.last = b;
            self.match_db = self.compute_match(channels);
            return true;
        }
        false
    }

    /// Whether band `b`'s table is current: not stale and not being refreshed.
    #[cfg(test)]
    pub(crate) fn current_for(&self, b: usize) -> bool {
        !self.stale[b] && self.current != Some(b)
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn fingerprint<H: core::hash::Hasher>(&self, h: &mut H) {
        for s in &self.stale {
            h.write_u8(*s as u8);
        }
        h.write_usize(self.current.unwrap_or(usize::MAX));
        h.write_usize(self.pos);
        h.write_usize(self.last);
        h.write_u64(self.match_db.to_bits());
        for v in &self.partials {
            h.write_u64(v.to_bits());
        }
    }
}

/// A slot never captured: unused, contributes nothing.
const IDLE: Captured = Captured { proto: Prototype::IDLE, place: Placement::Stereo, contributes: false };

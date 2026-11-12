//! Smoothing and ramps on the processor's own sample clock (ADR 0054 decision 3).

use crate::math::{self, LOG2_E};

/// The one-pole coefficient of a time constant of `ms` milliseconds at `fs`: `e^(−1/n)` with
/// `n = ms·fs/1000` samples (`transit-dsp`'s `time_coef`), through `exp2`. Below a thousandth of a
/// sample the smoother jumps.
pub(crate) fn time_coef(ms: f64, fs: f64) -> f64 {
    let n = ms * fs / 1000.0;
    if n < 1.0e-3 { 0.0 } else { math::exp2(-LOG2_E / n) }
}

/// A one-pole smoother in its own value with `transit-dsp`'s `Smoother` snap: it lands on the target
/// once within `1e-6·(1 + |target|)` of it or once a step no longer moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Smoother {
    pub(crate) cur: f64,
    pub(crate) target: f64,
    pub(crate) coef: f64,
}

impl Smoother {
    pub(crate) fn new(v: f64) -> Self {
        Self { cur: v, target: v, coef: 0.0 }
    }

    pub(crate) fn snap(&mut self) {
        self.cur = self.target;
    }

    pub(crate) fn settled(&self) -> bool {
        self.cur == self.target
    }

    /// One sample's step.
    #[inline]
    pub(crate) fn step(&mut self) -> f64 {
        if self.cur != self.target {
            let previous = self.cur;
            self.cur = self.target + (self.cur - self.target) * self.coef;
            if (self.cur - self.target).abs() <= 1.0e-6 * (1.0 + self.target.abs()) || self.cur == previous {
                self.cur = self.target;
            }
        }
        self.cur
    }

    /// `n` samples' steps; true when the value moved.
    pub(crate) fn advance(&mut self, n: u64) -> bool {
        let mut moved = false;
        let mut left = n;
        while left > 0 && self.cur != self.target {
            self.step();
            moved = true;
            left -= 1;
        }
        moved
    }
}

/// A band's wet amount `w` (decision 3.2): a linear ramp of `len` samples counted in whole samples,
/// so it lands on exactly 0 and exactly 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Ramp {
    pub(crate) pos: u32,
    /// +1 rising, −1 falling, 0 holding.
    pub(crate) dir: i8,
}

impl Ramp {
    /// `w` now, for a ramp of `len` samples.
    #[inline]
    pub(crate) fn wet(&self, len: u32) -> f64 {
        if self.pos >= len { 1.0 } else { f64::from(self.pos) / f64::from(len) }
    }

    /// Advances one sample.
    #[inline]
    pub(crate) fn step(&mut self, len: u32) {
        match self.dir {
            1 if self.pos < len => self.pos += 1,
            -1 if self.pos > 0 => self.pos -= 1,
            _ => {}
        }
    }

    /// Whether `w` is zero and not rising: the band is silent.
    pub(crate) fn silent(&self) -> bool {
        self.pos == 0 && self.dir <= 0
    }

    /// Whether the ramp is moving.
    pub(crate) fn moving(&self, len: u32) -> bool {
        (self.dir > 0 && self.pos < len) || (self.dir < 0 && self.pos > 0)
    }
}

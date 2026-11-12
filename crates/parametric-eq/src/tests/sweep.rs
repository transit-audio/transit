//! The deviation measure of decision 2.5 and the sweep helpers shared by the matching, shelf and
//! feasibility tests: the designed magnitude (from the coefficients, in f64) against the analog
//! prototype on the 1000-point grid, where the prototype is above a floor.

#![allow(dead_code)]

use super::rbj;
use super::util::{EvalGrid, db};
use crate::design::{self, Design, Used};
use crate::prototype::Prototype;
use crate::{BandType, Slope};

/// One band setting as stored.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Case {
    pub(crate) kind: BandType,
    pub(crate) slope: Slope,
    pub(crate) f0: f64,
    pub(crate) gain: f64,
    pub(crate) q: f64,
}

impl Case {
    pub(crate) fn used(&self, fs: f64) -> Used {
        Used::from_stored(self.f0, self.gain, self.q, fs)
    }

    pub(crate) fn design(&self, fs: f64) -> Design {
        design::design(self.kind, self.slope, self.used(fs), fs)
    }

    pub(crate) fn prototype(&self, fs: f64) -> Prototype {
        let u = self.used(fs);
        Prototype::new(self.kind, self.slope, u.f0, u.gain_db, u.q)
    }
}

/// The largest `|dB(digital) − dB(prototype)|` over the grid where the prototype is above `floor_db`.
pub(crate) fn deviation(grid: &EvalGrid, floor_db: f64, digital: impl Fn(usize) -> f64, proto: impl Fn(f64) -> f64) -> f64 {
    let mut worst: f64 = 0.0;
    for (i, &f) in grid.f.iter().enumerate() {
        let p = db(proto(f));
        if p > floor_db {
            worst = worst.max((db(digital(i)) - p).abs());
        }
    }
    worst
}

/// The matched design's deviation for one case.
pub(crate) fn matched(case: &Case, fs: f64, grid: &EvalGrid, floor_db: f64) -> f64 {
    let d = case.design(fs);
    let proto = case.prototype(fs);
    deviation(grid, floor_db, |i| d.power(grid.sc[i].0, grid.sc[i].1), |f| proto.power(f))
}

/// The bilinear (RBJ) design's deviation for one case.
pub(crate) fn bilinear(case: &Case, fs: f64, grid: &EvalGrid, floor_db: f64) -> f64 {
    let u = case.used(fs);
    let v = rbj::design(case.kind, case.slope, u.f0, u.gain_db, u.q, fs);
    let proto = case.prototype(fs);
    let tau = std::f64::consts::TAU;
    deviation(grid, floor_db, |i| rbj::power(&v, tau * grid.f[i] / fs), |f| proto.power(f))
}

/// The worst of `f` over every case.
pub(crate) fn worst(cases: &[Case], mut f: impl FnMut(&Case) -> f64) -> (f64, Option<Case>) {
    let mut w = (0.0, None);
    for c in cases {
        let d = f(c);
        if d > w.0 {
            w = (d, Some(*c));
        }
    }
    w
}

/// Every combination of the given values.
pub(crate) fn cases(kind: BandType, slopes: &[Slope], freqs: &[f64], gains: &[f64], qs: &[f64]) -> Vec<Case> {
    let mut v = Vec::new();
    for &slope in slopes {
        for &f0 in freqs {
            for &gain in gains {
                for &q in qs {
                    v.push(Case { kind, slope, f0, gain, q });
                }
            }
        }
    }
    v
}

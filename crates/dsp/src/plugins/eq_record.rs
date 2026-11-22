//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state record of the parametric equalizers (one generic type `Eq<S>`, two entries: EQ 7-Band and
//! EQ 1-Band) and its restore.
//!
//! By exhaustive destructuring:
//! - **configuration**: the entry `info`, with what it chooses: the design function `design` and
//!   the output-gain parameter indexes `gain_ids` (both set by `build` from the entry, never
//!   written again); the session rate `sr` (sanitized by `prepare`); the channel count (the length
//!   of `filt`, `sane_channels(channels)` from `prepare`); the number of smoothers (one per
//!   parameter of the entry). The maximum block is written as 0 (`prepare` ignores it); no loaded
//!   content.
//! - **encoded**: the parameter values; every parameter's smoother (current value, target and
//!   coefficient; a toggle's or a choice's smoother is snapped by `update`, so it never ramps, but
//!   its current value is what the design reads); every channel's sections whole
//!   (`Biquad::record_state`: the coefficients in use and both memories; a section whose band is
//!   off holds the identity and memories that keep the input's history); the output gain smoother.
//! - **derived**, re-derived by the restore: `vals`, the smoothed values the last design read.
//!   Every write of a smoother's current value is followed, in the same call, by `redesign`, which
//!   copies every current value into `vals`: `new` (`build`), `prepare` and `reset` snap and
//!   redesign; `process` advances every smoother and redesigns whenever one settles; `update`
//!   moves only targets, except the snapped toggles and choices, and redesigns when one of those
//!   changed. So `vals[i]` is bit for bit `sm[i]`'s current value between calls. It is read by
//!   `update` (a snapped toggle or choice whose value differs from `vals` forces a design), so it
//!   is observable, and the restore writes it from the restored smoothers. Witness: the mode
//!   switch (a pass filter switched on at T, then off: the design that turns it off runs only
//!   because `vals` holds the old value).
//! - **unobservable**: `coeffs`, the designed coefficients. Its one read is in `redesign`, right
//!   after its one write there (`coeffs = design(vals, sr)`, then copied into every channel's
//!   sections): no read precedes a write. Poison witness: [`Eq::poison_unobservable`] after the
//!   restore and after every block.
//!
//! The 32-sample control chunk of `process` restarts at every call (its position is a local), so
//! nothing of it is kept between calls.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl<const S: usize> Eq<S> {
    fn config(&self) -> Config<'static> {
        Config { entry: self.info.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.filt.len() as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Eq { info: _, design: _, gain_ids: _, p, sm, vals: _, coeffs: _, filt, gain, sr: _ } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            sm.iter().for_each(|s| s.fingerprint(w));
            filt.iter().flatten().for_each(|b| b.record_state(w));
            gain.fingerprint(w);
        });
    }

    /// Sets the state [`Eq::record_state`] wrote and re-derives `vals`; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let Eq { info: _, design: _, gain_ids: _, p, sm, vals, coeffs: _, filt, gain, sr: _ } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            for s in sm.iter_mut() {
                s.restore(r)?;
            }
            for b in filt.iter_mut().flatten() {
                b.restore_state(r)?;
            }
            gain.restore(r)?;
            // Derived (module docs): the smoothed values the last design read.
            for (v, s) in vals.iter_mut().zip(sm.iter()) {
                *v = s.value();
            }
            Some(())
        })
    }

    /// Poison witnesses only: overwrites the designed coefficients (unobservable, module docs) with
    /// finite values from `seed`; returns how many it wrote.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        let k = seed as f32;
        for (i, c) in self.coeffs.iter_mut().enumerate() {
            let x = 0.125 + 0.01 * (i as f32 + k);
            *c = Coeffs { b0: x, b1: -x, b2: 0.5 * x, a1: -0.25, a2: x * 0.5 };
        }
        (S * 5) as u64
    }

    /// The witnesses' observations: a parameter's or the output gain's smoother is still moving,
    /// a section holds memory, and the sections in use (not the identity) on the first channel.
    #[must_use]
    pub fn observe(&self) -> EqObserved {
        EqObserved {
            settling: self.sm.iter().any(Smoother::settling) || self.gain.settling(),
            memory: self.filt.iter().flatten().any(Biquad::holds_memory),
            sections: self.filt.first().map_or(0, |f| f.iter().filter(|b| b.coeffs != Coeffs::IDENTITY).count()),
        }
    }
}

/// What the equalizers' transplant witnesses read (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqObserved {
    pub settling: bool,
    pub memory: bool,
    pub sections: usize,
}

//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state record of the Saturator and its restore.
//!
//! By exhaustive destructuring:
//! - **configuration**: the entry; the session rate `sr` (sanitized by `prepare`); the channel
//!   count (the length of `dc` and of `lp`, both `sane_channels(channels)` from `prepare`); each DC
//!   blocker's pole (`DcBlocker::new(10.0, sr)` in `prepare`, a function of the rate alone: nothing
//!   else sets it). The maximum block is written as 0 (`prepare` ignores it); no loaded content.
//!   The Saturator does not oversample: `process` shapes each sample at the session rate.
//! - **encoded**: the parameter values; every channel's DC blocker memories (kept while the
//!   asymmetry is 0, when `process` skips the blocker, and read again when it comes back); every
//!   channel's tone low-pass memory `lp` (kept while the tone is fully open, when `process` skips
//!   the low-pass, and read again when it closes); the four smoothers (drive, asymmetry, mix,
//!   output: current value, target and coefficient).
//! - **derived**, re-derived by the restore: the tone coefficient `tone`, which `update` sets to
//!   `tone_coef(tone parameter, sr)` and nothing else writes; `update` runs in `new`, after every
//!   parameter change (`param_plumbing!`) and in `prepare` after the rate is set, so between calls
//!   it is that function of recorded state and configuration. Witness: the mode switch (the tone
//!   closed at T).
//! - **unobservable**: none.
//!
//! Non-finite input never reaches the state: `process` replaces it with 0 before shaping, and the
//! low-pass and the DC blocker's output memory pass through `flush`.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl Saturator {
    fn config(&self) -> Config<'static> {
        Config { entry: SAT_INFO.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.dc.len() as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Saturator { p, sr: _, dc, lp, drive, bias, mix, out, tone: _ } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            dc.iter().for_each(|d| d.record_memories(w));
            w.f32s(lp);
            for s in [drive, bias, mix, out] {
                s.fingerprint(w);
            }
        });
    }

    /// Sets the state [`Saturator::record_state`] wrote and re-derives the tone coefficient; `None`
    /// on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let Saturator { p, sr, dc, lp, drive, bias, mix, out, tone } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            for d in dc.iter_mut() {
                d.restore_memories(r)?;
            }
            r.f32s(lp)?;
            for s in [&mut *drive, &mut *bias, &mut *mix, &mut *out] {
                s.restore(r)?;
            }
            // Derived (module docs).
            *tone = tone_coef(p.v(2), *sr);
            Some(())
        })
    }

    /// The witnesses' observations: a smoother is still moving, a DC blocker or a tone low-pass
    /// holds memory, and the tone low-pass is in use.
    #[must_use]
    pub fn observe(&self) -> SatObserved {
        SatObserved {
            settling: [&self.drive, &self.bias, &self.mix, &self.out].iter().any(|s| s.settling()),
            dc_memory: self.dc.iter().any(DcBlocker::holds_memory),
            lp_memory: self.lp.iter().any(|&v| v != 0.0),
            tone_on: self.tone > 0.0,
        }
    }
}

/// What the Saturator's transplant witnesses read (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SatObserved {
    pub settling: bool,
    pub dc_memory: bool,
    pub lp_memory: bool,
    pub tone_on: bool,
}

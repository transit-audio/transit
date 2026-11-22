//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state records of Gain and Trim and their restores.
//!
//! Both by exhaustive destructuring:
//! - **configuration**: the entry and the channel count `ch` (`sane_channels`, from `prepare`); the
//!   maximum block is written as 0 (`prepare` ignores it) and the sample rate as 0: neither type
//!   keeps the rate, whose one effect, the smoother's coefficient (`set_time` in `prepare`), is
//!   encoded state; no loaded content.
//! - **encoded**: the parameter values (Gain's gain; Trim's gain and polarity) and the gain smoother
//!   (current value, target and coefficient).
//! - **unobservable** and **derived**: none.

use super::*;
use transit_state_record::{Config, Reader, Writer};

fn config(entry: &'static str, ch: usize) -> Config<'static> {
    Config { entry, sample_rate: 0.0, max_block: 0, channels: ch as u64, content: 0 }
}

impl Gain {
    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Gain { p, ch, g } = self;
        w.record(&config(GAIN_INFO.id, *ch), |w| {
            p.record_state(w);
            g.fingerprint(w);
        });
    }

    /// Sets the state [`Gain::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let Gain { p, ch, g } = self;
        r.record(&config(GAIN_INFO.id, *ch), |r| {
            p.restore_state(r)?;
            g.restore(r)
        })
    }

    /// The witnesses' observation: the smoother is still moving.
    #[must_use]
    pub fn settling(&self) -> bool {
        self.g.settling()
    }
}

impl Trim {
    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Trim { p, ch, g } = self;
        w.record(&config(TRIM_INFO.id, *ch), |w| {
            p.record_state(w);
            g.fingerprint(w);
        });
    }

    /// Sets the state [`Trim::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let Trim { p, ch, g } = self;
        r.record(&config(TRIM_INFO.id, *ch), |r| {
            p.restore_state(r)?;
            g.restore(r)
        })
    }

    /// The witnesses' observations: the smoother is still moving, and its current value.
    #[must_use]
    pub fn settling(&self) -> bool {
        self.g.settling()
    }

    /// The smoother's current value (negative while the polarity is inverted).
    #[must_use]
    pub fn gain(&self) -> f32 {
        self.g.value()
    }
}

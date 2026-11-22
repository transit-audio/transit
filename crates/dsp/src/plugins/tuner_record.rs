//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state record of the Tuner and its restore.
//!
//! By exhaustive destructuring:
//! - **configuration**: the entry. The Tuner keeps neither the rate, nor the maximum block, nor the
//!   channel count (`prepare` ignores all three), so its configuration child writes each as 0; no
//!   loaded content.
//! - **encoded**: the parameter values (the reference pitch and hold). No output depends on them:
//!   `process` leaves every buffer untouched, bit for bit, and reports no latency or tail. They
//!   are state the instance holds (section 12) and the panel reads them through `param`.
//! - **unobservable** and **derived**: none.
//!
//! Outside the instance, and outside this record: the input ring the mix keeps for an insert that
//! asks for one (`Plugin::wants_sample_tap`, a constant `true` here) is the slot's `sample_tap`,
//! which ADR 0029 section 14 classifies as output only (the panels), and the pitch analysis
//! ([`TunerModel`]) runs on the UI thread over that ring. Neither is read back by the instance or
//! by the audio path, so neither can influence a later output sample.

use super::*;
use transit_state_record::{Config, Reader, Writer};

fn config() -> Config<'static> {
    Config { entry: TUNER_INFO.id, sample_rate: 0.0, max_block: 0, channels: 0, content: 0 }
}

impl Tuner {
    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Tuner { p } = self;
        w.record(&config(), |w| p.record_state(w));
    }

    /// Sets the state [`Tuner::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let Tuner { p } = self;
        r.record(&config(), |r| p.restore_state(r))
    }
}

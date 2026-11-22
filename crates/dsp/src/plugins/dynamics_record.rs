//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state records of the Compressor/Limiter and the Expander/Gate and their restores.
//!
//! **Compressor**, by exhaustive destructuring:
//! - **configuration**: the entry, the session rate `sr` (sanitized by `prepare`), the channel count
//!   (the length of `sc` and of `delay`, both `sane_channels(channels)` from `prepare`); the
//!   maximum block is written as 0 (`prepare` ignores it); no loaded content. Each lookahead ring's
//!   length (`prepare`: 10 ms at the rate, rounded up to a power of two) is configuration, checked
//!   on restore.
//! - **encoded**: the parameter values; every channel's sidechain high-pass (coefficients and
//!   memories, `Biquad::record_state`: kept while the high-pass is off, and read again when it
//!   comes back on); every channel's lookahead line whole with its write position; the detector
//!   envelope `gr`; the attack and release coefficients `att`, `rel` and the lookahead in samples
//!   (each set from the parameters by `update`; encoded rather than re-derived); the makeup and mix
//!   smoothers.
//! - **unobservable**: the last block's meter readings `gr_meter` and `det_peak`. `run` writes both
//!   at its end on every call (from `max_gr` and `det_peak`, which start at 0, so a block of no
//!   frames writes 0), and nothing in `run` or `gain_for` reads them; only the meter getters
//!   `gain_reduction_db` and `detector_level_db` do, which feed the panels (the mix's `Strip::gr`
//!   and the scope hub's operating point: output only, ADR 0029 section 14). Poison witness:
//!   [`Compressor::poison_unobservable`] after the restore and after every block.
//! - **derived**: none.
//!
//! **Expander/Gate**, by exhaustive destructuring:
//! - **configuration**: the entry, `sr`, `ch`; maximum block 0; no content. Configuration fields of
//!   the instance: the detector's release coefficient `env_rel`, which `update` sets to
//!   `time_coef(5.0, sr)`, a function of the rate alone (`new` and every `prepare` call `update`;
//!   no parameter reaches it).
//! - **encoded**: the parameter values; the detector envelope `env`; the gain reduction `gr`; the
//!   attack and release coefficients; the hold length in samples and the hold countdown
//!   `hold_left`.
//! - **unobservable**: the last block's meter reading `gr_meter`: `run` writes it at its end on
//!   every call and nothing in `run` or `tick` reads it; only `gain_reduction_db` does (output
//!   only, as the Compressor's). Poison witness: [`ExpanderGate::poison_unobservable`].
//! - **derived**: none.
//!
//! The key input (`process_with_key`) is an input buffer (section 12): both entries detect on it
//! instead of on their input through the same code and keep nothing more from it.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl Compressor {
    fn config(&self) -> Config<'static> {
        Config { entry: COMP_INFO.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.sc.len() as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Compressor { p, sr: _, sc, delay, gr, att, rel, makeup, mix, lookahead, gr_meter: _, det_peak: _ } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            sc.iter().for_each(|b| b.record_state(w));
            delay.iter().for_each(|d| d.fingerprint(w));
            w.f32(*gr);
            w.f32(*att);
            w.f32(*rel);
            makeup.fingerprint(w);
            mix.fingerprint(w);
            w.usize(*lookahead);
        });
    }

    /// Sets the state [`Compressor::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let Compressor { p, sr: _, sc, delay, gr, att, rel, makeup, mix, lookahead, gr_meter: _, det_peak: _ } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            for b in sc.iter_mut() {
                b.restore_state(r)?;
            }
            for d in delay.iter_mut() {
                d.restore(r)?;
            }
            *gr = r.f32()?;
            *att = r.f32()?;
            *rel = r.f32()?;
            makeup.restore(r)?;
            mix.restore(r)?;
            *lookahead = r.usize()?;
            Some(())
        })
    }

    /// Poison witnesses only: overwrites the meter readings (unobservable, module docs) with values
    /// from `seed`; returns how many it wrote.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        self.gr_meter = 7.0 + seed as f32;
        self.det_peak = 0.25 + seed as f32 * 0.5;
        2
    }

    /// The witnesses' observations: the detector envelope, the lookahead in samples, whether the
    /// makeup or the mix smoother is still moving, the largest magnitude in the lookahead lines and
    /// in the sidechain filters' memories.
    #[must_use]
    pub fn observe(&self) -> CompObserved {
        CompObserved {
            gr: self.gr,
            lookahead: self.lookahead,
            settling: self.makeup.settling() || self.mix.settling(),
            line_peak: self.delay.iter().map(crate::util::DelayLine::peak).fold(0.0f32, f32::max),
            sidechain_memory: self.sc.iter().any(Biquad::holds_memory),
        }
    }
}

/// What the Compressor's transplant witnesses read (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompObserved {
    pub gr: f32,
    pub lookahead: usize,
    pub settling: bool,
    pub line_peak: f32,
    pub sidechain_memory: bool,
}

impl ExpanderGate {
    fn config(&self) -> Config<'static> {
        Config { entry: GATE_INFO.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let ExpanderGate { p, sr: _, ch: _, env, env_rel: _, gr, att, rel, hold, hold_left, gr_meter: _ } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            w.f32s(&[*env, *gr, *att, *rel]);
            w.usize(*hold);
            w.usize(*hold_left);
        });
    }

    /// Sets the state [`ExpanderGate::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let ExpanderGate { p, sr: _, ch: _, env, env_rel: _, gr, att, rel, hold, hold_left, gr_meter: _ } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            let mut v = [0.0f32; 4];
            r.f32s(&mut v)?;
            [*env, *gr, *att, *rel] = v;
            *hold = r.usize()?;
            *hold_left = r.usize()?;
            Some(())
        })
    }

    /// Poison witnesses only: overwrites the meter reading (unobservable, module docs) with a value
    /// from `seed`; returns how many it wrote.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        self.gr_meter = 11.0 + seed as f32;
        1
    }

    /// The witnesses' observations: the detector envelope, the gain reduction, the hold length and
    /// the hold countdown.
    #[must_use]
    pub fn observe(&self) -> GateObserved {
        GateObserved { env: self.env, gr: self.gr, hold: self.hold, hold_left: self.hold_left }
    }
}

/// What the Expander/Gate's transplant witnesses read (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateObserved {
    pub env: f32,
    pub gr: f32,
    pub hold: usize,
    pub hold_left: usize,
}

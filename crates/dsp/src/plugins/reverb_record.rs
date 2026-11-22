//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the complete
//! state record of Room Reverb and Plate Reverb (one type, two registry entries) and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the registry entry (`info`, and `cfg`, the
//!   entry's constant line and diffuser table, chosen with it in `build` and never written), the
//!   session rate `sr` (sanitized by `prepare`), the channel count `ch`; the maximum block is
//!   written as 0, since `prepare` ignores it; no loaded content (digest 0). The lengths of every
//!   ring (the pre-delay lines, the diffusers, the feedback lines) are set by `prepare` from the
//!   rate and the entry's table: configuration, checked on restore (each ring's length must be the
//!   receiving instance's).
//! - **encoded**, in the state child, every other field:
//!   - the parameter values;
//!   - both pre-delay lines and all eight feedback lines whole, with their write positions
//!     (`DelayLine::fingerprint`: the whole ring, section 12's rule for delay lines; the ring past
//!     the longest reachable read is not separated out);
//!   - the eight diffusers (allpass sections: every sample of each ring, and its position; every
//!     position is read in turn);
//!   - the damping filters' memories `lp`, the feedback gains `gains` and the damping coefficient
//!     `damp` (both kept between blocks: `gains` follows the smoothed size, which `process`
//!     advances once per block, so it is encoded rather than re-derived);
//!   - the two low-cut filters (coefficients and memories, `Biquad::record_state`);
//!   - the four smoothers (mix, size, pre-delay, width: current value, target and coefficient);
//!   - the modulation LFO's phase.
//! - **unobservable**: none. **Derived**: none (every derived value the instance keeps is encoded).
//!
//! The record's size is dominated by the rings: at 48 kHz the two pre-delay rings are 16,384
//! samples each and the feedback lines 24,576 (Plate) or 34,816 (Room) samples in all (each
//! rounded up to a power of two); two samples to a word.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl Allpass {
    /// The ring whole and its position.
    fn record_state(&self, w: &mut Writer<'_>) {
        let Allpass { buf, pos } = self;
        w.usize(*pos);
        w.f32s(buf);
    }

    /// Sets what [`Allpass::record_state`] wrote (the ring's length is configuration).
    fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let Allpass { buf, pos } = self;
        let p = r.usize()?;
        r.f32s(buf)?;
        if p >= buf.len().max(1) {
            return None;
        }
        *pos = p;
        Some(())
    }
}

impl Reverb {
    fn config(&self) -> Config<'static> {
        Config { entry: self.info.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Reverb {
            // Configuration (module docs).
            info: _,
            cfg: _,
            p,
            sr: _,
            ch: _,
            pre,
            diff,
            lines,
            lp,
            gains,
            damp,
            lowcut,
            mix,
            size,
            pre_ms,
            width,
            lfo,
        } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            pre.iter().for_each(|d| d.fingerprint(w));
            diff.iter().flatten().for_each(|a| a.record_state(w));
            lines.iter().for_each(|d| d.fingerprint(w));
            w.f32s(lp);
            w.f32s(gains);
            w.f32(*damp);
            lowcut.iter().for_each(|b| b.record_state(w));
            for s in [mix, size, pre_ms, width] {
                s.fingerprint(w);
            }
            w.f32(*lfo);
        });
    }

    /// Sets the state [`Reverb::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let Reverb { info: _, cfg: _, p, sr: _, ch: _, pre, diff, lines, lp, gains, damp, lowcut, mix, size, pre_ms, width, lfo } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            for d in pre.iter_mut() {
                d.restore(r)?;
            }
            for a in diff.iter_mut().flatten() {
                a.restore_state(r)?;
            }
            for d in lines.iter_mut() {
                d.restore(r)?;
            }
            r.f32s(lp)?;
            r.f32s(gains)?;
            *damp = r.f32()?;
            for b in lowcut.iter_mut() {
                b.restore_state(r)?;
            }
            for s in [mix, size, pre_ms, width] {
                s.restore(r)?;
            }
            *lfo = r.f32()?;
            Some(())
        })
    }
}

/// What the transplant harness's witnesses read to assert a boundary was reached and an obligation
/// completed (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observed {
    /// Each smoother still moving: mix, size, pre-delay, width.
    pub settling: [bool; 4],
    /// The largest magnitude in the feedback lines and in the pre-delay lines (0 when silent).
    pub lines_peak: f32,
    pub pre_peak: f32,
    /// The modulation LFO's phase.
    pub lfo: f32,
}

impl Reverb {
    /// The witnesses' observations (see [`Observed`]).
    #[must_use]
    pub fn observe(&self) -> Observed {
        let peak = |ls: &[crate::util::DelayLine]| ls.iter().map(crate::util::DelayLine::peak).fold(0.0f32, f32::max);
        Observed {
            settling: [self.mix.settling(), self.size.settling(), self.pre_ms.settling(), self.width.settling()],
            lines_peak: peak(&self.lines),
            pre_peak: peak(&self.pre),
            lfo: self.lfo,
        }
    }
}

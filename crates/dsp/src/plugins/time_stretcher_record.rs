//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the Time
//! Stretcher's complete state record and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the entry, the session rate `sr` (sanitized by
//!   `prepare`), the channel count `ch`; the maximum block is written as 0, since `prepare` ignores
//!   it; no loaded content (digest 0). Configuration fields of the instance: `kernel` (tables
//!   built from constants in `new`, never written), `fade_len` (a function of the session rate, set
//!   by `prepare`), and the path's configuration (`Path::record_state`);
//! - **encoded**, in the state child: the parameter values; the path (both converters, the
//!   gathered codec frames and their count); the firmware's processor (`transit_granular::record`:
//!   every field, the recording whole, frozen or not, which no declared tail covers; each audio
//!   buffer's memory past its live extent left out, `audio_buffer.rs`); the Trigger gate as the
//!   last firmware block saw it; the running and the wanted quality; the restart fade; `idle`.
//!
//! The record's size is dominated by the recording: the firmware's two blocks (118,784 and 65,408
//! bytes) at the stereo qualities record into 65,408 bytes each (16,352 words for both), at the
//! mono ones into 118,784 bytes (14,848 words).

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl TimeStretcher {
    fn config(&self) -> Config<'static> {
        Config { entry: TIME_STRETCHER_INFO.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        use transit_granular::record::Record;
        let TimeStretcher {
            p,
            // Configuration (module docs).
            sr: _,
            ch: _,
            kernel: _,
            path,
            granular,
            last_gate,
            quality,
            want,
            fade,
            fade_len: _,
            idle,
        } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            path.record_state(w);
            granular.record(w);
            w.bool(*last_gate);
            w.usize(quality.index());
            w.usize(want.index());
            match *fade {
                Fade::Steady => w.u8(0),
                Fade::Out(k) => {
                    w.u8(1);
                    w.u32(k);
                }
                Fade::In(k) => {
                    w.u8(2);
                    w.u32(k);
                }
            }
            w.bool(*idle);
        });
    }

    /// Sets the state [`TimeStretcher::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        use transit_granular::record::Record;
        let config = self.config();
        let TimeStretcher { p, sr: _, ch: _, kernel: _, path, granular, last_gate, quality, want, fade, fade_len: _, idle } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            path.restore_state(r)?;
            granular.restore(r)?;
            *last_gate = r.bool()?;
            *quality = Quality::from_index(r.usize()?)?;
            *want = Quality::from_index(r.usize()?)?;
            *fade = match r.u8()? {
                0 => Fade::Steady,
                1 => Fade::Out(r.u32()?),
                2 => Fade::In(r.u32()?),
                _ => return None,
            };
            *idle = r.bool()?;
            Some(())
        })
    }
}

/// What the transplant harness's witnesses read to assert a boundary was reached and an obligation
/// completed (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The quality the firmware runs and the one the parameters ask for.
    pub quality: usize,
    pub want: usize,
    /// The restart fade: 0 steady, 1 fading out, 2 fading in.
    pub fade: u8,
    /// Nothing processed since the last reset.
    pub idle: bool,
    /// Samples recorded into the crossfade tail while frozen (up to 256); the first buffer's write
    /// head; a firmware buffer reset pending.
    pub frozen_tail: i32,
    pub write_head: i32,
    pub reset_pending: bool,
    /// Codec frames gathered toward the next firmware block.
    pub fill: usize,
    /// The Trigger gate as the last firmware block saw it.
    pub last_gate: bool,
}

impl TimeStretcher {
    /// Poison witnesses only: overwrites the processor's buffer memory past its live extents and
    /// the path's `starved` count and converter scratch, from a source seeded with `seed`; returns
    /// the values written.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        let mut p = transit_granular::record::Poison::new(seed);
        self.granular.poison_unobservable(&mut p);
        p.count() + self.path.poison_unobservable(seed)
    }

    /// The witnesses' observations (see [`Observed`]).
    #[must_use]
    pub fn observe(&self) -> Observed {
        let (frozen_tail, write_head, reset_pending) = self.granular.observe();
        Observed {
            quality: self.quality.index(),
            want: self.want.index(),
            fade: match self.fade {
                Fade::Steady => 0,
                Fade::Out(_) => 1,
                Fade::In(_) => 2,
            },
            idle: self.idle,
            frozen_tail,
            write_head,
            reset_pending,
            fill: self.path.fill(),
            last_gate: self.last_gate,
        }
    }
}

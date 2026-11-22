//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the Filter's
//! complete state record and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the entry, the session rate `sr` (sanitized by
//!   `prepare`), the channel count `ch`; the maximum block is written as 0 (`prepare` ignores it);
//!   no loaded content. The number of per-channel filter states (`ch.clamp(1, MAX_CHANNELS)`, from
//!   `prepare`) follows from it and is checked on restore.
//! - **encoded**, in the state child:
//!   - the parameter values;
//!   - the control-rate clock and the redesign flag `dirty`;
//!   - the pending events, `queue[..queued]` with their offsets and kinds (events given before the
//!     next `process`);
//!   - the paraphonic note state (`Notes::record_state`: the entries in use with their table
//!     indexes, the count, the pedal, the last key);
//!   - both envelopes (stage and level) and the triggering velocity;
//!   - the LFO (phase, held step, previous step, generator);
//!   - the follower's level;
//!   - the four smoothers (log2 cutoff, `k`, drive and output: current value, target, coefficient);
//!   - both coefficient sets last designed (kept between blocks: a block reads them until the next
//!     control-rate redesign);
//!   - every channel's two filter sections (the second is kept while the slope is 12 dB and read
//!     again at 24 dB);
//!   - the transport as last told: tempo, bar length, bar origin, the synced LFO's anchor, the
//!     host block position and the expected next one.
//! - **derived** (bit-exact functions of recorded state and configuration, re-derived by restore,
//!   which calls `update` after setting the parameters and the transport, then puts back the
//!   smoothers it set targets on): both envelopes' rates `env_rates` and `amp_rates`, the
//!   follower's attack and release coefficients, the LFO increment `lfo_inc`. Each is written only
//!   by `update`, from the parameters, `sr`, `tempo` and `bar_quarters`, and `update` runs after
//!   every change of any of those (`new`, `set_param`, `prepare`, `set_transport` when the tempo or
//!   the bar changes), so between calls each equals what `update` computes now.
//! - **unobservable**: the queue's entries at and past `queued` (`push` writes an entry before
//!   counting it; `sort_queue` and `process` read only `..queued`; `all_notes_off` and `process` set
//!   `queued` to 0); the fields of a note entry not in use (`Notes::record_state`). Restore leaves
//!   the former as they are and writes the latter free; the poison witness
//!   ([`Filter::poison_unobservable`]) fills both.

use super::*;
use transit_state_record::{Config, Reader, Writer};

fn write_kind(w: &mut Writer<'_>, kind: &Kind) {
    match *kind {
        Kind::On { channel, key, velocity, voice } => {
            w.u8(0);
            w.u8(channel);
            w.u8(key);
            w.u8(velocity);
            w.i32(voice);
        }
        Kind::Off { channel, key, voice } => {
            w.u8(1);
            w.u8(channel);
            w.u8(key);
            w.i32(voice);
        }
        Kind::Pedal(down) => {
            w.u8(2);
            w.bool(down);
        }
        Kind::AllOff => w.u8(3),
    }
}

fn read_kind(r: &mut Reader<'_>) -> Option<Kind> {
    Some(match r.u8()? {
        0 => Kind::On { channel: r.u8()?, key: r.u8()?, velocity: r.u8()?, voice: r.i32()? },
        1 => Kind::Off { channel: r.u8()?, key: r.u8()?, voice: r.i32()? },
        2 => Kind::Pedal(r.bool()?),
        3 => Kind::AllOff,
        _ => return None,
    })
}

impl Filter {
    fn config(&self) -> Config<'static> {
        Config { entry: FILTER_INFO.id, sample_rate: f64::from(self.sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let Filter {
            p,
            // Configuration (module docs).
            sr: _,
            ch: _,
            clock,
            dirty,
            queue,
            queued,
            notes,
            env,
            // Derived (module docs).
            env_rates: _,
            amp,
            amp_rates: _,
            velocity,
            lfo,
            lfo_inc: _,
            follower,
            cutoff,
            k,
            drive,
            output,
            coefs,
            state,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            host_pos,
            next_pos,
        } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            w.u64(*clock);
            w.bool(*dirty);
            w.usize(*queued);
            for t in queue.iter().take(*queued) {
                let Timed { offset, kind } = t;
                w.usize(*offset);
                write_kind(w, kind);
            }
            notes.record_state(w);
            env.record_state(w);
            amp.record_state(w);
            w.u8(*velocity);
            lfo.record_state(w);
            follower.record_state(w);
            for s in [cutoff, k, drive, output] {
                s.fingerprint(w);
            }
            coefs.iter().for_each(|c| c.record_state(w));
            w.usize(state.len());
            for s in state.iter().flatten() {
                s.fingerprint(w);
            }
            w.f64(*tempo);
            w.f64(*bar_quarters);
            w.i64(bar_origin.0);
            w.f64(bar_origin.1);
            w.present(*anchor, |w, (sample, quarters)| {
                w.i64(sample);
                w.f64(quarters);
            });
            w.i64(*host_pos);
            w.i64(*next_pos);
        });
    }

    /// Sets the state [`Filter::record_state`] wrote; `None` on any mismatch. The derived fields
    /// are re-derived (module docs).
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let Filter {
            p,
            sr: _,
            ch: _,
            clock,
            dirty,
            queue,
            queued,
            notes,
            env,
            env_rates: _,
            amp,
            amp_rates: _,
            velocity,
            lfo,
            lfo_inc: _,
            follower,
            cutoff,
            k,
            drive,
            output,
            coefs,
            state,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            host_pos,
            next_pos,
        } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            *clock = r.u64()?;
            *dirty = r.bool()?;
            let n = r.usize()?;
            if n > MAX_QUEUE {
                return None;
            }
            for t in queue.iter_mut().take(n) {
                *t = Timed { offset: r.usize()?, kind: read_kind(r)? };
            }
            *queued = n;
            notes.restore_state(r)?;
            env.restore_state(r)?;
            amp.restore_state(r)?;
            *velocity = r.u8()?;
            lfo.restore_state(r)?;
            follower.restore_state(r)?;
            for s in [&mut *cutoff, &mut *k, &mut *drive, &mut *output] {
                s.restore(r)?;
            }
            for c in coefs.iter_mut() {
                c.restore_state(r)?;
            }
            if r.usize()? != state.len() {
                return None;
            }
            for s in state.iter_mut().flatten() {
                s.restore(r)?;
            }
            *tempo = r.f64()?;
            *bar_quarters = r.f64()?;
            *bar_origin = (r.i64()?, r.f64()?);
            *anchor = r.present(|r| Some((r.i64()?, r.f64()?)))?;
            *host_pos = r.i64()?;
            *next_pos = r.i64()?;
            Some(())
        })?;
        // The derived fields, from the restored parameters and transport; `update` also sets the
        // smoothers' targets, so the restored smoothers go back after it.
        let smoothers = [self.cutoff, self.k, self.drive, self.output];
        self.update();
        [self.cutoff, self.k, self.drive, self.output] = smoothers;
        Some(())
    }

    /// Poison witnesses only: fills the queue's entries at and past `queued` and the fields of every
    /// note entry not in use with values from `seed`; returns how many entries it wrote.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        let mut n = 0;
        for (i, t) in self.queue.iter_mut().enumerate().skip(self.queued) {
            let v = seed.wrapping_mul(0x9E37_79B9).wrapping_add(i as u32);
            *t = Timed {
                offset: (v % 300) as usize,
                kind: Kind::On { channel: (v & 0x0F) as u8, key: (v >> 4) as u8 & 0x7F, velocity: 1 + ((v >> 11) % 127) as u8, voice: -1 },
            };
            n += 1;
        }
        n + self.notes.poison_free(seed)
    }

    /// The witnesses' observations (see [`FilterObserved`]).
    #[must_use]
    pub fn observe(&self) -> FilterObserved {
        let (held, pedal) = self.notes.observe();
        FilterObserved {
            clock: self.clock,
            queued: self.queued,
            held,
            pedal,
            env_stage: self.env.stage as u8,
            amp_stage: self.amp.stage as u8,
            settling: [self.cutoff.settling(), self.k.settling(), self.drive.settling(), self.output.settling()],
            anchored: self.anchor.is_some(),
            second_section: self.state.iter().any(|s| s[1] != Section::default()),
        }
    }
}

/// What the Filter's transplant witnesses read (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterObserved {
    /// Samples since the last reset (the control grid is every [`CONTROL`] of them).
    pub clock: u64,
    /// Events given and not yet processed.
    pub queued: usize,
    /// Note entries in use, and the pedal.
    pub held: usize,
    pub pedal: bool,
    /// The envelopes' stages (0 idle, 1 attack, 2 decay, 3 sustain, 4 release).
    pub env_stage: u8,
    pub amp_stage: u8,
    /// Each smoother still moving: cutoff, `k`, drive, output.
    pub settling: [bool; 4],
    /// The synced LFO holds a transport anchor.
    pub anchored: bool,
    /// A second (24 dB) section holds memory.
    pub second_section: bool,
}

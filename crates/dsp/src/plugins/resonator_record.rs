//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the
//! Resonator's complete state record and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the entry, the host rate `host_sr` (sanitized by
//!   `prepare`), the channel count `ch`; the maximum block is written as 0, since `prepare` ignores
//!   it; no loaded content (digest 0). Configuration fields of the instance: `kernel` (tables
//!   built from constants in `new`, never written), `latency` (a function of the host rate, set
//!   with the converter by `prepare`), and the converter's configuration (`Converter::record_state`);
//! - **encoded**, in the state child: the parameter values; the port's part
//!   (`transit_resonator::record`, which leaves out its block buffers and the state a pending or
//!   configured re-initialization rewrites); the converter's history and counters; the event queue
//!   (each event's offset and kind); the strum ring whole, its start and length; the held, pedalled
//!   and poly-pressure tables; the note pairing; sustain; the newest strum's key and velocity and
//!   the last release velocity; bend, wheel, channel pressure, the rack macros; the model and
//!   polyphony the part was last told; the level smoother; both shapers; `shaped`, `gate`,
//!   `fresh`; the running LFO phases; the tempo, bar length and bar origin; the transport anchor;
//!   the host positions;
//! - **derived**, re-derived by restore from restored state and configuration, never kept from the
//!   receiving instance: `matrix` is `Matrix::new` over the rows the parameter values set and
//!   `settings` is `settings()` over the parameter values, the bar length and the bar origin (the
//!   part's rate is a constant). Both are recomputed whenever one of their inputs changes
//!   (`update` after every parameter set, `prepare`, `set_transport` when the bar changes), so
//!   they always equal that function (the Macro Oscillator's rule). Witness: every transplant case
//!   restores into a fresh instance whose parameters are the defaults, after histories that set
//!   matrix rows and shaper parameters.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl ResonatorInstrument {
    fn config(&self) -> Config<'static> {
        Config { entry: RESONATOR_INFO.id, sample_rate: f64::from(self.host_sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        use transit_resonator::record::Record;
        let ResonatorInstrument {
            p,
            part,
            // Configuration (module docs).
            kernel: _,
            up,
            latency: _,
            host_sr: _,
            ch: _,
            events,
            strums,
            strum_at,
            strum_len,
            held,
            pairing,
            pedalled,
            sustain,
            key,
            velocity,
            release_velocity,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            model,
            polyphony,
            level,
            shapers,
            // Derived (module docs).
            matrix: _,
            settings: _,
            shaped,
            gate,
            fresh,
            free_phase,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            host_pos,
            next_pos,
        } = self;
        w.record(&self.config(), |w| {
            p.record_state(w);
            part.record(w);
            up.record_state(w);
            w.usize(events.len());
            for e in events {
                event_record(e, w);
            }
            for (k, v) in strums {
                w.u8(*k);
                w.f32(*v);
            }
            w.usize(*strum_at);
            w.usize(*strum_len);
            w.bytes(&held.map(u8::from));
            pairing.record_state(w);
            w.bytes(&pedalled.map(u8::from));
            w.bool(*sustain);
            w.u8(*key);
            w.f32s(&[*velocity, *release_velocity, *bend, *wheel]);
            w.u8(*pressure);
            w.bytes(poly);
            w.f32s(macros);
            model.record(w);
            w.usize(*polyphony);
            level.fingerprint(w);
            shapers.iter().for_each(|s| s.record_state(w));
            for b in [shaped, gate, fresh] {
                w.bool(*b);
            }
            free_phase.iter().for_each(|&x| w.f64(x));
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

    /// Sets the state [`ResonatorInstrument::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        use transit_resonator::record::Record;
        let config = self.config();
        let ResonatorInstrument {
            p,
            part,
            kernel: _,
            up,
            latency: _,
            host_sr: _,
            ch: _,
            events,
            strums,
            strum_at,
            strum_len,
            held,
            pairing,
            pedalled,
            sustain,
            key,
            velocity,
            release_velocity,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            model,
            polyphony,
            level,
            shapers,
            matrix: _,
            settings: _,
            shaped,
            gate,
            fresh,
            free_phase,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            host_pos,
            next_pos,
        } = self;
        r.record(&config, |r| {
            p.restore_state(r)?;
            part.restore(r)?;
            up.restore_state(r)?;
            let n = r.usize().filter(|&n| n <= events.capacity())?;
            events.clear();
            for _ in 0..n {
                events.push(event_restore(r)?);
            }
            for (k, v) in strums.iter_mut() {
                *k = r.u8()?;
                *v = r.f32()?;
            }
            *strum_at = r.usize().filter(|&x| x < MAX_STRUMS)?;
            *strum_len = r.usize().filter(|&x| x <= MAX_STRUMS)?;
            let mut bytes = [0u8; 128];
            r.bytes(&mut bytes)?;
            *held = bytes.map(|b| b != 0);
            pairing.restore_state(r)?;
            r.bytes(&mut bytes)?;
            *pedalled = bytes.map(|b| b != 0);
            *sustain = r.bool()?;
            *key = r.u8()?;
            let mut v = [0.0f32; 4];
            r.f32s(&mut v)?;
            [*velocity, *release_velocity, *bend, *wheel] = v;
            *pressure = r.u8()?;
            r.bytes(poly)?;
            r.f32s(macros)?;
            model.restore(r)?;
            *polyphony = r.usize()?;
            level.restore(r)?;
            for s in shapers.iter_mut() {
                s.restore_state(r)?;
            }
            for b in [shaped, gate, fresh] {
                *b = r.bool()?;
            }
            for x in free_phase.iter_mut() {
                *x = r.f64()?;
            }
            *tempo = r.f64()?;
            *bar_quarters = r.f64()?;
            *bar_origin = (r.i64()?, r.f64()?);
            *anchor = r.present(|r| Some((r.i64()?, r.f64()?)))?;
            *host_pos = r.i64()?;
            *next_pos = r.i64()?;
            Some(())
        })?;
        // The derived fields, from the restored state and the configuration (module docs).
        self.matrix = Matrix::new((0..matrix::ROWS).map(|i| self.row(i)));
        self.settings = self.settings();
        Some(())
    }
}

/// A queued event: its offset, then its kind (the variant's position, then its fields).
fn event_record(e: &Event, w: &mut Writer<'_>) {
    let Event { offset, kind } = e;
    w.usize(*offset);
    match *kind {
        Kind::On(key, vel) => {
            w.u8(0);
            w.u8(key);
            w.u8(vel);
        }
        Kind::Off(key, vel) => {
            w.u8(1);
            w.u8(key);
            w.u8(vel);
        }
        Kind::AllOff => w.u8(2),
        Kind::SoundOff => w.u8(3),
        Kind::Sustain(on) => {
            w.u8(4);
            w.bool(on);
        }
        Kind::Bend(b) => {
            w.u8(5);
            w.f32(b);
        }
        Kind::Wheel(v) => {
            w.u8(6);
            w.f32(v);
        }
        Kind::Pressure(v) => {
            w.u8(7);
            w.u8(v);
        }
        Kind::PolyPressure(key, v) => {
            w.u8(8);
            w.u8(key);
            w.u8(v);
        }
        Kind::ResetControllers => w.u8(9),
    }
}

fn event_restore(r: &mut Reader<'_>) -> Option<Event> {
    let offset = r.usize()?;
    let kind = match r.u8()? {
        0 => Kind::On(r.u8()?, r.u8()?),
        1 => Kind::Off(r.u8()?, r.u8()?),
        2 => Kind::AllOff,
        3 => Kind::SoundOff,
        4 => Kind::Sustain(r.bool()?),
        5 => Kind::Bend(r.f32()?),
        6 => Kind::Wheel(r.f32()?),
        7 => Kind::Pressure(r.u8()?),
        8 => Kind::PolyPressure(r.u8()?, r.u8()?),
        9 => Kind::ResetControllers,
        _ => return None,
    };
    Some(Event { offset, kind })
}

/// What the transplant harness's witnesses read to assert a boundary was reached and an obligation
/// completed (test seam, feature `testing`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observed {
    /// The part's configured model, its polyphony, a re-initialization pending, its active voice.
    pub model: ResonatorModel,
    pub polyphony: usize,
    pub dirty: bool,
    pub active_voice: usize,
    /// The output limiter's peak follower (above 1 it turns the gain down).
    pub limiter_peak: f32,
    /// A key is held down; a key is kept by the pedal; the pedal is down.
    pub held: bool,
    pub pedalled: bool,
    pub sustain: bool,
    /// The shapers' envelopes are gated; the shaper shapes the output.
    pub gate: bool,
    pub shaped: bool,
    /// The shapers' amp envelope level.
    pub amp: f32,
    /// Strums waiting for a part block; events queued for a later frame.
    pub strums: usize,
    pub pending: usize,
    /// The level smoother is still moving toward its target.
    pub ramping: bool,
    /// The transport anchor is set.
    pub anchored: bool,
    /// Nothing struck since the last reset.
    pub fresh: bool,
    /// Converter input pushed beyond what has been pulled (part frames rendered ahead).
    pub ahead: i64,
}

impl ResonatorInstrument {
    /// Poison witnesses only: overwrites the part's unobservable state (its block buffers and the
    /// dormant state outside its read set) and the converter's scratch from a source seeded with
    /// `seed`; returns the values written.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        let mut p = transit_resonator::record::Poison::new(seed);
        self.part.poison_unobservable(&mut p);
        p.count() + self.up.poison_unobservable(seed)
    }

    /// The witnesses' observations (see [`Observed`]).
    #[must_use]
    pub fn observe(&self) -> Observed {
        let (model, polyphony, dirty, active_voice, limiter_peak) = self.part.observe();
        Observed {
            model,
            polyphony,
            dirty,
            active_voice,
            limiter_peak,
            held: self.held.iter().any(|h| *h),
            pedalled: self.pedalled.iter().any(|h| *h),
            sustain: self.sustain,
            gate: self.gate,
            shaped: self.shaped,
            amp: self.shapers[0].amp.level,
            strums: self.strum_len,
            pending: self.events.len(),
            ramping: self.level.settling(),
            anchored: self.anchor.is_some(),
            fresh: self.fresh,
            ahead: self.up.ahead(),
        }
    }
}

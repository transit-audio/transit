//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the Modal
//! Voice's complete state record and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the entry, the host rate `host_sr` (sanitized by
//!   `prepare`), the channel count `ch`; the maximum block is written as 0, since `prepare` ignores
//!   it; no loaded content (digest 0). Configuration fields of the instance: `kernel` (tables
//!   built from constants in `new`, never written), `latency` (a function of the host rate, set
//!   with the converter by `prepare`), and the converter's configuration (`Converter::record_state`);
//! - **encoded**, in the state child: the parameter values; the port's part
//!   (`transit_modal_voice`'s record: every field but its scratch buffers); the converter's history
//!   and counters; the event queue (each event's offset and kind); the keys (their order, length,
//!   held and pedalled tables); the note pairing; sustain; `retrigger`, `gate`, `shaper_gate`; the
//!   sounding key and the velocity; the strength and dry share last sent; `latch`; the release
//!   velocity; bend, wheel, channel pressure, poly pressure per key, the rack macros; the model the
//!   part was last told; the level smoother; both shapers; the shaper block's position, matrix sums
//!   and path; `shaped`, `fresh`, `rose`; the running LFO phases; the tempo, bar length and bar
//!   origin; the transport anchor and the block's song position; the host positions;
//! - **derived**, re-derived by restore from restored state and configuration, never kept from the
//!   receiving instance: `matrix` (`Matrix::new` over the rows the parameter values set) and
//!   `settings` (`settings()` over the parameter values, the bar length and the bar origin; the
//!   part's rate is a constant), recomputed whenever an input changes (`update` after every
//!   parameter set, `prepare`, `set_transport` when the bar changes; the Macro Oscillator's rule).
//!   Witness: every transplant case restores into a fresh instance with default parameters, after
//!   histories that set matrix rows and shaper parameters.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl ModalVoiceInstrument {
    fn config(&self) -> Config<'static> {
        Config { entry: MODAL_VOICE_INFO.id, sample_rate: f64::from(self.host_sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        use transit_resonator::record::Record;
        let ModalVoiceInstrument {
            p,
            part,
            // Configuration (module docs).
            kernel: _,
            up,
            latency: _,
            host_sr: _,
            ch: _,
            events,
            keys,
            pairing,
            sustain,
            retrigger,
            gate,
            shaper_gate,
            key,
            velocity,
            strength_sent,
            raw_sent,
            latch,
            release_velocity,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            model,
            level,
            shapers,
            // Derived (module docs).
            matrix: _,
            settings: _,
            shaper_at,
            shaper_sums,
            shaper_path,
            shaped,
            fresh,
            rose,
            free_phase,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            quarters,
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
            let Keys { order, len, held, pedalled } = keys;
            w.bytes(order);
            w.usize(*len);
            w.bytes(&held.map(u8::from));
            w.bytes(&pedalled.map(u8::from));
            pairing.record_state(w);
            for b in [sustain, retrigger, gate, shaper_gate] {
                w.bool(*b);
            }
            w.u8(*key);
            w.f32s(&[*velocity, *strength_sent, *raw_sent]);
            w.bool(*latch);
            w.f32s(&[*release_velocity, *bend, *wheel]);
            w.u8(*pressure);
            w.bytes(poly);
            w.f32s(macros);
            w.usize(*model);
            level.fingerprint(w);
            shapers.iter().for_each(|s| s.record_state(w));
            w.usize(*shaper_at);
            w.f32s(shaper_sums);
            w.u8(path_index(*shaper_path));
            for b in [shaped, fresh, rose] {
                w.bool(*b);
            }
            free_phase.iter().for_each(|&x| w.f64(x));
            w.f64(*tempo);
            w.f64(*bar_quarters);
            w.i64(bar_origin.0);
            w.f64(bar_origin.1);
            w.present(*anchor, |w, (sample, q)| {
                w.i64(sample);
                w.f64(q);
            });
            w.present(*quarters, |w, q| w.f64(q));
            w.i64(*host_pos);
            w.i64(*next_pos);
        });
    }

    /// Sets the state [`ModalVoiceInstrument::record_state`] wrote; `None` on any mismatch.
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        use transit_resonator::record::Record;
        let config = self.config();
        let ModalVoiceInstrument {
            p,
            part,
            kernel: _,
            up,
            latency: _,
            host_sr: _,
            ch: _,
            events,
            keys,
            pairing,
            sustain,
            retrigger,
            gate,
            shaper_gate,
            key,
            velocity,
            strength_sent,
            raw_sent,
            latch,
            release_velocity,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            model,
            level,
            shapers,
            matrix: _,
            settings: _,
            shaper_at,
            shaper_sums,
            shaper_path,
            shaped,
            fresh,
            rose,
            free_phase,
            tempo,
            bar_quarters,
            bar_origin,
            anchor,
            quarters,
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
            let Keys { order, len, held, pedalled } = keys;
            r.bytes(order)?;
            *len = r.usize().filter(|&l| l <= order.len())?;
            let mut bytes = [0u8; 128];
            r.bytes(&mut bytes)?;
            *held = bytes.map(|b| b != 0);
            r.bytes(&mut bytes)?;
            *pedalled = bytes.map(|b| b != 0);
            pairing.restore_state(r)?;
            for b in [sustain, retrigger, gate, shaper_gate] {
                *b = r.bool()?;
            }
            *key = r.u8()?;
            let mut v = [0.0f32; 3];
            r.f32s(&mut v)?;
            [*velocity, *strength_sent, *raw_sent] = v;
            *latch = r.bool()?;
            r.f32s(&mut v)?;
            [*release_velocity, *bend, *wheel] = v;
            *pressure = r.u8()?;
            r.bytes(poly)?;
            r.f32s(macros)?;
            *model = r.usize().filter(|&m| m < MODELS.len())?;
            level.restore(r)?;
            for s in shapers.iter_mut() {
                s.restore_state(r)?;
            }
            *shaper_at = r.usize()?;
            r.f32s(shaper_sums)?;
            *shaper_path = path_of(r.u8()?)?;
            for b in [shaped, fresh, rose] {
                *b = r.bool()?;
            }
            for x in free_phase.iter_mut() {
                *x = r.f64()?;
            }
            *tempo = r.f64()?;
            *bar_quarters = r.f64()?;
            *bar_origin = (r.i64()?, r.f64()?);
            *anchor = r.present(|r| Some((r.i64()?, r.f64()?)))?;
            *quarters = r.present(|r| r.f64())?;
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

fn path_index(p: Path) -> u8 {
    match p {
        Path::Dry => 0,
        Path::Shaped => 1,
        Path::In => 2,
        Path::Out => 3,
    }
}

fn path_of(i: u8) -> Option<Path> {
    Some(match i {
        0 => Path::Dry,
        1 => Path::Shaped,
        2 => Path::In,
        3 => Path::Out,
        _ => return None,
    })
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
    /// The `model` choice the part was last told (3: the FM pair).
    pub model: usize,
    /// Keys held down or kept by the pedal; the pedal is down; a key is kept by the pedal alone.
    pub keys: usize,
    pub sustain: bool,
    pub pedalled: bool,
    /// The part's gate as the last block played it; the shapers' envelopes are gated.
    pub gate: bool,
    pub shaper_gate: bool,
    /// A retrigger is pending (the next block drops the gate).
    pub retrigger: bool,
    /// The shapers' amp envelope level.
    pub amp: f32,
    /// Events queued for a later frame.
    pub pending: usize,
    /// The level smoother is still moving toward its target.
    pub ramping: bool,
    /// The transport anchor is set.
    pub anchored: bool,
    /// Nothing played since the last reset.
    pub fresh: bool,
    /// Converter input pushed beyond what has been pulled (part frames rendered ahead).
    pub ahead: i64,
    /// The frame of the shaper's 12-frame block the next part frame falls on.
    pub shaper_at: usize,
}

impl ModalVoiceInstrument {
    /// Poison witnesses only: overwrites the part's unobservable state (its scratch buffers, the
    /// values it writes before reading, its meters, and the controls and raw share this instrument
    /// writes before every part block) and the converter's scratch from a source seeded with
    /// `seed`; returns the values written.
    pub fn poison_unobservable(&mut self, seed: u32) -> u64 {
        let mut p = transit_resonator::record::Poison::new(seed);
        self.part.poison_unobservable(&mut p);
        // The instrument writes every control and the raw share before each part block.
        self.part.poison_owner_written(&mut p);
        p.count() + self.up.poison_unobservable(seed)
    }

    /// The witnesses' observations (see [`Observed`]).
    #[must_use]
    pub fn observe(&self) -> Observed {
        Observed {
            model: self.model,
            keys: self.keys.len,
            sustain: self.sustain,
            pedalled: self.keys.pedalled.iter().any(|p| *p),
            gate: self.gate,
            shaper_gate: self.shaper_gate,
            retrigger: self.retrigger,
            amp: self.shapers[0].amp.level,
            pending: self.events.len(),
            ramping: self.level.settling(),
            anchored: self.anchor.is_some(),
            fresh: self.fresh,
            ahead: self.up.ahead(),
            shaper_at: self.shaper_at,
        }
    }
}

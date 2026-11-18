//! Test seam (feature `testing`; ADR 0029, amendment for #742, sections 12 and 13): the Macro
//! Oscillator's complete state record and its restore.
//!
//! The inventory, by exhaustive destructuring (a new field does not compile until its treatment is
//! decided):
//! - **configuration** (the configuration child): the entry, the host rate `host_sr` (sanitized
//!   by `prepare`), the channel count `ch`; the maximum block is written as 0, since `prepare`
//!   ignores it and nothing depends on it; no loaded content (digest 0);
//! - **encoded**, in the state child: the parameter values; every slot (all [`MAX_VOICES`] of
//!   them, sounding or not: the port's voice with every engine of its bank, its generator,
//!   envelopes, trigger delay, post-processors and tuning, then the slot's MIDI state, shaper and
//!   seed, `Slot::record_state`); the voice count; the voice rate and the rate correction; the
//!   event queue (each event's offset and kind); the partial firmware block (`block`, `block_r`)
//!   and its position `pos`; the level smoother; the controllers (sustain, bend, wheel, channel
//!   pressure, poly pressure per key, the rack macros); the phrase counter; the note pairing; the
//!   running LFO phases; the tempo, bar length and bar origin; the transport anchor; the host
//!   positions;
//! - **derived**, re-derived by restore from restored state and configuration, never kept from the
//!   receiving instance: `matrix` is `Matrix::new` over the rows the parameter values set, and
//!   `shaper` is `settings()` over the parameter values, the bar length, the bar origin and the
//!   host rate. Both are recomputed whenever one of their inputs changes (`update` after every
//!   parameter set, `prepare`, `set_transport` when the bar changes), so they always equal that
//!   function. Witness: every transplant case restores into a fresh instance whose parameters are
//!   the defaults, after histories that set matrix rows and shaper parameters (`MidRamp`,
//!   `ModeSwitch`, `Sustain`), so a derived value kept from the receiver would change the output.

use super::*;
use transit_state_record::{Config, Reader, Writer};

impl MacroOscillator {
    fn config(&self) -> Config<'static> {
        Config { entry: MACRO_OSC_INFO.id, sample_rate: f64::from(self.host_sr), max_block: 0, channels: self.ch as u64, content: 0 }
    }

    /// The complete record (see the module docs).
    pub(super) fn record_state(&self, w: &mut Writer<'_>) {
        let MacroOscillator {
            p,
            slots,
            voices,
            rate,
            // Configuration (the configuration child).
            host_sr: _,
            rate_offset,
            ch: _,
            events,
            block,
            pos,
            level,
            sustain,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            counter,
            pairing,
            block_r,
            // Derived (see the module docs).
            matrix: _,
            shaper: _,
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
            w.usize(slots.len());
            slots.iter().for_each(|s| s.record_state(w));
            w.usize(*voices);
            w.u8(rate_index(*rate));
            w.f32(*rate_offset);
            w.usize(events.len());
            for e in events {
                event_record(e, w);
            }
            w.f32s(block);
            w.usize(*pos);
            level.fingerprint(w);
            w.bool(*sustain);
            w.f32(*bend);
            w.f32(*wheel);
            w.u8(*pressure);
            w.bytes(poly);
            w.f32s(macros);
            w.u64(*counter);
            pairing.record_state(w);
            w.f32s(block_r);
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

    /// Sets the state [`MacroOscillator::record_state`] wrote; `None` on any mismatch (another
    /// configuration, another slot count, a short or long stream).
    pub(super) fn restore_state(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let config = self.config();
        let MacroOscillator {
            p,
            slots,
            voices,
            rate,
            host_sr: _,
            rate_offset,
            ch: _,
            events,
            block,
            pos,
            level,
            sustain,
            bend,
            wheel,
            pressure,
            poly,
            macros,
            counter,
            pairing,
            block_r,
            matrix: _,
            shaper: _,
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
            (r.usize()? == slots.len()).then_some(())?;
            slots.iter_mut().try_for_each(|s| s.restore_state(r))?;
            *voices = r.usize().filter(|v| (1..=MAX_VOICES).contains(v))?;
            *rate = rate_of(r.u8()?)?;
            *rate_offset = r.f32()?;
            let n = r.usize().filter(|&n| n <= events.capacity())?;
            events.clear();
            for _ in 0..n {
                events.push(event_restore(r)?);
            }
            r.f32s(block)?;
            *pos = r.usize().filter(|&p| p <= BLOCK)?;
            level.restore(r)?;
            *sustain = r.bool()?;
            *bend = r.f32()?;
            *wheel = r.f32()?;
            *pressure = r.u8()?;
            r.bytes(poly)?;
            r.f32s(macros)?;
            *counter = r.u64()?;
            pairing.restore_state(r)?;
            r.f32s(block_r)?;
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
        self.shaper = self.settings();
        Some(())
    }
}

fn rate_index(rate: SampleRate) -> u8 {
    match rate {
        SampleRate::Hz44100 => 0,
        SampleRate::Hz48000 => 1,
        SampleRate::Hz88200 => 2,
        SampleRate::Hz96000 => 3,
    }
}

fn rate_of(index: u8) -> Option<SampleRate> {
    Some(match index {
        0 => SampleRate::Hz44100,
        1 => SampleRate::Hz48000,
        2 => SampleRate::Hz88200,
        3 => SampleRate::Hz96000,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// Bit `i`: voice `i` is not idle.
    pub sounding: u32,
    /// Bit `i`: a key holds voice `i`.
    pub held: u32,
    /// Bit `i`: only the pedal keeps voice `i` (it sounds a key no longer held).
    pub pedalled: u32,
    /// Bit `i`: voice `i` is released, silent and counting down to idle (its quiet count is not 0).
    pub counting: u32,
    /// The sustain pedal is down.
    pub sustain: bool,
    /// The level smoother is still moving toward its target.
    pub ramping: bool,
    /// Frames of the current 12-frame block already written out (12: none pending).
    pub pos: usize,
    /// Events queued for a later frame.
    pub pending: usize,
    /// Each voice's engine that rendered last (`None` before its first render).
    pub engines: [Option<usize>; MAX_VOICES],
    /// The transport anchor is set (the synced LFOs count from it).
    pub anchored: bool,
}

impl MacroOscillator {
    /// The witnesses' observations (see [`Observed`]).
    #[must_use]
    pub fn observe(&self) -> Observed {
        let mut engines = [None; MAX_VOICES];
        for (e, s) in engines.iter_mut().zip(&self.slots) {
            *e = s.voice.active_engine();
        }
        let bits = |f: &dyn Fn(&Slot) -> bool| self.slots.iter().enumerate().fold(0u32, |m, (i, s)| m | u32::from(f(s)) << i);
        Observed {
            sounding: bits(&|s| !s.idle),
            held: bits(&|s| s.notes.any_held()),
            pedalled: bits(&|s| s.sounding() && !s.notes.any_held()),
            counting: bits(&Slot::counting),
            sustain: self.sustain,
            ramping: self.level.settling(),
            pos: self.pos,
            pending: self.events.len(),
            engines,
            anchored: self.anchor.is_some(),
        }
    }
}

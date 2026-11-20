//! Test seam (feature `testing`, issue #210; ADR 0029, amendment section 8 and the #742
//! amendment, sections 12 and 14): a deterministic digest of each strip's state at a block
//! boundary, so the scheduling-condition gate (`tests/sched_equivalence.rs`) compares the graph
//! render with a serial render that has the same muted blocks on state, not only on output.
//!
//! Every part is the [`PrintHasher`] digest of a state record's word stream
//! ([`transit_state_record::Writer`]: typed items, floats under their own tags), written while
//! it is hashed; nothing is stored. Per strip, four parts:
//!
//! - **plugins**: each insert's, the instrument's and each clip-effect module's
//!   [`Plugin::fingerprint`] (the built-in compressor's detector envelope, sidechain filters,
//!   lookahead lines and smoothers; a test plug-in's own state; partial records until an entry
//!   declares its record complete);
//! - **lines**: the delay lines a skip must neither write nor idle: each keyed insert's
//!   [`key::KeyIn`] line (its delay, its history, its write position, its read), the strip's delay
//!   compensation lines (post and pre fader) and its edges' reads, and the [`key::KeyBank`] entry
//!   lines keyed at the strip (with a key's pending early-key frames);
//! - **midi**: the instrument's event state in section 14's canonical pending form (its queue,
//!   then what the render workers' pool parked for it: the deferred ring, then the live events
//!   of an away strip, admitted into a copy of the ledger; delivered notes and pedals; see
//!   `events::seam`), then its MIDI effect host (stages with their effects' records and carries,
//!   the notes given and started, retries, the epoch, the note view);
//! - **strip**: the strip's and its slots' own fields (section 14's inventory): a configuration
//!   child (the identity of the adopted plan: generations, track, layout, routes, mode bypass,
//!   each slot's layout, channels, suspendability and id, the hardware inserts' and the External
//!   Instrument's specs), then a state child (latency and delay compensation position, chase and
//!   edit flags, mute, touch ramps, clip effects, per slot the parameter values last pushed, the
//!   reported latency, failure, suspension and parameter pass flags and a reading insert's note
//!   delay, and each hardware insert's `live` flag with its send while live).
//!
//! [`strip_print`] destructures every inventoried type exhaustively, so a new field does not
//! compile until its treatment is decided; each field left out says why (configuration of an
//! identical plan, output only, telemetry, a protocol stamp, a scope assertion, or unobservable
//! by an overwrite-before-read argument that the mix-state gate's poison controls witness).
//!
//! A strip a render worker holds has no print (its state is in the worker's cell). Nothing here
//! runs on the audio path; it is compiled only with the feature.

use super::*;
use std::hash::Hasher;
use transit_state_record::Writer;

mod facts;
mod nudge;
mod poison;
pub use facts::{AudioFact, MidiFact, StripFacts};
pub use nudge::Nudge;
pub use poison::Poison;

/// The strip print's hash, 64 bits, one step per 64-bit word (#742; it was byte-wise FNV-1a,
/// which made hashing a third of the scheduling gate's time): the record codec's
/// [`transit_state_record::Digest`] (its docs say why two streams that differ in one word always
/// differ, and what the collision estimate for several rests on), which the print's writer feeds
/// directly ([`Writer::digest`]: no dynamic call per word). Deterministic across runs, platforms
/// and Rust versions (the standard library's hasher is not). Prints are compared only within one
/// process run; no value is pinned.
pub type PrintHasher = transit_state_record::Digest;

/// One strip's state at a block boundary (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StripPrint {
    /// The strip's track.
    pub track: TrackId,
    /// Its plug-ins' state.
    pub plugins: u64,
    /// Its delay lines and those of the keys keyed at it.
    pub lines: u64,
    /// Its instrument's event state, pending events and MIDI effect host.
    pub midi: u64,
    /// The strip's and its slots' own fields (configuration, then state).
    pub strip: u64,
}

impl StripPrint {
    /// The names of the parts that differ from `other`'s, for a divergence report.
    #[must_use]
    pub fn differing(&self, other: &StripPrint) -> Vec<&'static str> {
        let parts = [
            ("plugins", self.plugins != other.plugins),
            ("lines", self.lines != other.lines),
            ("midi", self.midi != other.midi),
            ("strip", self.strip != other.strip),
        ];
        parts.iter().filter(|p| p.1).map(|p| p.0).collect()
    }
}

/// A mix delay line: its flags, positions and, while it holds history, its contents. An idle
/// line's contents are unobservable: `DelayLine::write` clears the ring before writing an idle
/// line, `read_x` reads nothing from one, and `take_history` hands a line that was not live on
/// as not live (the gate's poison control of an idle line witnesses it).
pub(crate) fn line_record(l: &DelayLine, w: &mut Writer<'_>) {
    let DelayLine { bufs, idx, len, placeholder, live, written, fresh } = l;
    w.bool(*live);
    w.bool(*fresh);
    w.bool(*placeholder);
    w.usize(*idx);
    w.usize(*len);
    w.usize(*written);
    w.usize(bufs.len());
    if *live {
        for b in bufs {
            w.f32s(b);
        }
    }
}

/// A mix delay line's flags and positions, readable.
pub(crate) fn line_summary(l: &DelayLine) -> String {
    format!("line (live {}, fresh {}, write position {}, history {} of {})", l.live, l.fresh, l.idx, l.written, l.len)
}

/// An edge's read of a delay line (the delay it plays, its crossfade).
pub(crate) fn edge_record(e: &EdgeTap, w: &mut Writer<'_>) {
    let EdgeTap { eff, from, left, pre, init } = e;
    w.usize(*eff);
    w.usize(*from);
    w.usize(*left);
    w.bool(*pre);
    w.bool(*init);
}

/// An event source (its port, channel and generation).
pub(crate) fn source_record(w: &mut Writer<'_>, s: &events::Source) {
    let events::Source { port, channel, generation } = s;
    w.u16(*port);
    w.u8(*channel);
    w.u64(*generation);
}

/// A MIDI message: its discriminant, then that variant's fields.
pub(crate) fn message_record(w: &mut Writer<'_>, m: &Message) {
    match *m {
        Message::NoteOn { key, velocity, id } => {
            w.u8(0);
            w.u8(key);
            w.u8(velocity);
            w.u32(id.0);
        }
        Message::NoteOff { key, velocity, id } => {
            w.u8(1);
            w.u8(key);
            w.u8(velocity);
            w.u32(id.0);
        }
        Message::PolyPressure { key, value, id } => {
            w.u8(2);
            w.u8(key);
            w.u8(value);
            w.u32(id.0);
        }
        Message::ChannelPressure { value } => {
            w.u8(3);
            w.u8(value);
        }
        Message::Control { cc, value } => {
            w.u8(4);
            w.u8(cc);
            w.u8(value);
        }
        Message::Control14 { msb, value } => {
            w.u8(5);
            w.u8(msb);
            w.u16(value);
        }
        Message::PitchBend { value } => {
            w.u8(6);
            w.u16(value);
        }
        Message::Program { program } => {
            w.u8(7);
            w.u8(program);
        }
    }
}

/// A plug-in event (its channel, the host's voice id and its message).
pub(crate) fn event_record(w: &mut Writer<'_>, e: &PluginEvent) {
    let PluginEvent { channel, note_id, message } = e;
    w.u8(*channel);
    w.i32(*note_id);
    message_record(w, message);
}

/// A plug-in event at an offset in its block.
pub(crate) fn timed_record(w: &mut Writer<'_>, e: &transit_dsp::midi_fx::TimedEvent) {
    let transit_dsp::midi_fx::TimedEvent { offset, event } = e;
    w.u32(*offset);
    event_record(w, event);
}

/// The parameter values last pushed: the logical values (the inline prefix, or the prepared
/// storage of a long map), each a presence bool for the `NaN` sentinel ("never pushed", section
/// 12) and the value under the f32 tag; the prepared storage's length (it decides the long-map
/// view). Inline values past `len` and the heap of a short map are refilled with the sentinel
/// whenever the length changes (`Applied::values`), so they are unobservable. `mismatch` is
/// telemetry.
fn applied_record(a: &Applied, w: &mut Writer<'_>) {
    let Applied { len, inline, heap, mismatch: _ } = a;
    w.usize(*len);
    w.usize(heap.len());
    let values = if *len <= APPLIED_INLINE { inline.get(..*len).unwrap_or_default() } else { heap.as_slice() };
    w.usize(values.len());
    for &v in values {
        w.present((!v.is_nan()).then_some(v), |w, v| w.f32(v));
    }
}

/// A slot's configuration (section 14: of an identical adopted plan).
fn slot_config(s: &PluginSlot, w: &mut Writer<'_>) {
    w.u64(s.generation.get());
    w.u16(s.layout);
    w.usize(s.ch);
    w.bool(s.suspendable);
    w.bytes(s.id.as_bytes());
}

/// A slot's own state (its plug-in's record is in the plugins part, its instrument events in
/// the midi part).
fn slot_state(s: &PluginSlot, w: &mut Writer<'_>) {
    let PluginSlot {
        raw_latency,
        // Configuration: `slot_config`.
        id: _,
        ch: _,
        suspendable: _,
        generation: _,
        layout: _,
        // The plugins part.
        plugin: _,
        applied,
        failed,
        errors_seen,
        fresh,
        quiet,
        output_silent,
        woken,
        parameter_pass,
        // The midi part (canonical pending form).
        events: _,
        note_delay,
        // Output only: the panels' scope and analysis rings.
        tap: _,
        sample_tap: _,
    } = s;
    applied_record(applied, w);
    w.usize(raw_latency.load(std::sync::atomic::Ordering::Relaxed));
    w.bool(*failed);
    w.u64(*errors_seen);
    w.bool(*fresh);
    w.usize(*quiet);
    w.bool(*output_silent);
    w.bool(*woken);
    w.bool(*parameter_pass);
    w.present(note_delay.as_ref(), |w, d| d.record_state(w));
}

/// Every slot of a strip, in order: the inserts, the instrument, then each clip's modules.
fn slots(st: &Strip) -> impl Iterator<Item = Option<&PluginSlot>> {
    let ClipFxTable(chains) = &st.clip_fx;
    let clips = chains.iter().flat_map(|(_, fx)| [fx.eq.as_ref(), fx.comp.as_ref()]);
    st.plugins.iter().map(Option::as_ref).chain(std::iter::once(st.instrument.as_ref())).chain(clips)
}

/// The digest of the record `f` writes.
fn digest(f: impl FnOnce(&mut Writer<'_>)) -> u64 {
    let mut h = PrintHasher::default();
    f(&mut Writer::digest(&mut h));
    h.finish()
}

/// What the midi part needs besides the strip: the next block's start, what the pool parked for
/// the strip, and the engine's admission state.
struct MidiContext<'a> {
    start: u64,
    parked: strip_pool::Parked,
    engine: &'a events::EngineEvents,
}

/// One home strip's print (see the module docs): every field of [`Strip`] is handed to its part
/// or set aside with its reason.
#[allow(clippy::panic)]
fn strip_print(st: &Strip, i: usize, keys: &key::KeyBank, cx: &MidiContext<'_>) -> StripPrint {
    let Strip {
        mode_bypass,
        // Unobservable, each overwritten before it is read (the gate's poison controls): `buf` by
        // the clip render into it at the start of each block (`render_strip`), `pre` by the
        // pre-fader copy before any pre-fader send or key reads it, `scratch` by the instrument's
        // second output channel (its contract: an instrument writes every output channel; a mono
        // strip lends `scratch` as that channel), `clipbuf` by each clip's render before its
        // effects and the mix read it, `outbuf` by `DelayLine::read_x`, which clears and refills it.
        buf: _,
        pre: _,
        key_pre,
        scratch: _,
        plugins,
        instrument,
        chase,
        edited,
        muted,
        // Output only: the gain-reduction reading, the meter and the failure report counters.
        gr: _,
        meter: _,
        failures: _,
        clip_fx,
        clipbuf: _,
        latency,
        key_in,
        tap,
        tap_pre,
        outbuf: _,
        pdc_out,
        pdc_pos,
        edges,
        sink_routes,
        input_route,
        generation,
        track,
        id,
        layout,
        // The hardware inserts' and the External Instrument's slots: each slot's spec in the
        // configuration child, a hardware slot's `live` and its live send in the state child
        // (`HwSlot::record_state` says why an idle send is unobservable).
        hw,
        ext,
        // Scope assertion: a retained panic payload fails before any comparison (below).
        panic,
        // The pool's protocol stamp: legitimately different between the graph and the serial
        // render (property 1 and the conflict count cover it).
        rendered: _,
        touch,
    } = st;
    // The clip-effect chains, sorted by clip id: the table holds nothing else.
    let ClipFxTable(chains) = clip_fx;
    if panic.is_some() {
        panic!("strip {i} holds a retained panic payload: a case with one fails before any comparison (#742 section 14)");
    }
    let plugins_part = digest(|w| {
        for s in slots(st) {
            w.present(s, |w, s| s.plugin.fingerprint(w));
        }
    });
    let lines = digest(|w| {
        w.usize(key_in.len());
        key_in.iter().for_each(|k| k.record(w));
        line_record(tap, w);
        line_record(tap_pre, w);
        edges.iter().for_each(|e| edge_record(e, w));
        keys.record_of(i, w);
    });
    let midi = digest(|w| {
        let inst = instrument.as_ref().and_then(|s| s.events.as_deref().map(|e| (s.generation, e)));
        w.present(inst, |w, (binding, ev)| ev.record_canonical(w, cx.start, &cx.parked, cx.engine, binding));
    });
    let strip = digest(|w| {
        w.child(|w| {
            w.u64(generation.get());
            w.u16(*track);
            w.u64(id.0);
            w.u16(*layout);
            w.usize(sink_routes.len());
            for r in sink_routes {
                w.present(*r, |w, r| w.usize(r));
            }
            w.present(input_route.as_ref(), |w, r| {
                w.usize(r.len());
                r.iter().for_each(|c| w.present(*c, |w, c| w.u16(c)));
            });
            w.usize(mode_bypass.len());
            mode_bypass.iter().for_each(|&b| w.bool(b));
            w.bool(*key_pre);
            w.usize(plugins.len());
            w.usize(chains.len());
            for (clip, fx) in chains {
                w.u64(*clip);
                w.f32(fx.sr);
                w.usize(fx.ch);
            }
            for s in slots(st) {
                w.present(s, |w, s| slot_config(s, w));
            }
            w.usize(hw.len());
            hw.iter().for_each(|h| h.record_config(w));
            w.present(ext.as_ref(), |w, x| x.record_config(w));
        });
        w.child(|w| {
            w.usize(*latency);
            w.usize(*pdc_out);
            w.usize(*pdc_pos);
            w.bool(*chase);
            w.bool(*edited);
            w.bool(*muted);
            touch.record(w);
            for (_, fx) in chains {
                let ClipFx { eq: _, comp: _, eq_on, comp_on, gain_db, lazy, sr: _, ch: _ } = fx;
                w.bool(*eq_on);
                w.bool(*comp_on);
                w.f32(*gain_db);
                w.bool(*lazy);
            }
            for s in slots(st) {
                w.present(s, |w, s| slot_state(s, w));
            }
            hw.iter().for_each(|h| h.record_state(w));
        });
    });
    StripPrint { track: *id, plugins: plugins_part, lines, midi, strip }
}

impl MixEngine {
    /// Every strip of the active plan in table order (phase 1, then the auxes, then the rest),
    /// with its print, or `None` while a render worker holds it. Off the audio path. Panics on a
    /// strip holding a retained panic payload, or a MIDI effect host caught mid-capture (section
    /// 14's scope assertions).
    #[must_use]
    pub fn strip_prints(&self) -> Vec<Option<StripPrint>> {
        let Some(p) = self.plan.as_deref() else { return Vec::new() };
        let tables = &p.tables;
        tables
            .strips
            .iter()
            .enumerate()
            .map(|(i, st)| {
                if self.pool.as_ref().is_some_and(|pool| pool.is_away(i)) {
                    return None;
                }
                let parked = self.pool.as_ref().map(|pool| pool.parked(i)).unwrap_or_default();
                let cx = MidiContext { start: self.events.next_frame, parked, engine: &self.events };
                Some(strip_print(st, i, &tables.keys, &cx))
            })
            .collect()
    }

    /// Every strip's state in readable form (the delay lines' flags and positions, each
    /// plug-in's record digest), as [`MixEngine::strip_prints`] orders them: for a failure report.
    #[must_use]
    pub fn strip_states(&self) -> Vec<Option<String>> {
        let Some(p) = self.plan.as_deref() else { return Vec::new() };
        let tables = &p.tables;
        tables
            .strips
            .iter()
            .enumerate()
            .map(|(i, st)| {
                if self.pool.as_ref().is_some_and(|pool| pool.is_away(i)) {
                    return None;
                }
                let mut parts = Vec::new();
                for (k, slot) in st.plugins.iter().enumerate() {
                    if let Some(slot) = slot {
                        let mut h = PrintHasher::default();
                        slot.plugin.fingerprint(&mut Writer::digest(&mut h));
                        parts.push(format!("insert {k} ({}): {:016x}", slot.id, h.finish()));
                    }
                }
                for k in &st.key_in {
                    parts.push(format!("key in (slot {}): want {}, ran {}, {}", k.slot, k.want, k.ran, k.line.summary()));
                }
                parts.extend(tables.keys.summary_of(i));
                parts.push(format!("output {}, pre-fader {}", line_summary(&st.tap), line_summary(&st.tap_pre)));
                parts.push(format!("latency {}, delay compensation out {} at {}", st.latency, st.pdc_out, st.pdc_pos));
                Some(parts.join("; "))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;

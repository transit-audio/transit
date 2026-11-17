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
//!   each slot's layout, channels, suspendability and id), then a state child (latency and delay
//!   compensation position, chase and edit flags, mute, touch ramps, clip effects, and per slot
//!   the parameter values last pushed, the reported latency, failure, suspension and parameter
//!   pass flags and a reading insert's note delay).
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
/// which made hashing a third of the scheduling gate's time). Deterministic across runs,
/// platforms and Rust versions (the standard library's hasher is not): the record writer gives
/// it every value as a word through `write_u64` (floats by their bits under their own tags, see
/// `transit_state_record`). Prints are compared only within one process run; no value is pinned.
///
/// Each step is `state = mix(state ^ word)`, where `mix` is the SplitMix64 finalizer (xor-shift,
/// multiply, xor-shift, multiply, xor-shift; both multipliers odd). Every operation of `mix` is
/// invertible mod 2^64, so `mix` is a bijection. For a fixed state a step is therefore injective
/// in the word, and for a fixed word it is a bijection of the state. So two streams that differ
/// in exactly one word (any single bit of any recorded value) always end in different prints:
/// the states differ right after that word, and every later step keeps them apart. Two streams
/// that differ in several words collide only if `mix` maps the differences onto each other,
/// which a full-avalanche permutation does with odds near 2^-64 under the assumption that the
/// digests of different streams behave as independent uniform values (an estimate, not a bound).
#[derive(Clone, Copy, Debug)]
pub struct PrintHasher(u64);

impl Default for PrintHasher {
    fn default() -> Self {
        PrintHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl PrintHasher {
    /// One step over a word (see the type docs).
    #[inline]
    fn word(&mut self, w: u64) {
        self.0 = mix(self.0 ^ w);
    }
}

/// The SplitMix64 finalizer: a bijection of `u64` with full avalanche (see [`PrintHasher`]).
#[inline]
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

impl Hasher for PrintHasher {
    /// Eight bytes to a word, little-endian; a short tail is zero-padded, then the length is
    /// hashed, so a tail of zero bytes still counts. (The record writer never calls it.)
    fn write(&mut self, bytes: &[u8]) {
        let (words, tail) = bytes.as_chunks::<8>();
        for &w in words {
            self.word(u64::from_le_bytes(w));
        }
        if !tail.is_empty() {
            let mut last = [0u8; 8];
            last[..tail.len()].copy_from_slice(tail);
            self.word(u64::from_le_bytes(last));
        }
        self.word(bytes.len() as u64);
    }

    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

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
    f(&mut Writer::new(&mut h));
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
        generation,
        track,
        id,
        layout,
        // Scope assertions: the gate fails a case that holds or skips a strip with a hardware
        // insert or an External Instrument (`MixEngine::strip_external`); a retained panic payload
        // fails before any comparison (below).
        hw: _,
        ext: _,
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
                        slot.plugin.fingerprint(&mut Writer::new(&mut h));
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

    /// Strip `i` carries a hardware insert or an External Instrument (section 14's scope: such a
    /// strip may not be held or skipped by a case; its state is not inventoried yet).
    #[must_use]
    pub fn strip_external(&self, i: usize) -> bool {
        self.plan.as_deref().and_then(|p| p.tables.strips.get(i)).is_some_and(|s| !s.hw.is_empty() || s.ext.is_some())
    }
}

#[cfg(test)]
mod tests;

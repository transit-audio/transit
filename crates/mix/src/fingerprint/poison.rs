//! Test seam (feature `testing`; ADR 0029, amendment for #742, section 14): the poison controls
//! that witness the strip inventory's overwrite-before-read arguments. One of two identical
//! renders fills one buffer of one strip with a fixed pattern between blocks; the outputs and the
//! prints must stay identical, and the buffer's write site must have reached it (the pattern is
//! gone from it after the next block, or the MIDI host's storage carried events again).

use super::super::*;

/// The storage a poison control fills (the treatment column of section 14's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poison {
    /// `Strip::buf`, every channel.
    Buf,
    /// `Strip::pre`.
    Pre,
    /// `Strip::scratch` (a mono strip's lent second channel for its instrument).
    Scratch,
    /// `Strip::clipbuf`.
    Clipbuf,
    /// `Strip::outbuf`.
    Outbuf,
    /// Each keyed insert's `KeyIn::scratch`.
    KeyInScratch,
    /// The key entries keyed at the strip: `buf` past the pending early-key frames.
    KeyBuf,
    /// The key entries keyed at the strip: `scratch`.
    KeyScratch,
    /// The instrument's MIDI effect host: the collection buffer `a`.
    MidiA,
    /// The host's effect output `b`.
    MidiB,
    /// The host's merge `merged`.
    MidiMerged,
    /// The strip's delay compensation lines (`tap`, `tap_pre`) while idle: their contents.
    IdleTap,
    /// Each hardware insert's send while the slot is not live.
    HwSend,
    /// Each keyed insert's `KeyIn` line while idle: its contents.
    IdleKeyIn,
    /// The lines of the key entries keyed at the strip while idle: their contents.
    IdleKeyEntry,
}

/// The pattern's value at position `k`: finite, nonzero, and changed from `prior` (its negation
/// when it equals it).
fn value(k: usize, prior: f32) -> f32 {
    let v = 0.375 + (k % 13) as f32 * 0.0625;
    if v.to_bits() == prior.to_bits() { -v } else { v }
}

/// Whether `x` holds the pattern's value at `k` (either sign).
fn holds(k: usize, x: f32) -> bool {
    let v = 0.375 + (k % 13) as f32 * 0.0625;
    x.to_bits() == v.to_bits() || x.to_bits() == (-v).to_bits()
}

fn fill(bufs: &mut [Vec<f32>], from: usize) -> usize {
    let mut n = 0;
    for b in bufs {
        for (k, x) in b.iter_mut().enumerate().skip(from) {
            let v = value(k, *x);
            n += usize::from(v.to_bits() != x.to_bits());
            *x = v;
        }
    }
    n
}

fn left(bufs: &[Vec<f32>], from: usize) -> usize {
    bufs.iter().map(|b| b.iter().enumerate().skip(from).filter(|(k, x)| holds(*k, **x)).count()).sum()
}

impl MixEngine {
    /// Fills `target` of `track`'s strip with the pattern (lengths and capacities kept). Returns
    /// the values (or MIDI entries) it changed: zero means the control proves nothing. Between
    /// blocks, with the strip home.
    pub fn poison(&mut self, track: TrackId, target: Poison) -> usize {
        let Some(p) = self.plan.as_deref_mut() else { return 0 };
        let Some(i) = p.tables.strip_of(track) else { return 0 };
        let tables = &mut p.tables;
        let Some(st) = tables.strips.get_mut(i) else { return 0 };
        match target {
            Poison::Buf => fill(&mut st.buf, 0),
            Poison::Pre => fill(&mut st.pre, 0),
            Poison::Scratch => fill(std::slice::from_mut(&mut st.scratch), 0),
            Poison::Clipbuf => fill(&mut st.clipbuf, 0),
            Poison::Outbuf => fill(&mut st.outbuf, 0),
            Poison::KeyInScratch => st.key_in.iter_mut().map(|k| fill(&mut k.scratch, 0)).sum(),
            Poison::IdleTap => [&mut st.tap, &mut st.tap_pre].into_iter().filter(|l| !l.live).map(|l| fill(&mut l.bufs, 0)).sum(),
            Poison::HwSend => st.hw.iter_mut().map(|h| h.poison_idle_send(fill)).sum(),
            Poison::IdleKeyIn => st.key_in.iter_mut().map(|k| k.line.poison_idle(fill)).sum(),
            Poison::IdleKeyEntry => tables.keys.poison_idle_lines_of(i, fill),
            Poison::KeyBuf | Poison::KeyScratch => tables.keys.poison_of(i, target == Poison::KeyBuf, fill),
            Poison::MidiA | Poison::MidiB | Poison::MidiMerged => {
                let which = match target {
                    Poison::MidiA => 0,
                    Poison::MidiB => 1,
                    _ => 2,
                };
                st.instrument.as_mut().and_then(|s| s.events.as_deref_mut()).map_or(0, |e| e.fx.poison(which))
            }
        }
    }

    /// After a block: the poisoned storage's write site ran over it (no pattern value is left in
    /// a float buffer; the MIDI host's storage carried events again; an idle delay line came
    /// alive and its first write cleared it).
    #[must_use]
    pub fn poison_reached(&self, track: TrackId, target: Poison) -> bool {
        let Some(p) = self.plan.as_deref() else { return false };
        let Some(st) = p.tables.strip_of(track).and_then(|i| p.tables.strips.get(i).map(|s| (i, s))) else { return false };
        let (i, st) = st;
        match target {
            Poison::Buf => left(&st.buf, 0) == 0,
            Poison::Pre => left(&st.pre, 0) == 0,
            Poison::Scratch => left(std::slice::from_ref(&st.scratch), 0) == 0,
            Poison::Clipbuf => left(&st.clipbuf, 0) == 0,
            Poison::Outbuf => left(&st.outbuf, 0) == 0,
            Poison::KeyInScratch => st.key_in.iter().all(|k| left(&k.scratch, 0) == 0),
            // An idle line came alive this block: its write cleared the ring first.
            Poison::IdleTap => st.tap.live && left(&st.tap.bufs, 0) == 0,
            // The slot ran (it is live again) and its write covered the send.
            Poison::HwSend => !st.hw.is_empty() && st.hw.iter().all(|h| h.poison_reached(left)),
            // An idle line came alive this block: its first write cleared the ring.
            Poison::IdleKeyIn => !st.key_in.is_empty() && st.key_in.iter().all(|k| k.line.poison_idle_reached(left)),
            Poison::IdleKeyEntry => p.tables.keys.poison_idle_lines_reached(i, left),
            Poison::KeyBuf | Poison::KeyScratch => p.tables.keys.poison_left_of(i, target == Poison::KeyBuf, left) == 0,
            Poison::MidiA | Poison::MidiB | Poison::MidiMerged => {
                let which = match target {
                    Poison::MidiA => 0,
                    Poison::MidiB => 1,
                    _ => 2,
                };
                st.instrument.as_ref().and_then(|s| s.events.as_deref()).is_some_and(|e| e.fx.poison_reached(which))
            }
        }
    }
}

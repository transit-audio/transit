//! Test seam (feature `testing`, issue #210, ADR 0029 amendment section 8): a deterministic
//! fingerprint of each strip's state at a block boundary, so the scheduling-condition gate
//! (`tests/sched_equivalence.rs`) compares the graph render with a serial render that has the
//! same muted blocks on state, not only on output.
//!
//! Per strip, three hashes ([`PrintHasher`], the same on every platform and run):
//!
//! - **plugins**: each insert's and the instrument's [`Plugin::fingerprint`] (the built-in
//!   compressor's detector envelope, sidechain filters, lookahead lines and smoothers; a test
//!   plug-in's own state);
//! - **lines**: the delay lines a skip must neither write nor idle: each keyed insert's
//!   [`key::KeyIn`] line (its delay, its history, its write position, its read), the strip's delay
//!   compensation lines (post and pre fader) and its edges' reads, and the [`key::KeyBank`] entry
//!   lines keyed at the strip;
//! - **midi**: the instrument's delivered event state (sounding notes, held pedals, the values
//!   sent) and the messages still pending for it, in order: its queue, then what the render
//!   workers' pool parked for it (the deferred ring, then the live events of an away strip).
//!   Pending events are compared by message, not by due frame: a parked event is readmitted with
//!   the frame it lands on, a queued one keeps its own (both land on the same sample).
//!
//! A strip a render worker holds has no fingerprint (its state is in the worker's cell).
//! Nothing here runs on the audio path; it is compiled only with the feature.

use super::*;
use std::hash::Hasher;

/// The strip print's hash, 64 bits, one step per 64-bit word (#742; it was byte-wise FNV-1a,
/// which made hashing a third of the scheduling gate's time). Deterministic across runs,
/// platforms and Rust versions (the standard library's hasher is not): every value enters as
/// a little-endian word (`to_le_bytes`, `to_bits`, `usize` widened to `u64`), never in native
/// byte order. Prints are compared only within one process run; no value is pinned.
///
/// Each step is `state = mix(state ^ word)`, where `mix` is the SplitMix64 finalizer (xor-shift,
/// multiply, xor-shift, multiply, xor-shift; both multipliers odd). Every operation of `mix` is
/// invertible mod 2^64, so `mix` is a bijection. For a fixed state a step is therefore injective
/// in the word, and for a fixed word it is a bijection of the state. So two inputs that differ
/// in exactly one word (any single bit of any hashed sample) always end in different prints:
/// the states differ right after that word, and every later step keeps them apart. Two inputs
/// that differ in several words collide only if `mix` maps the differences onto each other,
/// which a full-avalanche permutation does with odds near 2^-64 (a plain rotate-xor-multiply
/// step did not: two single-bit changes in neighboring samples collided in this module's tests).
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

    /// Every sample's bits, in order, two samples to a word (the first in the low half). The
    /// slice's length is not hashed here: the caller hashes the line's `len` (its capacity, not
    /// a buffer's length), as before. An odd tail is still unambiguous because every buffer's
    /// length is fixed by the structure (the capacity plus one block; the channel count is the number of
    /// buffers).
    ///
    /// Whole runs of eight samples go through four independent lanes (word `k` of each run into
    /// lane `k`), so the steps do not wait on each other; the lanes start from the state, each
    /// with its own constant, and are then folded into the state one after another with the
    /// ordinary step; the rest of the slice takes the ordinary step too. Each lane is the same
    /// construction as the state, and the fold is injective in each lane, so the type docs'
    /// argument holds: a slice that differs in one sample differs in one word of one lane (or
    /// of the rest), and the print differs.
    pub(crate) fn write_f32s(&mut self, v: &[f32]) {
        let pair = |a: f32, b: f32| u64::from(a.to_bits()) | (u64::from(b.to_bits()) << 32);
        let s = self.0;
        let (mut l0, mut l1, mut l2, mut l3) = (s ^ LANES[0], s ^ LANES[1], s ^ LANES[2], s ^ LANES[3]);
        let (runs, rest) = v.as_chunks::<8>();
        for &[a, b, c, d, e, f, g, h] in runs {
            l0 = mix(l0 ^ pair(a, b));
            l1 = mix(l1 ^ pair(c, d));
            l2 = mix(l2 ^ pair(e, f));
            l3 = mix(l3 ^ pair(g, h));
        }
        for l in [l0, l1, l2, l3] {
            self.word(l);
        }
        let (pairs, last) = rest.as_chunks::<2>();
        for &[a, b] in pairs {
            self.word(pair(a, b));
        }
        if let [last] = last {
            self.word(u64::from(last.to_bits()));
        }
    }
}

/// The lanes' starting constants (distinct, so the lanes are not interchangeable).
const LANES: [u64; 4] = [0x243f_6a88_85a3_08d3, 0x1319_8a2e_0370_7344, 0xa409_3822_299f_31d0, 0x082e_fa98_ec4e_6c89];

/// The SplitMix64 finalizer: a bijection of `u64` with full avalanche (see [`PrintHasher`]).
#[inline]
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

impl Hasher for PrintHasher {
    /// Eight bytes to a word, little-endian; a short tail is zero-padded, then the length is
    /// hashed, so a tail of zero bytes still counts.
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

    fn write_u8(&mut self, i: u8) {
        self.word(u64::from(i));
    }

    fn write_u16(&mut self, i: u16) {
        self.word(u64::from(i));
    }

    fn write_u32(&mut self, i: u32) {
        self.word(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.word(i);
    }

    fn write_u128(&mut self, i: u128) {
        self.word(i as u64);
        self.word((i >> 64) as u64);
    }

    fn write_usize(&mut self, i: usize) {
        self.word(i as u64);
    }

    fn write_i8(&mut self, i: i8) {
        self.write_u8(i as u8);
    }

    fn write_i16(&mut self, i: i16) {
        self.write_u16(i as u16);
    }

    fn write_i32(&mut self, i: i32) {
        self.write_u32(i as u32);
    }

    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }

    fn write_i128(&mut self, i: i128) {
        self.write_u128(i as u128);
    }

    fn write_isize(&mut self, i: isize) {
        self.write_usize(i as usize);
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
    /// Its instrument's event state and pending events.
    pub midi: u64,
}

/// A mix delay line: its flags, positions and, while it holds history, its contents.
pub(crate) fn line_print(l: &DelayLine, h: &mut PrintHasher) {
    h.write_u8(u8::from(l.live) | (u8::from(l.fresh) << 1));
    h.write_usize(l.idx);
    h.write_usize(l.len);
    h.write_usize(l.written);
    if l.live {
        for b in &l.bufs {
            h.write_f32s(b);
        }
    }
}

/// A mix delay line's flags and positions, readable.
pub(crate) fn line_summary(l: &DelayLine) -> String {
    format!("line (live {}, fresh {}, write position {}, history {} of {})", l.live, l.fresh, l.idx, l.written, l.len)
}

/// An edge's read of a delay line (the delay it plays, its crossfade).
pub(crate) fn edge_print(e: &EdgeTap, h: &mut PrintHasher) {
    h.write_usize(e.eff);
    h.write_usize(e.from);
    h.write_usize(e.left);
    h.write_u8(u8::from(e.pre) | (u8::from(e.init) << 1));
}

impl MixEngine {
    /// Every strip of the active plan in table order (phase 1, then the auxes, then the rest),
    /// with its fingerprint, or `None` while a render worker holds it. Off the audio path.
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
                let mut plugins = PrintHasher::default();
                for (k, slot) in st.plugins.iter().enumerate() {
                    if let Some(slot) = slot {
                        plugins.write_usize(k);
                        slot.plugin.fingerprint(&mut plugins);
                    }
                }
                if let Some(inst) = &st.instrument {
                    plugins.write_u8(0xFF);
                    inst.plugin.fingerprint(&mut plugins);
                }
                let mut lines = PrintHasher::default();
                for k in &st.key_in {
                    k.fingerprint(&mut lines);
                }
                line_print(&st.tap, &mut lines);
                line_print(&st.tap_pre, &mut lines);
                for e in &st.edges {
                    edge_print(e, &mut lines);
                }
                tables.keys.fingerprint_of(i, &mut lines);
                let mut midi = PrintHasher::default();
                if let Some(ev) = st.instrument.as_ref().and_then(|s| s.events.as_deref()) {
                    ev.fingerprint(&mut midi);
                }
                if let Some(pool) = &self.pool {
                    pool.pending_midi(i, &mut midi);
                }
                Some(StripPrint { track: st.id, plugins: plugins.finish(), lines: lines.finish(), midi: midi.finish() })
            })
            .collect()
    }

    /// Every strip's state in readable form (the delay lines' flags and positions, each
    /// plug-in's fingerprint), as [`MixEngine::strip_prints`] orders them: for a failure report.
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
                        slot.plugin.fingerprint(&mut h);
                        parts.push(format!("insert {k} ({}): {:016x}", slot.id, h.finish()));
                    }
                }
                for k in &st.key_in {
                    parts.push(format!("key in (slot {}): want {}, ran {}, {}", k.slot, k.want, k.ran, k.line.summary()));
                }
                parts.extend(tables.keys.summary_of(i));
                parts.push(format!("output {}, pre-fader {}", line_summary(&st.tap), line_summary(&st.tap_pre)));
                Some(parts.join("; "))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples with mixed bit patterns: signs, zeros, a subnormal, a NaN payload, infinities.
    /// Nineteen of them: two runs of eight through the lanes, then a pair and a single sample
    /// through the ordinary step, so every path of `write_f32s` is covered.
    const SAMPLES: [u32; 19] = [
        0x3f80_0000,
        0x8000_0000,
        0x0000_0001,
        0x7fc0_0001,
        0xbe99_999a,
        0x0000_0000,
        0x4049_0fdb,
        0x7f80_0000,
        0xff80_0000,
        0x3e80_0000,
        0x0000_0000,
        0x0000_0000,
        0x3f00_0000,
        0xbf00_0000,
        0x0080_0000,
        0x3c23_d70a,
        0x0000_0000,
        0xc2c8_0000,
        0x3f80_0000,
    ];

    fn samples() -> Vec<f32> {
        SAMPLES.iter().map(|&b| f32::from_bits(b)).collect()
    }

    fn print_of(v: &[f32]) -> u64 {
        let mut h = PrintHasher::default();
        h.write_usize(v.len());
        h.write_f32s(v);
        h.finish()
    }

    /// Every single-bit change of every sample changes the print (the type docs' argument,
    /// exhaustively over this slice), and no two of the 608 changed slices share a print.
    #[test]
    fn every_single_bit_of_every_sample_changes_the_print() {
        let base = samples();
        let mut seen = std::collections::HashSet::from([print_of(&base)]);
        for i in 0..base.len() {
            for bit in 0..32 {
                let mut v = base.clone();
                v[i] = f32::from_bits(v[i].to_bits() ^ (1 << bit));
                assert!(seen.insert(print_of(&v)), "sample {i}, bit {bit}: the print did not change, or collided");
            }
        }
    }

    /// The same for the byte path (a plug-in's `write`), with a zero-padded tail.
    #[test]
    fn every_single_bit_of_a_byte_write_changes_the_print() {
        let bytes: Vec<u8> = (0u8..13).map(|b| b.wrapping_mul(37)).collect();
        let of = |b: &[u8]| {
            let mut h = PrintHasher::default();
            h.write(b);
            h.finish()
        };
        let mut seen = std::collections::HashSet::from([of(&bytes)]);
        for i in 0..bytes.len() {
            for bit in 0..8 {
                let mut b = bytes.clone();
                b[i] ^= 1 << bit;
                assert!(seen.insert(of(&b)), "byte {i}, bit {bit}: the print did not change, or collided");
            }
        }
        // A trailing zero byte is not the same input.
        let mut longer = bytes.clone();
        longer.push(0);
        assert_ne!(of(&bytes), of(&longer));
    }

    /// Negative control at the line level: a delay line that differs in one sample's lowest bit
    /// has a different print; an idle line's contents are not part of its print (as before).
    #[test]
    fn a_delay_line_differing_in_one_sample_has_a_different_print() {
        let line = |live: bool, nudge: Option<(usize, usize)>| {
            let mut l = DelayLine::with_capacity(2, 61, 8);
            for (c, b) in l.bufs.iter_mut().enumerate() {
                for (k, s) in b.iter_mut().enumerate() {
                    *s = (c * 100 + k) as f32 * 0.001;
                }
            }
            if let Some((c, k)) = nudge {
                l.bufs[c][k] = f32::from_bits(l.bufs[c][k].to_bits() ^ 1);
            }
            (l.live, l.idx, l.written) = (live, 5, 61);
            let mut h = PrintHasher::default();
            line_print(&l, &mut h);
            h.finish()
        };
        let base = line(true, None);
        for at in [(0, 0), (0, 7), (0, 68), (1, 0), (1, 30), (1, 34), (1, 68)] {
            assert_ne!(line(true, Some(at)), base, "sample {at:?}");
        }
        assert_eq!(line(false, Some((1, 34))), line(false, None), "an idle line hashes no contents");
    }

    /// The hash is a fixed function of the words, identical on every platform (little-endian
    /// words, `usize` widened): pinned over exact bit patterns only, no float arithmetic. This
    /// pins the hasher, not any strip's print (prints are compared only within one run). The
    /// value was computed by a separate model of the steps in arbitrary-precision integers,
    /// not captured from this code.
    #[test]
    fn the_hash_of_fixed_words_is_pinned() {
        let mut h = PrintHasher::default();
        h.write_u8(0xa5);
        h.write_u16(0x1234);
        h.write_u32(0xdead_beef);
        h.write_u64(0x0123_4567_89ab_cdef);
        h.write_usize(48_000);
        h.write_i32(-2);
        h.write_f32s(&samples());
        h.write(b"transit");
        assert_eq!(h.finish(), 0x4aa0_df7a_291d_c918, "the pinned hash of fixed words");
    }
}

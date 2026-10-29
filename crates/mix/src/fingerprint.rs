//! Test seam (feature `testing`, issue #210, ADR 0029 amendment section 8): a deterministic
//! fingerprint of each strip's state at a block boundary, so the scheduling-condition gate
//! (`tests/sched_equivalence.rs`) compares the graph render with a serial render that has the
//! same muted blocks on state, not only on output.
//!
//! Per strip, three hashes (FNV-1a 64, the same on every platform and run):
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

/// FNV-1a, 64 bits: deterministic across runs, platforms and Rust versions (the standard
/// library's hasher is not).
#[derive(Clone, Copy, Debug)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
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
pub(crate) fn line_print(l: &DelayLine, h: &mut dyn Hasher) {
    h.write_u8(u8::from(l.live) | (u8::from(l.fresh) << 1));
    h.write_usize(l.idx);
    h.write_usize(l.len);
    h.write_usize(l.written);
    if l.live {
        for b in &l.bufs {
            b.iter().for_each(|v| h.write_u32(v.to_bits()));
        }
    }
}

/// A mix delay line's flags and positions, readable.
pub(crate) fn line_summary(l: &DelayLine) -> String {
    format!("line (live {}, fresh {}, write position {}, history {} of {})", l.live, l.fresh, l.idx, l.written, l.len)
}

/// An edge's read of a delay line (the delay it plays, its crossfade).
pub(crate) fn edge_print(e: &EdgeTap, h: &mut dyn Hasher) {
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
                let mut plugins = Fnv::default();
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
                let mut lines = Fnv::default();
                for k in &st.key_in {
                    k.fingerprint(&mut lines);
                }
                line_print(&st.tap, &mut lines);
                line_print(&st.tap_pre, &mut lines);
                for e in &st.edges {
                    edge_print(e, &mut lines);
                }
                tables.keys.fingerprint_of(i, &mut lines);
                let mut midi = Fnv::default();
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
                        let mut h = Fnv::default();
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

//! Sidechain keys (ADR 0015 slice c): the buffers an insert detects on in place of its own input.
//!
//! The plan compiler lays out one [`KeyEntry`] per keyed insert, with a buffer as wide as the
//! keyed strip. The render fills an entry from its source (a strip's tap or a bus) just before
//! the keyed strip's layer runs, when everything that source depends on has rendered, and the
//! keyed strip reads it through a shared reference: layers are disjoint, so the borrow is safe.
//! Nothing here allocates once the bank is built.
//!
//! Delay compensation (ADR 0015 section 4): at a keyed insert the key and the strip's own signal
//! must come from the same source sample time. [`KeyBank::align`] finds the difference of their
//! arrival latencies each block; a key that arrives late delays the keyed strip's input before
//! the insert ([`KeyIn`], part of the strip's latency), a key that arrives early is delayed in its
//! [`KeyEntry`]. Both are [`KeyLine`]s sized when the plan is built.
//!
//! Render workers (issue #174, ADR 0029 amendment): a keyed strip a worker renders reads its keys
//! from its job, not from the plan's bank. The callback fills the strip's entries in the bank
//! when its sources are ready, then lends their storage (buffers, delay line, scratch) to the
//! job's bank ([`KeyBank::lend`]) and takes it back when the strip comes home
//! ([`KeyBank::take_back`]): swaps of vectors, nothing allocated, and the entry's history goes
//! with it. A [`KeyIn`] has its own scratch, so its line runs wherever the strip renders.

use super::*;

/// Where a key is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyEnd {
    /// A strip, by table index: its signal after the fader, or before it for a pre-fader key.
    Strip(u16),
    /// A bus.
    Bus(BusId),
}

/// One keyed insert of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeySpec {
    /// The keyed strip's table index and track.
    pub keyed: u16,
    pub id: TrackId,
    pub slot: u16,
    pub from: KeyEnd,
    pub pre: bool,
    /// Channels of the keyed strip: the key is mapped onto them.
    pub ch: u16,
}

/// A fixed delay applied in place to a block, over a delay line sized by the plan: the same
/// history and crossfade rules as a strip's edges, so a change of delay never reads silence the
/// line never held.
pub(crate) struct KeyLine {
    line: DelayLine,
    tap: EdgeTap,
}

impl KeyLine {
    /// A line for `ch` channels holding delays up to `cap` samples, in blocks up to `block`.
    pub(crate) fn new(ch: usize, cap: usize, block: usize) -> Self {
        KeyLine { line: DelayLine::with_capacity(ch, cap, block), tap: EdgeTap::default() }
    }

    /// The longest delay the line holds.
    pub(crate) fn cap(&self) -> usize {
        self.line.len
    }

    /// Delays the first `frames` of `buf` by `want` samples in place (`scratch` has a channel per
    /// channel of `buf`, with room for a block). With no delay and none still ringing, the block is
    /// left alone and the line goes idle.
    pub(crate) fn apply(&mut self, want: usize, buf: &mut [Vec<f32>], scratch: &mut [Vec<f32>], frames: usize) {
        if want == 0 && self.tap.holds() == 0 {
            self.line.idle();
            return;
        }
        self.line.write(buf, frames);
        let (from, to, done) = self.tap.step(want, false, &self.line, frames);
        if from == 0 && to == 0 {
            return;
        }
        self.line.read_x(from, to, done, scratch, frames);
        for (d, s) in buf.iter_mut().zip(scratch.iter()) {
            if let (Some(d), Some(s)) = (d.get_mut(..frames), s.get(..frames)) {
                d.copy_from_slice(s);
            }
        }
    }

    /// The line was not run this block: its history has a gap.
    pub(crate) fn idle(&mut self) {
        self.line.idle();
    }

    /// Takes over `old`'s history (a plan that keeps the strip).
    pub(crate) fn take_history(&mut self, old: &KeyLine) {
        self.line.take_history(&old.line);
        self.tap = old.tap;
    }

    /// Test seam (issue #210): the line's history, its write position and its read.
    #[cfg(feature = "testing")]
    pub(crate) fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        crate::fingerprint::line_print(&self.line, h);
        crate::fingerprint::edge_print(&self.tap, h);
    }

    /// Test seam (issue #210): the line's flags and positions, readable.
    #[cfg(feature = "testing")]
    pub(crate) fn summary(&self) -> String {
        format!("{} {:?}", crate::fingerprint::line_summary(&self.line), self.tap)
    }

    /// Test view: whether the line holds history (it was written and not idled since), its write
    /// position (it moves with every block written) and how much history it holds (a reset
    /// starts it again from one block).
    #[cfg(test)]
    pub(crate) fn history(&self) -> (bool, usize, usize) {
        (self.line.live, self.line.idx, self.line.written)
    }
}

/// The delay of a keyed strip's own signal before one keyed insert (the key arrives late).
pub(crate) struct KeyIn {
    pub(crate) slot: u16,
    /// This block's delay, set by [`KeyBank::align`].
    pub(crate) want: usize,
    pub(crate) line: KeyLine,
    /// The insert ran this block.
    pub(crate) ran: bool,
    /// Scratch for the line's delayed read (channels x block): the strip's own, so the line runs
    /// the same on the callback and on a render worker.
    pub(crate) scratch: Vec<Vec<f32>>,
}

impl KeyIn {
    /// Test seam (issue #210): this block's delay, whether the insert ran, and the line.
    #[cfg(feature = "testing")]
    pub(crate) fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        h.write_u16(self.slot);
        h.write_usize(self.want);
        h.write_u8(u8::from(self.ran));
        self.line.fingerprint(h);
    }

    pub(crate) fn new(slot: u16, ch: usize, cap: usize, block: usize) -> Self {
        KeyIn { slot, want: 0, line: KeyLine::new(ch, cap, block), ran: false, scratch: (0..ch).map(|_| Vec::with_capacity(block)).collect() }
    }
}

struct KeyEntry {
    spec: KeySpec,
    /// The entry's storage is lent to a render worker's job ([`KeyBank::lend`]).
    lent: bool,
    buf: Vec<Vec<f32>>,
    /// Delays the key when it arrives before the keyed strip's signal, by `want` this block.
    line: KeyLine,
    scratch: Vec<Vec<f32>>,
    want: usize,
}

/// The key buffers of a plan.
#[derive(Default)]
pub(crate) struct KeyBank {
    entries: Vec<KeyEntry>,
}

pub(crate) struct KeyAlignment {
    pub at: usize,
    pub arrive: usize,
    pub pos: Samples,
    pub pdc: bool,
    pub duplex: bool,
}

impl KeyBank {
    /// A bank of no entries.
    pub(crate) const EMPTY: KeyBank = KeyBank { entries: Vec::new() };

    /// The entries for `specs`, each with a silent buffer of `block` frames and a delay line
    /// holding up to `cap_of(keyed strip)` samples.
    pub(crate) fn new(specs: &[KeySpec], block: usize, cap_of: impl Fn(u16) -> usize) -> Self {
        let entries = specs
            .iter()
            .map(|&spec| {
                let ch = usize::from(spec.ch).max(1);
                KeyEntry {
                    spec,
                    lent: false,
                    buf: vec![vec![0.0; block]; ch],
                    line: KeyLine::new(ch, cap_of(spec.keyed), block),
                    scratch: (0..ch).map(|_| Vec::with_capacity(block)).collect(),
                    want: 0,
                }
            })
            .collect();
        KeyBank { entries }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// An empty bank with room for `n` entries lent to it ([`KeyBank::lend`]), allocated now so a
    /// lend never allocates.
    pub(crate) fn with_room(n: usize) -> Self {
        KeyBank { entries: Vec::with_capacity(n) }
    }

    /// Lends the storage of every entry keyed at table index `keyed` to `to` (a render worker's
    /// job), in bank order: `to` then serves those keys ([`KeyBank::get`]), this bank keeps their
    /// specs and this block's delays. False, with nothing lent, when `to` has no room for them or
    /// still holds entries. Allocation-free.
    pub(crate) fn lend(&mut self, keyed: usize, to: &mut KeyBank) -> bool {
        let n = self.entries.iter().filter(|e| usize::from(e.spec.keyed) == keyed).count();
        if !to.entries.is_empty() || to.entries.capacity() < n {
            return false;
        }
        for e in self.entries.iter_mut().filter(|e| usize::from(e.spec.keyed) == keyed && !e.lent) {
            let mut v = KeyEntry {
                spec: e.spec,
                lent: false,
                buf: Vec::new(),
                line: KeyLine { line: DelayLine::placeholder(), tap: EdgeTap::default() },
                scratch: Vec::new(),
                want: e.want,
            };
            std::mem::swap(&mut e.buf, &mut v.buf);
            std::mem::swap(&mut e.line, &mut v.line);
            std::mem::swap(&mut e.scratch, &mut v.scratch);
            e.lent = true;
            to.entries.push(v);
        }
        true
    }

    /// Takes back what [`KeyBank::lend`] lent to `from` for table index `keyed`, and empties
    /// `from`. Nothing happens when nothing was lent. Allocation-free (the emptied entries hold
    /// no storage).
    pub(crate) fn take_back(&mut self, keyed: usize, from: &mut KeyBank) {
        let mut lent = from.entries.iter_mut();
        for e in self.entries.iter_mut().filter(|e| usize::from(e.spec.keyed) == keyed && e.lent) {
            let Some(v) = lent.next() else { break };
            std::mem::swap(&mut e.buf, &mut v.buf);
            std::mem::swap(&mut e.line, &mut v.line);
            std::mem::swap(&mut e.scratch, &mut v.scratch);
            e.lent = false;
        }
        from.entries.clear();
    }

    /// The strip sources of the entries keyed at table index `keyed`.
    pub(crate) fn sources_of(&self, keyed: usize) -> impl Iterator<Item = usize> + '_ {
        self.entries.iter().filter(move |e| usize::from(e.spec.keyed) == keyed).filter_map(|e| match e.spec.from {
            KeyEnd::Strip(j) => Some(usize::from(j)),
            KeyEnd::Bus(_) => None,
        })
    }

    /// Test view of the entries keyed at `keyed`: whether each line holds history and its write
    /// position, in bank order (a skipped block neither writes nor idles them).
    #[cfg(test)]
    pub(crate) fn lines_of(&self, keyed: usize) -> Vec<(bool, usize, usize)> {
        self.entries.iter().filter(|e| usize::from(e.spec.keyed) == keyed).map(|e| e.line.history()).collect()
    }

    /// Test seam (issue #210): the entries keyed at `keyed`, in bank order: each one's delay
    /// and line (an entry lent to a render worker's job is skipped: the strip is away).
    #[cfg(feature = "testing")]
    pub(crate) fn fingerprint_of(&self, keyed: usize, h: &mut dyn std::hash::Hasher) {
        for e in self.entries.iter().filter(|e| usize::from(e.spec.keyed) == keyed && !e.lent) {
            h.write_u16(e.spec.slot);
            h.write_usize(e.want);
            e.line.fingerprint(h);
        }
    }

    /// Test seam (issue #210): the entries keyed at `keyed`, readable.
    #[cfg(feature = "testing")]
    pub(crate) fn summary_of(&self, keyed: usize) -> Vec<String> {
        let mine = self.entries.iter().filter(|e| usize::from(e.spec.keyed) == keyed && !e.lent);
        mine.map(|e| format!("key entry (slot {}): want {}, {}", e.spec.slot, e.want, e.line.summary())).collect()
    }

    /// Delay compensation for the keys of the strip at table index `at` (`t`'s): sets the delay
    /// of each key and of the strip's input before each keyed insert, and the strip's latency
    /// with them. `arrive` is the latency the strip's input arrives with; `source` gives the
    /// latency a key arrives with (its source's output). With `pdc` off nothing is delayed.
    /// `duplex`: the device has live input, so the strip's hardware inserts run and add their round trip.
    /// Returns the longest delay a line could not hold (0: none).
    pub(crate) fn align(&mut self, t: &Track, st: &mut Strip, alignment: KeyAlignment, source: &dyn Fn(&KeySpec) -> usize) -> usize {
        let KeyAlignment { at, arrive, pos, pdc, duplex } = alignment;
        for k in st.key_in.iter_mut() {
            k.want = 0;
        }
        for e in self.entries.iter_mut().filter(|e| usize::from(e.spec.keyed) == at) {
            e.want = 0;
        }
        // A frozen track reports the latency of the chain its render replaces (`frozen_latency`): the
        // forward pass reads `st.latency`, so dropping it here made a frozen key source arrive early.
        let mut own = crate::head_latency(t, st, duplex);
        let mut short = 0usize;
        for (i, ins) in t.mixer.inserts.iter().enumerate() {
            // A hardware insert has no plug-in: its round trip is the latency before the next slot.
            if let Some(l) = crate::hardware::slot_latency(ins, &st.hw, i, duplex) {
                own = own.saturating_add(l);
                continue;
            }
            let (Some(ins), Some(Some(slot))) = (ins, st.plugins.get_mut(i)) else { continue };
            if !ins.active || ins.bypass || slot.failed {
                continue;
            }
            // Block-start values only: processing sends the breakpoints inside the block.
            apply_params(slot, ins, t, i, pos, 0);
            if pdc
                && ins.key.is_some()
                && let Some(e) = self.entries.iter_mut().find(|e| e.spec.id == t.id && usize::from(e.spec.slot) == i)
                && let Some(k) = st.key_in.iter_mut().find(|k| usize::from(k.slot) == i)
            {
                let key_at = source(&e.spec).min(MAX_LATENCY);
                let here = arrive.saturating_add(own);
                if key_at > here {
                    // The key is late: the signal waits for it.
                    let d = key_at - here;
                    if d > k.line.cap() {
                        short = short.max(d);
                    }
                    k.want = d.min(k.line.cap());
                    own = own.saturating_add(k.want);
                } else {
                    // The key is early: it waits for the signal.
                    let d = here - key_at;
                    if d > e.line.cap() {
                        short = short.max(d);
                    }
                    e.want = d.min(e.line.cap());
                }
            }
            own = own.saturating_add(slot.plugin.latency());
        }
        st.latency = own.min(MAX_LATENCY);
        short
    }

    /// The key of track `id`'s insert in `slot`, as last filled.
    pub(crate) fn get(&self, id: TrackId, slot: usize) -> Option<&[Vec<f32>]> {
        self.entries.iter().find(|e| e.spec.id == id && usize::from(e.spec.slot) == slot).map(|e| e.buf.as_slice())
    }

    /// Fills the keys of the strips with table index in `keyed` from `strip` (the strips rendered
    /// so far, by table index) and `busses`, over `frames` samples. A source that has not rendered
    /// (`strip` gives none) fills silence. An entry lent to a render worker's job is left alone.
    pub(crate) fn fill<'a>(&mut self, keyed: std::ops::Range<usize>, strip: impl Fn(usize) -> Option<&'a Strip>, busses: &BusTable, frames: usize) {
        for e in self.entries.iter_mut().filter(|e| keyed.contains(&usize::from(e.spec.keyed)) && !e.lent) {
            let src: Option<&[Vec<f32>]> = match e.spec.from {
                KeyEnd::Strip(i) => strip(usize::from(i)).map(|st| if e.spec.pre { st.pre.as_slice() } else { st.buf.as_slice() }),
                KeyEnd::Bus(b) => busses.bus(b).map(|b| b.buf.as_slice()),
            };
            transit_dsp::map_key_channels(src.unwrap_or(&[]), &mut e.buf, 0, frames);
            e.line.apply(e.want, &mut e.buf, &mut e.scratch, frames);
        }
    }
}

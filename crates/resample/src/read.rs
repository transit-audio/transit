//! One output sample at a fractional source position (ADR 0056, decisions 8.3 and 8.5).

use core::ops::Range;

use crate::table::{PHASES, SincTable, ZERO_CROSSINGS};

/// The largest rate the reader takes (decision 5.2's upper bound on `q`): above it the tap count, and
/// so the work per sample, would grow without the bound decision 8.8 budgets.
pub const MAX_RATE: f64 = 32.0;
/// The largest position magnitude the reader takes, 2^50 frames (far beyond decision 5.5's 2^40; a
/// position this large still carries a fraction below one frame).
pub const MAX_POSITION: f64 = (1u64 << 50) as f64;

/// A source the reader takes its taps from: frames of one or more channels at integer indices.
///
/// A frame outside the source reads 0 (decision 8.5: before the file's first frame or past its last).
/// A frame the source does not have yet (a disk window or an input ring not filled) also reads 0 here;
/// counting that miss is the caller's business. The reader asks only for the frames in
/// [`taps`]`(x, q)`, in ascending order.
pub trait Frames {
    /// Channels the source has.
    fn channels(&self) -> usize;
    /// Channel `channel`'s sample at frame `frame`, or 0 outside the source.
    fn sample(&self, channel: usize, frame: i64) -> f32;
}

/// One channel over a borrowed slice, frame 0 at index 0.
#[derive(Debug, Clone, Copy)]
pub struct Mono<'a>(pub &'a [f32]);

impl Frames for Mono<'_> {
    fn channels(&self) -> usize {
        1
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        if channel != 0 || frame < 0 {
            return 0.0;
        }
        usize::try_from(frame).ok().and_then(|i| self.0.get(i)).copied().unwrap_or(0.0)
    }
}

/// Interleaved frames over a borrowed slice (`channels` samples a frame, frame 0 first). A trailing
/// partial frame is not part of the source.
#[derive(Debug, Clone, Copy)]
pub struct Interleaved<'a> {
    data: &'a [f32],
    channels: usize,
    frames: usize,
}

impl<'a> Interleaved<'a> {
    /// Interleaved `data` of `channels` channels (at least 1).
    #[must_use]
    pub fn new(data: &'a [f32], channels: usize) -> Interleaved<'a> {
        let channels = channels.max(1);
        Interleaved { data, channels, frames: data.len() / channels }
    }

    /// Whole frames in the source.
    #[must_use]
    pub fn frames(&self) -> usize {
        self.frames
    }
}

impl Frames for Interleaved<'_> {
    fn channels(&self) -> usize {
        self.channels
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        match usize::try_from(frame) {
            Ok(f) if f < self.frames && channel < self.channels => self.data[f * self.channels + channel],
            _ => 0.0,
        }
    }
}

/// One borrowed slice per channel (planar), each with frame 0 at index 0; a shorter channel reads 0
/// past its end.
#[derive(Debug, Clone, Copy)]
pub struct Planar<'a>(pub &'a [&'a [f32]]);

impl Frames for Planar<'_> {
    fn channels(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        let Ok(f) = usize::try_from(frame) else { return 0.0 };
        self.0.get(channel).and_then(|c| c.get(f)).copied().unwrap_or(0.0)
    }
}

/// Whether the reader takes this position and rate: `x` finite with |x| ≤ [`MAX_POSITION`], `q` in
/// `(0, MAX_RATE]`. For anything else [`read_frame`] writes silence.
#[inline]
#[must_use]
pub fn accepts(x: f64, q: f64) -> bool {
    q > 0.0 && q <= MAX_RATE && (-MAX_POSITION..=MAX_POSITION).contains(&x)
}

/// The tap reach in source frames, `16·max(1, q)`: every tap lies strictly within this distance of the
/// position. For a streaming caller at a fixed rate it is the reader's latency (look-ahead) in source
/// frames: 16 at `q ≤ 1`, `16·q` above.
#[inline]
#[must_use]
pub fn reach(q: f64) -> f64 {
    ZERO_CROSSINGS as f64 * if q > 1.0 { q } else { 1.0 }
}

/// The halo a buffered caller keeps on each side of the read position, `ceil(16·max(1, q)) + 1` frames
/// (decision 8.6): every frame [`taps`] can return lies within it.
#[must_use]
pub fn halo(q: f64) -> usize {
    let r = reach(q.min(MAX_RATE));
    let c = r as usize;
    (if (c as f64) < r { c + 1 } else { c }) + 1
}

/// The direct-index path (decision 5.3): at `q = 1` exactly and an integer position, the read is the
/// source frame itself, bit for bit, and the caller indexes it directly instead of calling the reader.
/// `Some(frame)` then; `None` for every other rate or position.
#[inline]
#[must_use]
pub fn direct(x: f64, q: f64) -> Option<i64> {
    if q != 1.0 || !accepts(x, q) {
        return None;
    }
    let k = x as i64;
    if k as f64 == x { Some(k) } else { None }
}

#[inline]
fn tap_u(x: f64, k: i64, q: f64) -> f64 {
    let d = x - k as f64;
    let d = if d < 0.0 { -d } else { d };
    if q > 1.0 { d / q } else { d }
}

/// The frames the reader reads for one output sample at position `x` and rate `q` (decision 8.3):
/// every integer `k` whose scaled distance `u = |x − k| / max(1, q)`, formed in binary64, is below 16.
/// Empty when [`accepts`] refuses. At most `2·16·max(1, q)` frames, so at most 1,024.
#[must_use]
pub fn taps(x: f64, q: f64) -> Range<i64> {
    if !accepts(x, q) {
        return 0..0;
    }
    let r = reach(q);
    let limit = ZERO_CROSSINGS as f64;
    // Candidates two past the reach on each side, then trimmed by the exact test. The test is monotone
    // in |x − k|, so the taps are contiguous; at most three trims a side are ever needed (rounding of
    // `x ± r` moves a candidate by less than one frame), and the loops are bounded by four.
    let mut first = transit_math::f64::floor(x - r) as i64 - 1;
    let mut last = transit_math::f64::ceil(x + r) as i64 + 1;
    for _ in 0..4 {
        if tap_u(x, first, q) < limit {
            break;
        }
        first += 1;
    }
    for _ in 0..4 {
        if tap_u(x, last, q) < limit {
            break;
        }
        last -= 1;
    }
    first..last + 1
}

/// One output frame at source position `x` and rate `q` (decision 8.3), one value per channel in
/// `out` (channels past the source's read 0).
///
/// For every tap `k` of [`taps`]`(x, q)` in ascending order: `u = |x − k| / max(1, q)` in `f64`, the
/// coefficient [`SincTable::coefficient`]`(u)` in `f32`, and per channel `acc += sample·coefficient` in
/// `f32` from `acc = 0`; then, for `q > 1`, `acc·(1/q as f32)`. No fused multiply-add. The output
/// depends only on the source frames, `x` and `q`: no state, so a block split, a seek or a loop wrap
/// changes nothing. At `q ≤ 1` and an integer `x` it is the source frame exactly (a −0.0 sample reads
/// +0.0). A position or rate [`accepts`] refuses writes silence.
///
/// Real time: no allocation, no lock; at most 1,024 taps per frame (`2·16·max(1, q)`).
#[inline]
pub fn read_frame<S: Frames + ?Sized>(table: &SincTable, src: &S, x: f64, q: f64, out: &mut [f32]) {
    out.fill(0.0);
    let n = out.len().min(src.channels());
    let out = &mut out[..n];
    let range = taps(x, q);
    if n == 0 || range.is_empty() {
        return;
    }
    let mut tap = |k: i64, c: f32| {
        for (ch, acc) in out.iter_mut().enumerate() {
            *acc += src.sample(ch, k) * c;
        }
    };
    if q > 1.0 {
        for k in range {
            tap(k, table.coefficient(tap_u(x, k, q)));
        }
        let scale = (1.0 / q) as f32;
        for acc in out.iter_mut() {
            *acc *= scale;
        }
    } else {
        // At q <= 1 every tap's `u` is its exact distance `|x − k|` (x − k is exact: it keeps x's
        // fraction bits and is smaller), so `512·u` differs from tap to tap by exact multiples of 512 and
        // every tap on one side of x shares the fraction `512·u − floor(512·u)` bit for bit: the
        // coefficients are table reads at a fixed stride of 512, the same values
        // `SincTable::coefficient` gives tap by tap (pinned by `read_frame_is_decision_8_3_bit_for_bit`).
        let below = transit_math::f64::floor(x) as i64;
        let (mut i, f) = table.split(x - range.start as f64);
        for k in range.start..=below {
            tap(k, table.at(i, f));
            i = i.wrapping_sub(PHASES);
        }
        if below + 1 < range.end {
            let (mut i, f) = table.split((below + 1) as f64 - x);
            for k in below + 1..range.end {
                tap(k, table.at(i, f));
                i += PHASES;
            }
        }
    }
}

/// One channel's output at source position `x` and rate `q` over a mono slice: [`read_frame`] with
/// [`Mono`].
#[inline]
#[must_use]
pub fn read(table: &SincTable, src: &[f32], x: f64, q: f64) -> f32 {
    let mut out = [0.0f32];
    read_frame(table, &Mono(src), x, q, &mut out);
    out[0]
}

//! The sample tap of an insert that analyzes its input on the UI thread (the Tuner, #266): a
//! preallocated ring of the signed input summed to mono, next to the scope's peak tap.
//!
//! The compiler makes one [`SampleTap`] with each instance whose plug-in asks for it
//! (`Plugin::wants_sample_tap`) and registers it in the [`crate::ScopeHub`] under the track and the
//! insert's uid. The render writes only while the `enabled` flag is set, which the UI flips while
//! the panel is visible. Nothing here allocates or locks on the audio thread: relaxed stores into
//! the ring, then one release store of the write counter.
//!
//! The channels are summed, not averaged, so a source on one side of a stereo return reads at its
//! full level against the Tuner's gate. Like any mono sum, opposite-phase channels cancel.
//!
//! A reader takes the counter first and then the span that ends [`READ_MARGIN`] frames behind it,
//! so the block the render may be writing during the read never overlaps what is read: no reading
//! mixes old and new frames.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Frames the ring holds (a power of two): the longest detector window plus the read margin and a
/// block of the render in flight, with room to spare.
pub const SAMPLE_FRAMES: usize = 16_384;
/// Frames between the newest write and the end of what [`SampleTap::read`] returns: at least one
/// render block, so the frames the render may overwrite while a read runs are never in the read.
pub const READ_MARGIN: usize = 2048;
const MASK: usize = SAMPLE_FRAMES - 1;

/// The ring of one insert and its enable flag.
pub struct SampleTap {
    enabled: AtomicBool,
    /// The channels' sum per frame, as `f32` bits (not clipped).
    buf: Box<[AtomicU32]>,
    /// Frames written in all (wrapping).
    write: AtomicUsize,
}

impl Default for SampleTap {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleTap {
    /// A tap, off, with a silent ring.
    #[must_use]
    pub fn new() -> Self {
        SampleTap { enabled: AtomicBool::new(false), buf: (0..SAMPLE_FRAMES).map(|_| AtomicU32::new(0)).collect(), write: AtomicUsize::new(0) }
    }

    /// Bytes one tap holds (counted in the plan's prepared bytes).
    pub(crate) const BYTES: usize = SAMPLE_FRAMES * size_of::<AtomicU32>() + size_of::<SampleTap>();

    /// Whether the render writes to this tap.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Audio thread: stores the sum of the channels for each of the first `frames` frames of
    /// `buf`. Does nothing while disabled. Bounded by the ring size whatever the block length.
    /// Public so a headless tap (tests, `--no-audio`) can be fed the way the render feeds it.
    pub fn push(&self, buf: &[Vec<f32>], frames: usize) {
        if !self.enabled() || buf.is_empty() {
            return;
        }
        // One writer: the render thread.
        let w = self.write.load(Ordering::Relaxed);
        // A block longer than the ring leaves only its last frames.
        let first = frames.saturating_sub(SAMPLE_FRAMES);
        for i in first..frames {
            let mut sum = 0.0f32;
            for c in buf {
                let x = c.get(i).copied().unwrap_or(0.0);
                sum += if x.is_finite() { x } else { 0.0 };
            }
            if let Some(cell) = self.buf.get(w.wrapping_add(i) & MASK) {
                cell.store(sum.to_bits(), Ordering::Relaxed);
            }
        }
        self.write.store(w.wrapping_add(frames), Ordering::Release);
    }

    /// Frames written in all (wrapping): unchanged between two looks means the render wrote nothing
    /// in between (bypassed, stopped or removed source).
    #[must_use]
    pub fn written(&self) -> usize {
        self.write.load(Ordering::Acquire)
    }

    /// Copies `out.len()` frames (at most `SAMPLE_FRAMES - READ_MARGIN`; the older ones read 0.0) in
    /// time order, ending [`READ_MARGIN`] frames before the newest write. Returns how many frames
    /// of `out` the ring has received, saturated at `out.len()`.
    pub fn read(&self, out: &mut [f32]) -> usize {
        let end = self.write.load(Ordering::Acquire);
        let n = out.len().min(SAMPLE_FRAMES - READ_MARGIN);
        let skip = out.len() - n;
        let stop = end.wrapping_sub(READ_MARGIN);
        let start = stop.wrapping_sub(n);
        for (i, o) in out.iter_mut().enumerate() {
            *o = match i.checked_sub(skip) {
                Some(k) => self.buf.get(start.wrapping_add(k) & MASK).map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed))),
                None => 0.0,
            };
        }
        end.saturating_sub(READ_MARGIN).min(out.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_tap_writes_nothing_and_enabled_tap_keeps_the_sum_of_the_channels() {
        let t = SampleTap::new();
        let block = vec![vec![0.5f32; 4096], vec![-0.25f32; 4096]];
        t.push(&block, 4096);
        let mut out = [1.0f32; 8];
        assert_eq!(t.read(&mut out), 0);
        assert_eq!(t.written(), 0);
        assert!(out.iter().all(|x| *x == 0.0));
        t.set_enabled(true);
        t.push(&block, 4096);
        assert_eq!(t.written(), 4096);
        assert_eq!(t.read(&mut out), 8);
        assert!(out.iter().all(|x| (*x - 0.25).abs() < 1e-7), "signed, summed, not a peak");
    }

    #[test]
    fn a_source_on_one_channel_keeps_its_level_and_a_sum_is_not_clipped() {
        let t = SampleTap::new();
        t.set_enabled(true);
        let (left, silent, loud) = (vec![0.5f32; 4096], vec![0.0f32; 4096], vec![0.9f32; 4096]);
        t.push(&[left.clone(), silent], 4096);
        let mut out = [0.0f32; 4];
        t.read(&mut out);
        assert!(out.iter().all(|x| *x == 0.5), "a left-only source reads at its own level");
        t.push(&[left, loud], 4096);
        t.read(&mut out);
        assert!(out.iter().all(|x| (*x - 1.4).abs() < 1e-6), "above 1.0 survives: {out:?}");
        // The documented limit: opposite-phase channels cancel, as in any mono sum.
        let (a, b) = (vec![0.5f32; 4096], vec![-0.5f32; 4096]);
        t.push(&[a, b], 4096);
        t.read(&mut out);
        assert!(out.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn ring_wraps_and_a_long_block_leaves_its_last_frames_ending_a_margin_behind() {
        let t = SampleTap::new();
        t.set_enabled(true);
        let ramp: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        t.push(&[ramp], 20_000);
        let mut out = [0.0f32; 4];
        t.read(&mut out);
        let last = 19_999.0 - READ_MARGIN as f32;
        assert_eq!(out, [last - 3.0, last - 2.0, last - 1.0, last]);
        let mut big = vec![0.0f32; SAMPLE_FRAMES + 10];
        t.read(&mut big);
        assert_eq!(big.last().copied(), Some(last));
        assert_eq!(big[0], 0.0);
    }

    #[test]
    fn the_read_never_includes_the_newest_margin_frames() {
        let t = SampleTap::new();
        t.set_enabled(true);
        let ramp: Vec<f32> = (0..12_000).map(|i| i as f32).collect();
        t.push(&[ramp], 12_000);
        let mut out = vec![0.0f32; 8192];
        assert_eq!(t.read(&mut out), 8192);
        assert_eq!(out[8191], (12_000 - READ_MARGIN - 1) as f32);
        assert_eq!(out[0], (12_000 - READ_MARGIN - 8192) as f32);
        assert!(out.windows(2).all(|w| w[1] == w[0] + 1.0), "one unbroken span");
    }

    #[test]
    fn non_finite_samples_store_as_silence() {
        let t = SampleTap::new();
        t.set_enabled(true);
        t.push(&[vec![f32::NAN, f32::INFINITY, 0.5]], 3);
        t.push(&[vec![0.0; READ_MARGIN]], READ_MARGIN);
        let mut out = [9.0f32; 3];
        t.read(&mut out);
        assert_eq!(out, [0.0, 0.0, 0.5]);
        assert_eq!(t.written(), 3 + READ_MARGIN);
    }
}

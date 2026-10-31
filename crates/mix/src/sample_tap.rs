//! The sample tap of an insert that analyzes its input on the UI thread (the Tuner, #266): a
//! preallocated ring of the signed input averaged to mono, next to the scope's peak tap.
//!
//! The compiler makes one [`SampleTap`] with each instance whose plug-in asks for it
//! (`Plugin::wants_sample_tap`) and registers it in the [`crate::ScopeHub`] under the track and the
//! insert's uid. The render writes only while the `enabled` flag is set, which the UI flips while
//! the panel is visible. Nothing here allocates or locks on the audio thread: relaxed stores into
//! the ring, then one release store of the write counter. A reader may see a block half written;
//! the detector that reads it is a display, so that is accepted.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Frames the ring holds (a power of two): the longest detector window, with room to spare.
pub const SAMPLE_FRAMES: usize = 8192;
const MASK: usize = SAMPLE_FRAMES - 1;

/// The ring of one insert and its enable flag.
pub struct SampleTap {
    enabled: AtomicBool,
    /// The channels' mean per frame, as `f32` bits.
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

    /// Audio thread: stores the mean of the channels for each of the first `frames` frames of
    /// `buf`. Does nothing while disabled. Bounded by the ring size whatever the block length.
    pub(crate) fn push(&self, buf: &[Vec<f32>], frames: usize) {
        if !self.enabled() || buf.is_empty() {
            return;
        }
        // One writer: the render thread.
        let w = self.write.load(Ordering::Relaxed);
        // A block longer than the ring leaves only its last frames.
        let first = frames.saturating_sub(SAMPLE_FRAMES);
        let inv = 1.0 / buf.len() as f32;
        for i in first..frames {
            let mut sum = 0.0f32;
            for c in buf {
                let x = c.get(i).copied().unwrap_or(0.0);
                sum += if x.is_finite() { x } else { 0.0 };
            }
            if let Some(cell) = self.buf.get(w.wrapping_add(i) & MASK) {
                cell.store((sum * inv).to_bits(), Ordering::Relaxed);
            }
        }
        self.write.store(w.wrapping_add(frames), Ordering::Release);
    }

    /// Copies the last `out.len()` frames (at most the ring holds; older ones read 0.0) in time
    /// order, the newest last. Returns how many frames the ring has ever received, saturated at
    /// `out.len()`.
    pub fn read(&self, out: &mut [f32]) -> usize {
        let end = self.write.load(Ordering::Acquire);
        let n = out.len().min(SAMPLE_FRAMES);
        let skip = out.len() - n;
        let start = end.wrapping_sub(n);
        for (i, o) in out.iter_mut().enumerate() {
            *o = match i.checked_sub(skip) {
                Some(k) => self.buf.get(start.wrapping_add(k) & MASK).map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed))),
                None => 0.0,
            };
        }
        end.min(out.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_tap_writes_nothing_and_enabled_tap_keeps_the_mean_of_the_channels() {
        let t = SampleTap::new();
        let block = vec![vec![0.5f32; 64], vec![-0.25f32; 64]];
        t.push(&block, 64);
        let mut out = [1.0f32; 8];
        assert_eq!(t.read(&mut out), 0);
        assert!(out.iter().all(|x| *x == 0.0));
        t.set_enabled(true);
        t.push(&block, 64);
        assert_eq!(t.read(&mut out), 8);
        assert!(out.iter().all(|x| (*x - 0.125).abs() < 1e-7), "signed, not a peak");
    }

    #[test]
    fn ring_wraps_and_a_long_block_leaves_its_last_frames() {
        let t = SampleTap::new();
        t.set_enabled(true);
        let ramp: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        t.push(&[ramp], 20_000);
        let mut out = [0.0f32; 4];
        t.read(&mut out);
        assert_eq!(out, [19_996.0, 19_997.0, 19_998.0, 19_999.0]);
        let mut big = vec![0.0f32; SAMPLE_FRAMES + 10];
        t.read(&mut big);
        assert_eq!(big[10 + SAMPLE_FRAMES - 1], 19_999.0);
        assert_eq!(big[0], 0.0);
        assert_eq!(big[10], 20_000.0 - SAMPLE_FRAMES as f32);
    }

    #[test]
    fn non_finite_samples_store_as_silence() {
        let t = SampleTap::new();
        t.set_enabled(true);
        t.push(&[vec![f32::NAN, f32::INFINITY, 0.5]], 3);
        let mut out = [9.0f32; 3];
        t.read(&mut out);
        assert_eq!(out, [0.0, 0.0, 0.5]);
    }
}

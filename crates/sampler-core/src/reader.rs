//! Reading the audio: the [`RegionReader`] a source lends the core, the [`SourceBank`] that finds a
//! zone's source, [`SliceSource`] over borrowed slices, and the four-point [`cubic`].
//!
//! A reader answers one native frame of one channel at a time. The core decides which frames a
//! voice needs (its region, its loop and the four cubic taps around its phase) and never asks for a
//! frame at or past [`RegionReader::frames`]. A reader that does not have a frame yet (a disk
//! window not primed) answers `None`: the core plays it as zero and counts an underrun; it never
//! waits. A frame outside the zone's region is not a miss: the core reads it as zero without asking.

/// One source's audio as the core reads it: mono or stereo f32 frames at the source's own rate.
pub trait RegionReader {
    /// Channels: 1 (mono) or 2 (stereo); the core refuses to start a voice on any other count.
    fn channels(&self) -> usize;
    /// Native frames in the source.
    fn frames(&self) -> u64;
    /// The source's own sample rate (Hz).
    fn sample_rate(&self) -> f32;
    /// Native frame `frame` of `channel`, or `None` when it is not available now (a miss).
    fn sample(&self, channel: usize, frame: u64) -> Option<f32>;
}

/// The sources a patch's zones refer to by index.
pub trait SourceBank {
    /// Source `index`, if there is one.
    fn source(&self, index: usize) -> Option<&dyn RegionReader>;
}

impl SourceBank for [SliceSource<'_>] {
    fn source(&self, index: usize) -> Option<&dyn RegionReader> {
        self.get(index).map(|s| s as &dyn RegionReader)
    }
}

impl SourceBank for [&dyn RegionReader] {
    fn source(&self, index: usize) -> Option<&dyn RegionReader> {
        self.get(index).map(|s| *s as &dyn RegionReader)
    }
}

/// A source over borrowed, fully resident slices (one per channel): the desktop adapter's in-memory
/// sample and the tests'. Every frame below [`RegionReader::frames`] is available.
#[derive(Debug, Clone, Copy)]
pub struct SliceSource<'a> {
    left: &'a [f32],
    right: Option<&'a [f32]>,
    sample_rate: f32,
}

impl<'a> SliceSource<'a> {
    /// A mono source.
    #[must_use]
    pub fn mono(data: &'a [f32], sample_rate: f32) -> SliceSource<'a> {
        SliceSource { left: data, right: None, sample_rate }
    }

    /// A stereo source; its length is the shorter channel's.
    #[must_use]
    pub fn stereo(left: &'a [f32], right: &'a [f32], sample_rate: f32) -> SliceSource<'a> {
        let n = left.len().min(right.len());
        SliceSource { left: left.get(..n).unwrap_or_default(), right: Some(right.get(..n).unwrap_or_default()), sample_rate }
    }
}

impl RegionReader for SliceSource<'_> {
    fn channels(&self) -> usize {
        if self.right.is_some() { 2 } else { 1 }
    }

    fn frames(&self) -> u64 {
        self.left.len() as u64
    }

    fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    fn sample(&self, channel: usize, frame: u64) -> Option<f32> {
        let data = match (channel, self.right) {
            (0, _) => self.left,
            (1, Some(r)) => r,
            _ => return None,
        };
        usize::try_from(frame).ok().and_then(|i| data.get(i)).copied()
    }
}

/// Splits a non-negative phase into its whole frame and the fraction, as the legacy sampler did:
/// `i = pos as usize`, `t = (pos - i) as f32`.
#[inline]
#[must_use]
pub fn split(pos: f64) -> (u64, f32) {
    let i = pos as u64;
    (i, (pos - i as f64) as f32)
}

/// The four-point cubic (Catmull-Rom) through taps `xm1, x0, x1, x2` (frames `i - 1 ..= i + 2`) at
/// fraction `t`: the legacy sampler's `hermite` arithmetic, expression for expression. A whole
/// frame (`t == 0`) is `x0` itself; a non-finite result (a float file's NaN or infinity) is silence.
#[inline]
#[must_use]
pub fn cubic(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    if t == 0.0 {
        return if x0.is_finite() { x0 } else { 0.0 };
    }
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    if y.is_finite() { y } else { 0.0 }
}

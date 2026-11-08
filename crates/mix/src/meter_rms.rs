//! The strip meter's RMS reading (#395): a running mean square over a fixed window of frames,
//! measured on the audio thread from the same blocks as the peak, in fixed per-channel storage
//! prepared with the plan. The reader converts the linear RMS to dBFS.

/// The RMS window: 300 ms (neither ADR 0045 nor the design README names RMS ballistics; 300 ms is
/// the integration time of the classic RMS meter). It is a count of frames at the plan's rate.
pub const RMS_WINDOW_MS: u32 = 300;

/// Bins the window is kept in. The window slides with each block: the newest partial bin counts
/// in full and the oldest whole bin by the share the partial one has not yet replaced.
const BINS: usize = 10;

/// One channel's running mean square: the sums of squares of the last [`BINS`] whole bins and of
/// the bin being filled. Fixed size; pushing allocates nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RmsWindow {
    bins: [f64; BINS],
    /// Frames per bin; 0 for a meter not prepared for a rate (it reads silence).
    bin_frames: u32,
    cur: f64,
    cur_frames: u32,
    /// Where the next whole bin goes: the oldest bin once the window is full.
    next: usize,
    /// Whole bins measured so far, up to [`BINS`].
    filled: usize,
}

impl RmsWindow {
    /// A window of [`RMS_WINDOW_MS`] at `sample_rate` (rounded down to whole bins: 14 400 frames
    /// at 48 kHz, 13 230 at 44.1 kHz).
    pub(crate) fn new(sample_rate: u32) -> Self {
        let window = u64::from(sample_rate) * u64::from(RMS_WINDOW_MS) / 1000;
        let bin_frames = u32::try_from(window / BINS as u64).unwrap_or(u32::MAX).max(1);
        Self { bin_frames, ..Self::default() }
    }

    /// The window's length in frames.
    pub(crate) fn window_frames(&self) -> u64 {
        u64::from(self.bin_frames) * BINS as u64
    }

    /// Forgets every measured frame, keeping the window's length.
    pub(crate) fn reset(&mut self) {
        *self = Self { bin_frames: self.bin_frames, ..Self::default() };
    }

    /// Adds a block of samples (a non-finite sample counts as silence, as the peak skips it).
    pub(crate) fn push(&mut self, block: &[f32]) {
        if self.bin_frames == 0 {
            return;
        }
        for v in block {
            let v = if v.is_finite() { f64::from(*v) } else { 0.0 };
            self.cur += v * v;
            self.cur_frames += 1;
            if self.cur_frames >= self.bin_frames {
                if let Some(b) = self.bins.get_mut(self.next) {
                    *b = self.cur;
                }
                self.next = (self.next + 1) % BINS;
                self.filled = (self.filled + 1).min(BINS);
                self.cur = 0.0;
                self.cur_frames = 0;
            }
        }
    }

    /// The RMS over the window (linear), or over the frames measured so far while it fills.
    pub(crate) fn rms(&self) -> f32 {
        let whole: f64 = self.bins.iter().sum();
        let (sum, frames) = if self.filled == BINS {
            let oldest = self.bins.get(self.next).copied().unwrap_or(0.0);
            let share = f64::from(self.cur_frames) / f64::from(self.bin_frames.max(1));
            (whole - oldest * share + self.cur, self.window_frames() as f64)
        } else {
            (whole + self.cur, (self.filled as u64 * u64::from(self.bin_frames) + u64::from(self.cur_frames)) as f64)
        };
        if frames <= 0.0 || sum <= 0.0 { 0.0 } else { (sum / frames).sqrt() as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(w: &mut RmsWindow, frames: usize, block: usize, f: impl Fn(usize) -> f32) {
        let signal: Vec<f32> = (0..frames).map(f).collect();
        for chunk in signal.chunks(block) {
            w.push(chunk);
        }
    }

    #[test]
    fn the_window_is_a_count_of_frames_at_the_rate() {
        assert_eq!(RmsWindow::new(48_000).window_frames(), 14_400);
        assert_eq!(RmsWindow::new(44_100).window_frames(), 13_230);
        assert_eq!(RmsWindow::default().rms(), 0.0);
    }

    #[test]
    fn a_dc_level_reads_its_own_level_while_filling_and_full() {
        let mut w = RmsWindow::new(48_000);
        feed(&mut w, 1_000, 256, |_| 0.25);
        assert!((w.rms() - 0.25).abs() < 1e-6);
        feed(&mut w, 40_000, 333, |_| 0.25);
        assert!((w.rms() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn the_window_forgets_what_is_older_than_it() {
        let mut w = RmsWindow::new(48_000);
        feed(&mut w, 48_000, 256, |_| 1.0);
        let past = w.window_frames() as usize + 1_440;
        feed(&mut w, past, 256, |_| 0.0);
        assert_eq!(w.rms(), 0.0);
        feed(&mut w, 2_000, 100, |_| f32::NAN);
        assert_eq!(w.rms(), 0.0, "non-finite samples count as silence");
        w.reset();
        assert_eq!(w.rms(), 0.0);
        assert_eq!(w.window_frames(), 14_400);
    }
}

//! Live integrated loudness (ITU-R BS.1770-4, EBU R128) for the audio thread (#366).
//!
//! [`LiveLoudness`] keeps only what the gated integrated figure needs, in fixed-size state: no
//! allocation anywhere (not even in [`LiveLoudness::new`]), no lock, so it runs inside a device
//! callback. The bounce's offline integrator, [`crate::meter::LoudnessMeter`], is the reference:
//! the same K-weighting filters, the same 400 ms blocks at 75 percent overlap built from 100 ms
//! sub-blocks, the same absolute gate (−70 LUFS) and relative gate (−10 LU) applied the same way.
//! The one difference is the histogram that holds the block loudness history for the relative
//! gate: 0.1 LU bins from −70 to +10 LUFS (each bin keeps its block count and its summed energy,
//! so the figure itself is not quantized; only the relative gate's decision is, per bin center).
//! A block louder than +10 LUFS lands in the top bin with its exact energy.

use crate::biquad::Biquad;
use crate::meter::{k_weighting, lufs};
use crate::util::sane_sr;

/// The widest main bus the mixer builds (9.1.6).
pub const CHANNELS: usize = 16;
/// The histogram's floor (the absolute gate) and bin width, in LUFS and LU.
pub const HIST_MIN: f32 = -70.0;
pub const HIST_STEP: f32 = 0.1;
/// −70 ..= +10 LUFS.
pub const HIST_BINS: usize = 800;
/// Sub-blocks per gating block (4 × 100 ms = 400 ms; a new block every 100 ms is 75 percent overlap).
const WINDOW: usize = 4;

/// Gated integrated loudness, running since the last [`LiveLoudness::reset`].
#[derive(Debug, Clone)]
pub struct LiveLoudness {
    sr: f32,
    channels: usize,
    weights: [f32; CHANNELS],
    filters: [[Biquad; 2]; CHANNELS],
    block_len: usize,
    block_pos: usize,
    block_acc: [f64; CHANNELS],
    sub: [f64; WINDOW],
    sub_idx: usize,
    sub_filled: usize,
    hist_count: [u32; HIST_BINS],
    hist_energy: [f64; HIST_BINS],
    /// The integrated figure as of the last finished block; `None` before the first gated block.
    integrated: Option<f32>,
}

impl LiveLoudness {
    /// A meter for `channels` (clamped to 1 ..= [`CHANNELS`]) at `sample_rate`. Channel weights
    /// follow the offline meter: 1.0 each, except a 6-channel (5.1: L R C LFE Ls Rs) layout, which
    /// drops the LFE and weights the surrounds 1.41.
    #[must_use]
    pub fn new(sample_rate: f32, channels: usize) -> Self {
        let sr = sane_sr(sample_rate);
        let [c1, c2] = k_weighting(sr);
        let mut meter = Self {
            sr,
            channels: 0,
            weights: [1.0; CHANNELS],
            filters: [[Biquad::new(c1), Biquad::new(c2)]; CHANNELS],
            block_len: ((sr * 0.1).round() as usize).max(1),
            block_pos: 0,
            block_acc: [0.0; CHANNELS],
            sub: [0.0; WINDOW],
            sub_idx: 0,
            sub_filled: 0,
            hist_count: [0; HIST_BINS],
            hist_energy: [0.0; HIST_BINS],
            integrated: None,
        };
        meter.set_channels(channels);
        meter
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Follows a main bus that changed width: a different count resets the measurement.
    pub fn set_channels(&mut self, channels: usize) {
        let channels = channels.clamp(1, CHANNELS);
        if channels == self.channels {
            return;
        }
        self.channels = channels;
        self.weights = [1.0; CHANNELS];
        if channels == 6 {
            self.weights[3] = 0.0;
            self.weights[4] = 1.41;
            self.weights[5] = 1.41;
        }
        self.reset();
    }

    /// Starts the measurement over: filters, the block under way and the history.
    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f[0].reset();
            f[1].reset();
        }
        self.block_pos = 0;
        self.block_acc = [0.0; CHANNELS];
        self.sub = [0.0; WINDOW];
        self.sub_idx = 0;
        self.sub_filled = 0;
        self.hist_count = [0; HIST_BINS];
        self.hist_energy = [0.0; HIST_BINS];
        self.integrated = None;
    }

    /// Feeds planar audio: the first [`Self::channels`] buffers, `frames` of each. Non-finite
    /// samples count as silence, as in the offline meter.
    pub fn process(&mut self, io: &[Vec<f32>], frames: usize) {
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        for n in 0..frames {
            for c in 0..ch {
                let x = io.get(c).and_then(|b| b.get(n)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                if let (Some([f1, f2]), Some(acc)) = (self.filters.get_mut(c), self.block_acc.get_mut(c)) {
                    let y = f2.process_sample(f1.process_sample(x));
                    *acc += f64::from(y) * f64::from(y);
                }
            }
            self.block_pos += 1;
            if self.block_pos >= self.block_len {
                self.finish_sub_block();
            }
        }
    }

    fn finish_sub_block(&mut self) {
        let len = self.block_pos.max(1) as f64;
        let mut energy = 0.0;
        for (acc, w) in self.block_acc.iter_mut().zip(&self.weights) {
            energy += f64::from(*w) * *acc / len;
            *acc = 0.0;
        }
        self.block_pos = 0;
        if let Some(s) = self.sub.get_mut(self.sub_idx) {
            *s = energy;
        }
        self.sub_idx = (self.sub_idx + 1) % WINDOW;
        self.sub_filled = (self.sub_filled + 1).min(WINDOW);
        if self.sub_filled < WINDOW {
            return;
        }
        // Newest first, the offline meter's order of summation.
        let mut sum = 0.0;
        for k in 1..=WINDOW {
            sum += self.sub.get((self.sub_idx + WINDOW - k) % WINDOW).copied().unwrap_or(0.0);
        }
        let z = sum / WINDOW as f64;
        let l = lufs(z);
        if l > HIST_MIN {
            let bin = (((l - HIST_MIN) / HIST_STEP) as usize).min(HIST_BINS - 1);
            if let (Some(c), Some(e)) = (self.hist_count.get_mut(bin), self.hist_energy.get_mut(bin)) {
                *c = c.saturating_add(1);
                *e += z;
            }
            self.integrated = self.gated();
        }
    }

    /// The two-pass gate over the histogram: everything above the absolute gate gives the
    /// relative gate (its mean −10 LU); the bins whose center lies above that are the figure.
    fn gated(&self) -> Option<f32> {
        let (mut n, mut e) = (0u64, 0.0f64);
        for (c, s) in self.hist_count.iter().zip(&self.hist_energy) {
            n += u64::from(*c);
            e += s;
        }
        if n == 0 {
            return None;
        }
        let gate = lufs(e / n as f64) - 10.0;
        let (mut n2, mut e2) = (0u64, 0.0f64);
        for (i, (c, s)) in self.hist_count.iter().zip(&self.hist_energy).enumerate() {
            if HIST_MIN + (i as f32 + 0.5) * HIST_STEP > gate {
                n2 += u64::from(*c);
                e2 += s;
            }
        }
        (n2 > 0).then(|| lufs(e2 / n2 as f64))
    }

    /// Integrated loudness in LUFS since the last reset; `None` until a block passed the absolute gate.
    #[must_use]
    pub fn integrated_lufs(&self) -> Option<f32> {
        self.integrated
    }

    /// Blocks that passed the absolute gate since the last reset.
    #[must_use]
    pub fn gated_blocks(&self) -> u64 {
        self.hist_count.iter().map(|c| u64::from(*c)).sum()
    }
}

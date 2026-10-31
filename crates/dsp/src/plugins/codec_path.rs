//! The Texture's signal path at one codec rate (#241): the session's frames go down to the
//! quality's codec rate, gather into the firmware's blocks, and come back up.
//!
//! ```text
//! session rate ── down converter ── codec rate ── block of B frames ── firmware ── up converter ── session rate
//! ```
//!
//! **Latency.** The firmware renders a block once its last frame has arrived, so the path holds
//! `B − 1` codec frames; each converter adds its half length. The up converter waits a little more
//! (`pad` ticks) so the whole delay is a whole number of session samples, which
//! [`Path::latency`] reports to delay compensation and the tests measure. At equal rates (a 48 kHz
//! session with the digital or clean tape quality) both converters are exact copies and the
//! latency is `B − 1` = 11 frames.
//!
//! **Why the up converter never waits.** With `H` the session rate, `I` the codec rate and
//! `g = gcd(H, I)`, a session sample lasts `I/g` ticks and a codec frame `H/g`. Codec frame `k` is
//! ready once session sample `⌊k·H/I⌋` has arrived (the down converter's newest tap), and its
//! block's output once the block's last frame `k + B − 1` (at most) is ready. Session output `n`
//! reads codec frames up to `⌊(n·I/g − extra)/(H/g)⌋`; with `extra ≥ (B − 1)·H/g` those are always
//! rendered by the time output `n` is due. [`Path::starved`] counts any read that came too early
//! (the tests hold it at zero for every quality and rate).

use super::resample::{Converter, Kernel, half_taps, ticks};
use transit_texture::{MAX_BLOCK_SIZE, Media};

pub(crate) struct Path {
    /// The codec rate and its block.
    rate: u32,
    block: usize,
    down: Converter,
    up: Converter,
    latency: usize,
    frames: [[f32; 2]; MAX_BLOCK_SIZE],
    fill: usize,
    starved: u64,
}

impl Path {
    /// The path of `media`'s codec rate in a session at `host` Hz.
    pub(crate) fn new(kernel: &Kernel, host: u32, media: Media) -> Self {
        let rate = media.sample_rate();
        let block = media.block_size().clamp(1, MAX_BLOCK_SIZE);
        let b = i64::try_from(block).unwrap_or(1) - 1;
        let (down, up, latency) = if host == rate {
            (Converter::new(kernel, host, rate, 0), Converter::new(kernel, rate, host, b), block - 1)
        } else {
            // Up converter (codec to session): one codec frame is `lp` ticks, one session sample
            // `mp` ticks.
            let (lp, mp) = ticks(rate, host);
            let jd = half_taps(host, rate);
            let ju = half_taps(rate, host);
            let pad = (-(ju + b) * lp).rem_euclid(mp);
            let total = jd + ((ju + b) * lp + pad) / mp;
            (Converter::new(kernel, host, rate, 0), Converter::new(kernel, rate, host, b * lp + pad), usize::try_from(total).unwrap_or(0))
        };
        Self { rate, block, down, up, latency, frames: [[0.0; 2]; MAX_BLOCK_SIZE], fill: 0, starved: 0 }
    }

    /// The codec rate, Hz.
    pub(crate) fn rate(&self) -> u32 {
        self.rate
    }

    /// The path's delay, session samples (the dry signal comes out this much later).
    pub(crate) fn latency(&self) -> usize {
        self.latency
    }

    /// Output samples read before the firmware had rendered them (always 0; see the module doc).
    #[cfg(test)]
    pub(crate) fn starved(&self) -> u64 {
        self.starved
    }

    /// Back to silence: the converters' histories cleared, no frames gathered.
    pub(crate) fn reset(&mut self) {
        self.down.reset();
        self.up.reset();
        self.fill = 0;
    }

    /// One session frame in, one out. `block` renders a full block of codec frames (input, output).
    #[inline]
    pub(crate) fn tick(&mut self, kernel: &Kernel, x: [f32; 2], block: &mut impl FnMut(&[[f32; 2]], &mut [[f32; 2]])) -> [f32; 2] {
        self.down.push(x);
        while self.down.ready() {
            let s = self.down.pull(kernel);
            if let Some(f) = self.frames.get_mut(self.fill) {
                *f = s;
            }
            self.fill += 1;
            if self.fill >= self.block {
                let mut out = [[0.0f32; 2]; MAX_BLOCK_SIZE];
                let n = self.block;
                block(&self.frames[..n], &mut out[..n]);
                for o in &out[..n] {
                    self.up.push(*o);
                }
                self.fill = 0;
            }
        }
        if !self.up.ready() {
            self.starved += 1;
        }
        self.up.pull(kernel)
    }
}

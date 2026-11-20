//! A firmware's signal path at its codec rate (the Texture's per quality, #241; the Time
//! Stretcher's at 32 kHz, #199): the session's frames go down to the codec rate, gather into the
//! firmware's blocks, and come back up.
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
//! latency is `B − 1` frames (11 for the Texture's 12-frame blocks).
//!
//! **Why the up converter never waits.** With `H` the session rate, `I` the codec rate and
//! `g = gcd(H, I)`, a session sample lasts `I/g` ticks and a codec frame `H/g`. Codec frame `k` is
//! ready once session sample `⌊k·H/I⌋` has arrived (the down converter's newest tap), and its
//! block's output once the block's last frame `k + B − 1` (at most) is ready. Session output `n`
//! reads codec frames up to `⌊(n·I/g − extra)/(H/g)⌋`; with `extra ≥ (B − 1)·H/g` those are always
//! rendered by the time output `n` is due. [`Path::starved`] counts any read that came too early
//! (the tests hold it at zero for every quality and rate).

use super::resample::{Converter, Kernel, half_taps, ticks};

/// The longest firmware block a path gathers, frames (the Time Stretcher's 32; the Texture's are
/// 12, 8 and 6).
pub(crate) const MAX_BLOCK_SIZE: usize = 32;

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

transit_texture::state_fields!(Path, s, [down, up, frames, fill], s.fill < s.block);

impl Path {
    /// An upper bound on the bytes of this path's checkpoint at a session rate of `host` Hz and a
    /// firmware rate of `rate` Hz.
    pub(crate) fn state_bound(host: u32, rate: u32) -> usize {
        // Both converters, the gathered frames and the fill counter (a usize, at most 8 bytes).
        super::resample::state_bound(host, rate) + super::resample::state_bound(rate, host) + MAX_BLOCK_SIZE * 2 * 4 + 8
    }

    /// The path of a firmware running at `rate` Hz in blocks of `block` frames (at most
    /// [`MAX_BLOCK_SIZE`]), in a session at `host` Hz.
    pub(crate) fn new(kernel: &Kernel, host: u32, rate: u32, block: usize) -> Self {
        let block = block.clamp(1, MAX_BLOCK_SIZE);
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

/// Test seam (feature `testing`; ADR 0029, amendment for #742): the path's state record.
/// Configuration (fixed by the rates and the block at construction, which the owner's
/// configuration child pins): `rate`, `block`, `latency`, and each converter's own configuration.
/// Encoded: both converters' state, every gathered frame (all of `frames`, so no overwrite
/// argument is needed for the ones past `fill`), `fill`. Unobservable: `starved`, a count no
/// rendering reads (the tests' `starved()` only).
#[cfg(feature = "testing")]
impl Path {
    pub(crate) fn record_state(&self, w: &mut transit_state_record::Writer<'_>) {
        let Path { rate: _, block: _, down, up, latency: _, frames, fill, starved: _ } = self;
        down.record_state(w);
        up.record_state(w);
        for f in frames {
            w.f32s(f);
        }
        w.usize(*fill);
    }

    pub(crate) fn restore_state(&mut self, r: &mut transit_state_record::Reader<'_>) -> Option<()> {
        let Path { rate: _, block, down, up, latency: _, frames, fill, starved: _ } = self;
        down.restore_state(r)?;
        up.restore_state(r)?;
        for f in frames.iter_mut() {
            r.f32s(f)?;
        }
        *fill = r.usize().filter(|x| x < block)?;
        Some(())
    }

    /// Poison witnesses only: overwrites `starved` and both converters' scratch; returns the
    /// values written.
    pub(crate) fn poison_unobservable(&mut self, seed: u32) -> u64 {
        self.starved = self.starved.wrapping_add(u64::from(seed) | 1);
        1 + self.down.poison_unobservable(seed) + self.up.poison_unobservable(seed.wrapping_add(1))
    }

    /// Codec frames gathered toward the next firmware block (the witnesses' observation).
    pub(crate) fn fill(&self) -> usize {
        self.fill
    }
}

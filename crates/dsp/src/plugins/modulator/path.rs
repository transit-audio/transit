//! The Cross Modulator's signal path (#200): the track and the key go down to the firmware's
//! 96 kHz, gather into its 60-frame blocks, and the output comes back up to the session rate.
//!
//! ```text
//! track, key ── down converters ── 96 kHz ── block of 60 frames ── firmware ── up converter ── session rate
//! ```
//!
//! The converters are the Texture's ([`super::super::resample`]): streaming windowed-sinc
//! converters timed on the rates' common tick, so the output does not depend on the host's block
//! size, and exact copies at equal rates (a 96 kHz session converts nothing: the firmware hears the
//! session's samples and its output is the session's output, bit for bit).
//!
//! **Latency.** The firmware renders a block once its last frame has arrived, so the path holds
//! 59 codec frames; each converter adds its half length, and the up converter waits a few ticks
//! more (`pad`) so the whole delay is a whole number of session samples ([`Path::latency`]). The
//! argument that the up converter never reads a frame before the firmware rendered it is the
//! Texture's (`texture/path.rs`); [`Path::starved`] counts any such read and the tests hold it at
//! zero.

use super::super::resample::{Converter, Kernel, half_taps, ticks};
use transit_modulator::BLOCK_SIZE;

/// The firmware's rate, Hz.
pub(crate) const RATE: u32 = 96_000;

/// One gathered codec frame: the track's two channels, then the key's.
pub(crate) type Frame = [f32; 4];

pub(crate) struct Path {
    track: Converter,
    key: Converter,
    up: Converter,
    latency: usize,
    frames: [Frame; BLOCK_SIZE],
    fill: usize,
    starved: u64,
}

/// The path's latency in a session at `host` Hz, session samples, and the up converter's extra
/// delay in ticks.
pub(crate) fn latency_at(host: u32) -> (usize, i64) {
    let b = i64::try_from(BLOCK_SIZE).unwrap_or(60) - 1;
    if host == RATE {
        return (BLOCK_SIZE - 1, b);
    }
    // One codec frame is `lp` ticks, one session sample `mp` ticks.
    let (lp, mp) = ticks(RATE, host);
    let jd = half_taps(host, RATE);
    let ju = half_taps(RATE, host);
    let pad = (-(ju + b) * lp).rem_euclid(mp);
    let total = jd + ((ju + b) * lp + pad) / mp;
    (usize::try_from(total).unwrap_or(0), b * lp + pad)
}

impl Path {
    /// The path in a session at `host` Hz.
    pub(crate) fn new(kernel: &Kernel, host: u32) -> Self {
        let (latency, extra) = latency_at(host);
        Self {
            track: Converter::new(kernel, host, RATE, 0),
            key: Converter::new(kernel, host, RATE, 0),
            up: Converter::new(kernel, RATE, host, extra),
            latency,
            frames: [[0.0; 4]; BLOCK_SIZE],
            fill: 0,
            starved: 0,
        }
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
        self.track.reset();
        self.key.reset();
        self.up.reset();
        self.fill = 0;
    }

    /// One session frame of the track and the key in, one frame out. `block` renders a full block
    /// of codec frames into the two output channels.
    #[inline]
    pub(crate) fn tick(&mut self, kernel: &Kernel, track: [f32; 2], key: [f32; 2], block: &mut impl FnMut(&[Frame], &mut [[f32; 2]])) -> [f32; 2] {
        self.track.push(track);
        self.key.push(key);
        while self.track.ready() {
            let t = self.track.pull(kernel);
            let k = self.key.pull(kernel);
            if let Some(f) = self.frames.get_mut(self.fill) {
                *f = [t[0], t[1], k[0], k[1]];
            }
            self.fill += 1;
            if self.fill >= BLOCK_SIZE {
                let mut out = [[0.0f32; 2]; BLOCK_SIZE];
                block(&self.frames, &mut out);
                for o in &out {
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

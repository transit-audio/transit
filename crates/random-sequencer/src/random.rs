//! The random source and the deja vu loop: `marbles/random/random_generator.h`,
//! `random_stream.h` and `random_sequence.h`.
//!
//! On the module a hardware random number generator fills a 128-word buffer the stream reads first,
//! falling back to a linear congruential generator when it is empty. Here nothing fills it, so
//! every word comes from the generator, seeded per instance: the same seed gives the same
//! sequence, as the oracle (whose stream is never written either) gives it.

#![forbid(unsafe_code)]

use crate::float::F;

/// `kDejaVuBufferSize`, `kHistoryBufferSize`.
const LOOP_SIZE: usize = 16;
const HISTORY_SIZE: usize = 16;

/// `kMaxUint32`: 2^32 as a float.
const MAX_UINT32: f32 = 4_294_967_296.0;

/// One step of the firmware's linear congruential generator (`x * 1664525 + 1013904223`, kept to
/// 32 bits).
#[inline(always)]
#[must_use]
pub fn lcg(x: u32) -> u32 {
    x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

/// `RandomStream` over its fallback `RandomGenerator`: the generator alone (see the module
/// documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandomStream {
    state: u32,
}

impl RandomStream {
    /// `RandomGenerator::Init(seed)` and `RandomStream::Init`.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// `GetWord()`.
    pub fn get_word(&mut self) -> u32 {
        self.state = lcg(self.state);
        self.state
    }

    /// `GetFloat()`: a word over 2^32 (exactly 1.0 for the top 128 words).
    pub fn get_float(&mut self) -> F {
        F::from_u32(self.get_word()) / MAX_UINT32
    }
}

/// `RandomSequence`: a loop of 16 values the deja vu control replays, mutates or shuffles, and a
/// history that lets other channels replay what one drew. The C++'s pointers into the loop and
/// the history are indices here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RandomSequence {
    loop_: [F; LOOP_SIZE],
    history: [F; HISTORY_SIZE],
    loop_write_head: i32,
    length: i32,
    step: i32,
    record_head: i32,
    replay_head: i32,
    replay_start: i32,
    replay_hash: u32,
    replay_shift: u32,
    deja_vu: F,
    redo_read: usize,
    redo_write: Option<usize>,
    redo_write_history: Option<usize>,
}

impl RandomSequence {
    /// `Init(random_stream)`: the loop filled from the stream.
    #[must_use]
    pub fn new(stream: &mut RandomStream) -> Self {
        let mut loop_ = [F::ZERO; LOOP_SIZE];
        for v in &mut loop_ {
            *v = stream.get_float();
        }
        Self {
            loop_,
            history: [F::ZERO; HISTORY_SIZE],
            loop_write_head: 0,
            length: 8,
            step: 0,
            record_head: 0,
            replay_head: -1,
            replay_start: 0,
            replay_hash: 0,
            replay_shift: 0,
            deja_vu: F::ZERO,
            redo_read: 0,
            redo_write: None,
            redo_write_history: None,
        }
    }

    /// `Record()`.
    pub fn record(&mut self) {
        self.replay_start = self.record_head;
        self.replay_head = -1;
    }

    /// `ReplayPseudoRandom(hash)`.
    pub fn replay_pseudo_random(&mut self, hash: u32) {
        self.replay_head = self.replay_start;
        self.replay_hash = hash;
        self.replay_shift = 0;
    }

    /// `ReplayShifted(shift)`.
    pub fn replay_shifted(&mut self, shift: u32) {
        self.replay_head = self.replay_start;
        self.replay_hash = 0;
        self.replay_shift = shift;
    }

    fn history_at(&self, i: usize) -> F {
        self.history.get(i % HISTORY_SIZE).copied().unwrap_or(F::ZERO)
    }

    /// `GetReplayValue()`.
    #[must_use]
    pub fn replay_value(&self) -> F {
        // `(replay_head_ - 1 - replay_shift_ + 2 * kHistoryBufferSize) % kHistoryBufferSize` in
        // unsigned arithmetic (the shift is unsigned).
        #[allow(clippy::cast_sign_loss)] // As the C++'s conversion of the int to unsigned.
        let h = ((self.replay_head - 1) as u32).wrapping_sub(self.replay_shift).wrapping_add(2 * HISTORY_SIZE as u32) % HISTORY_SIZE as u32;
        let v = self.history_at(h as usize);
        if self.replay_hash == 0 {
            v
        } else {
            let word = (v * MAX_UINT32).trunc_u32();
            let word = lcg(word ^ self.replay_hash);
            F::from_u32(word) / MAX_UINT32
        }
    }

    /// `RewriteValue(value)`.
    pub fn rewrite_value(&mut self, value: F) -> F {
        if self.replay_head >= 0 {
            return self.replay_value();
        }
        if let Some(w) = self.redo_write.and_then(|i| self.loop_.get_mut(i)) {
            *w = 1.0 + value;
        }
        let mut result = self.loop_.get(self.redo_read).copied().unwrap_or(F::ZERO);
        if result >= 1.0 {
            result -= 1.0;
        } else {
            result = F(0.5);
        }
        if let Some(h) = self.redo_write_history.and_then(|i| self.history.get_mut(i)) {
            *h = result;
        }
        result
    }

    /// `NextValue(deterministic, value)`.
    pub fn next_value(&mut self, stream: &mut RandomStream, deterministic: bool, value: F) -> F {
        if self.replay_head >= 0 {
            self.replay_head = (self.replay_head + 1) % HISTORY_SIZE as i32;
            return self.replay_value();
        }
        let p_sqrt = 2.0 * self.deja_vu - 1.0;
        let p = p_sqrt * p_sqrt;
        let mutate = stream.get_float() < p;
        if mutate && self.deja_vu <= 0.5 {
            // A new value at the end of the loop.
            let at = usize::try_from(self.loop_write_head).unwrap_or(0) % LOOP_SIZE;
            self.redo_write = Some(at);
            let v = if deterministic { 1.0 + value } else { stream.get_float() };
            if let Some(slot) = self.loop_.get_mut(at) {
                *slot = v;
            }
            self.loop_write_head = (self.loop_write_head + 1) % LOOP_SIZE as i32;
            self.step = self.length - 1;
        } else {
            self.redo_write = None;
            if mutate {
                self.step = (stream.get_float() * F::from_i32(self.length)).trunc_i32();
            } else {
                self.step += 1;
                if self.step >= self.length {
                    self.step = 0;
                }
            }
        }
        #[allow(clippy::cast_sign_loss)] // `uint32_t i = ...`: the sum is never negative.
        let i = (self.loop_write_head + LOOP_SIZE as i32 - self.length + self.step) as u32;
        self.redo_read = i as usize % LOOP_SIZE;
        let mut result = self.loop_.get(self.redo_read).copied().unwrap_or(F::ZERO);
        if result >= 1.0 {
            result -= 1.0;
        } else if deterministic {
            // A deterministic value was asked for, but the loop holds random ones.
            result = F(0.5);
        }
        let at = usize::try_from(self.record_head).unwrap_or(0) % HISTORY_SIZE;
        self.redo_write_history = Some(at);
        if let Some(h) = self.history.get_mut(at) {
            *h = result;
        }
        self.record_head = (self.record_head + 1) % HISTORY_SIZE as i32;
        result
    }

    /// `NextVector(destination, size)`: one value from the loop seeds a short run of the linear
    /// congruential generator.
    pub fn next_vector(&mut self, stream: &mut RandomStream, destination: &mut [F]) {
        let seed = self.next_value(stream, false, F::ZERO);
        let mut word = (seed * MAX_UINT32).trunc_u32();
        for d in destination {
            *d = F::from_u32(word) / MAX_UINT32;
            word = lcg(word);
        }
    }

    /// `set_deja_vu`.
    pub fn set_deja_vu(&mut self, deja_vu: F) {
        self.deja_vu = deja_vu;
    }

    /// `set_length`: 1 to 16, else ignored.
    pub fn set_length(&mut self, length: i32) {
        if !(1..=LOOP_SIZE as i32).contains(&length) {
            return;
        }
        self.length = length;
        self.step %= length;
    }

    /// `deja_vu()`.
    #[must_use]
    pub fn deja_vu(&self) -> F {
        self.deja_vu
    }

    /// `length()`.
    #[must_use]
    pub fn length(&self) -> i32 {
        self.length
    }

    /// `Reset()`: the next value is the loop's first.
    pub fn reset(&mut self) {
        self.step = self.length - 1;
    }
}

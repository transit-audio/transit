//! The Texture Synth's frozen cloud saved with the session (#241 slice 3; the Texture insert's
//! #252 pattern).
//!
//! The synth has no recording: a frozen cloud is the grains it was playing when Freeze went on,
//! held at their envelope level, with the oscillator and the reverb as they were. So the saved
//! state is the synth's whole continuation (`Synth::write_checkpoint`, about 68 KiB, the reverb's
//! memory most of it), the edge converter's and the SEED gate's, behind a header of the host rate,
//! the channel count and every parameter value. With Freeze off nothing is saved (an empty state,
//! which clears a stored one).
//!
//! What is shared with the insert is the pattern, not the code: the insert's machinery copies a
//! recording of up to 9 MiB in bounded chunks over many blocks, checks its setup and generation,
//! and writes the recording beside the session as a WAV file; none of that applies here. The
//! control thread loans a preallocated buffer through an SPSC queue; at the next block boundary
//! (or, on an inactive track, the strip's idle service) the audio thread writes the state into it
//! in one go (fixed size, no allocation, no lock, no wait) and returns it through a second queue.
//! Parsing and the restore happen on the instance's creation path, before it reaches the render
//! graph. Restored, the next block continues bit for bit at the same host rate (at another rate the
//! synth continues and the converter starts empty). The shaper is not saved: it restarts at rest,
//! so a cloud saved with the shaper on stays silent until a note opens its amp envelope.

use super::*;
use crate::plugins::texture::freeze_state::{MAX_HOST_RATE, MIN_HOST_RATE};
use crate::util::MAX_CHANNELS;
use crate::{PluginStateReader, StateBusy};
use rtrb::{Consumer, Producer, RingBuffer};
use std::time::{Duration, Instant};
use transit_texture::checkpoint::State;

const MAGIC: &[u8; 8] = b"TXSYN001";
/// The magic, the host rate, the channel count and every parameter value.
const HEADER: usize = 8 + 4 + 4 + COUNT * 4;
/// The SEED gate's flags and level, the key, the velocity, its gain and the level's smoother.
const SCALARS: usize = 1 + 1 + 1 + 4 + 4 + 4;

/// A state's largest size: the header, the synth's checkpoint, the largest edge converter over the
/// supported host rates and the scalars.
pub(super) fn capacity() -> usize {
    let converter =
        [MIN_HOST_RATE, 48_000.0, MAX_HOST_RATE].iter().map(|r| crate::plugins::resample::state_bound(ENGINE_RATE, *r as u32)).max().unwrap_or(0);
    HEADER + Synth::CHECKPOINT_BYTES + converter + SCALARS
}

#[derive(Debug)]
struct Loan {
    state: Vec<u8>,
    /// Frozen at a host rate a state cannot carry: the stored state is kept.
    unavailable: bool,
}

pub(super) struct Capture {
    requests: Consumer<Loan>,
    replies: Producer<Loan>,
    /// A reply the queue could not take yet (a reader that timed out and has not asked again).
    pending: Option<Loan>,
}

pub(super) struct Reader {
    requests: Producer<Loan>,
    replies: Consumer<Loan>,
    buffer: Option<Loan>,
}

pub(super) fn channel() -> (Capture, Reader) {
    let (request, requests) = RingBuffer::new(1);
    let (replies, reply) = RingBuffer::new(1);
    let loan = Loan { state: Vec::with_capacity(capacity()), unavailable: false };
    (Capture { requests, replies, pending: None }, Reader { requests: request, replies: reply, buffer: Some(loan) })
}

impl PluginStateReader for Reader {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.save_state_within(Duration::from_secs(5)).ok().flatten()
    }

    fn save_state_within(&mut self, busy: Duration) -> Result<Option<Vec<u8>>, StateBusy> {
        // A reply to an earlier request that timed out is stale: recycle its memory, then ask again.
        if self.buffer.is_none() {
            self.buffer = self.replies.pop().ok();
        }
        let Some(mut buffer) = self.buffer.take() else { return Err(StateBusy) };
        buffer.state.clear();
        buffer.unavailable = false;
        if let Err(rtrb::PushError::Full(buffer)) = self.requests.push(buffer) {
            self.buffer = Some(buffer);
            return Err(StateBusy);
        }
        let start = Instant::now();
        loop {
            if let Ok(buffer) = self.replies.pop() {
                let data = (!buffer.unavailable).then(|| buffer.state.clone());
                self.buffer = Some(buffer);
                return data.map(Some).ok_or(StateBusy);
            }
            if start.elapsed() >= busy.min(Duration::from_secs(5)) {
                return Err(StateBusy);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Whether a host rate is one a state carries.
fn rate_supported(sr: f32) -> bool {
    (MIN_HOST_RATE..=MAX_HOST_RATE).contains(&sr)
}

impl TextureSynth {
    /// Writes the state into `out` (cleared first); nothing when Freeze is off. Allocation-free
    /// when `out` has room ([`capacity`]).
    fn write_state(&self, out: &mut Vec<u8>) {
        out.clear();
        if !self.p.on(FREEZE) {
            return;
        }
        out.extend_from_slice(MAGIC);
        self.host_sr.write(out);
        (self.ch as u32).write(out);
        for i in 0..PARAMS.len() {
            self.p.v(i).write(out);
        }
        self.synth.write_checkpoint(out);
        self.up.write(out);
        self.gate_flags.write(out);
        self.gate_high.write(out);
        self.key.write(out);
        self.velocity.write(out);
        self.vel_gain.write(out);
        self.level.value().write(out);
    }

    /// The state now (Freeze on), or `None`. Off the audio thread only: it allocates.
    pub(super) fn snapshot(&self) -> Option<Vec<u8>> {
        let mut data = Vec::with_capacity(capacity());
        self.write_state(&mut data);
        (!data.is_empty()).then_some(data)
    }

    /// Answers a pending loan at a block boundary: the state written into it, or empty when Freeze
    /// is off. Allocation-free, never waits.
    pub(super) fn answer_capture(&mut self) {
        let Some(mut capture) = self.capture.take() else { return };
        if capture.pending.is_none()
            && let Ok(mut loan) = capture.requests.pop()
        {
            if self.p.on(FREEZE) && !rate_supported(self.host_sr) {
                loan.state.clear();
                loan.unavailable = true;
            } else {
                self.write_state(&mut loan.state);
            }
            capture.pending = Some(loan);
        }
        if let Some(loan) = capture.pending.take()
            && let Err(rtrb::PushError::Full(loan)) = capture.replies.push(loan)
        {
            capture.pending = Some(loan);
        }
        self.capture = Some(capture);
    }

    /// Restores a state on the creation path (before `prepare`): an empty one is nothing frozen
    /// and changes nothing; a refused one leaves the instance as it was.
    pub(super) fn restore(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        let Some(mut new) = Self::decode(data) else { return false };
        new.capture = self.capture.take();
        *self = new;
        true
    }

    fn decode(data: &[u8]) -> Option<Self> {
        if data.len() > capacity() || data.get(..8)? != MAGIC {
            return None;
        }
        let mut input = data.get(8..)?;
        let mut sr = 0.0f32;
        let mut ch = 0u32;
        sr.read(&mut input)?;
        ch.read(&mut input)?;
        if !sr.is_finite() || !rate_supported(sr) || !(1..=MAX_CHANNELS as u32).contains(&ch) {
            return None;
        }
        let mut t = Self::new();
        for p in PARAMS.iter() {
            let mut v = 0.0f32;
            v.read(&mut input)?;
            if !v.is_finite() || p.clamp(v) != v {
                return None;
            }
            t.set_param(p.id, v);
        }
        if !t.p.on(FREEZE) {
            return None;
        }
        t.prepare(sr, 512, ch as usize);
        t.synth.read_checkpoint(&mut input)?;
        t.up.read(&mut input)?;
        t.gate_flags.read(&mut input)?;
        t.gate_high.read(&mut input)?;
        t.key.read(&mut input)?;
        t.velocity.read(&mut input)?;
        t.vel_gain.read(&mut input)?;
        let mut level = 0.0f32;
        level.read(&mut input)?;
        if t.gate_flags >= 16 || t.key > 127 || !(0.0..=1.0).contains(&t.velocity) || !(0.0..=1.0).contains(&t.vel_gain) || !input.is_empty() {
            return None;
        }
        // The level's smoother where it was (it may still be gliding toward the knob).
        if !level.is_finite() || !(0.0..=4.0).contains(&level) {
            return None;
        }
        t.level = Smoother::new(level);
        t.level.set_time(20.0, ENGINE_RATE as f32);
        t.level.set(db_to_gain(t.p.v(LEVEL)));
        t.restored = true;
        Some(t)
    }
}

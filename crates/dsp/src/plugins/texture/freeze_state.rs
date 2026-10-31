//! Frozen recording and versioned continuation state (#252).
//!
//! The control thread loans a preallocated byte buffer through an SPSC queue. At the next
//! block boundary the device fills it and returns it through a second queue. Encoding, copying
//! and queue operations allocate nothing, take no lock and never wait. Decoding and file I/O
//! happen before a new instance reaches the render graph.

use super::*;
use crate::{PluginStateReader, StateBusy};
use rtrb::{Consumer, Producer, RingBuffer};
use std::time::{Duration, Instant};
use transit_texture::checkpoint::State;

const MAGIC: &[u8; 8] = b"TXFRZ001";
const HEADER: usize = 8 + 7 * 4 + 23 * 4 + 1;
// Maximum codec-rate PCM is clean tape mono: 2,048,000 frames, plus continuation history.
const CAPACITY: usize = 9 * 1024 * 1024;

#[cfg(test)]
#[path = "freeze_state_tests.rs"]
mod tests;

/// Validated metadata and borrowed components of a snapshot.
pub struct Recording<'a> {
    pub quality: u32,
    pub rate: u32,
    pub channels: u32,
    pub buffer_frames: u32,
    pub span: u32,
    pub host_rate: f32,
    pub pcm: &'a [u8],
    pub continuation: &'a [u8],
    pub header: &'a [u8],
}

impl<'a> Recording<'a> {
    /// Join a plain PCM payload and the companion file (header followed by continuation).
    pub fn from_files(pcm: &[u8], companion: &[u8]) -> Option<Vec<u8>> {
        Self::join(companion.get(..HEADER)?, pcm, companion.get(HEADER..)?)
    }
    /// Inspect a bounded packet; reject rate, layout, span or parameter mismatches.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() > CAPACITY || data.get(..8)? != MAGIC {
            return None;
        }
        let mut input = data.get(8..)?;
        let mut values = [0u32; 5];
        values.read(&mut input)?;
        let [quality, rate, channels, buffer_frames, span] = values;
        let media = *Media::ALL.get(quality as usize)?;
        let buffer = BufferSize::new(buffer_frames as usize)?;
        if !(1..=2).contains(&channels) || rate != media.sample_rate() {
            return None;
        }
        let ratio = if media == Media::CleanTape { 2 } else { 1 };
        let expected = buffer.max_delay(media) as u32 * (3 - channels) * ratio;
        if span != expected {
            return None;
        }
        let mut host_rate = 0.0f32;
        let mut host_channels = 0u32;
        host_rate.read(&mut input)?;
        host_channels.read(&mut input)?;
        if !host_rate.is_finite() || !(8_000.0..=384_000.0).contains(&host_rate) || !(1..=MAX_CHANNELS as u32).contains(&host_channels) {
            return None;
        }
        let mut params = [0.0f32; 23];
        params.read(&mut input)?;
        if params.iter().zip(TEXTURE_PARAMS.iter()).any(|(v, p)| !v.is_finite() || *v != p.clamp(*v))
            || params[P_FREEZE] < 0.5
            || params[P_QUALITY].round() as u32 != quality
            || (params[P_MONO_INPUT] >= 0.5) != (channels == 1)
            || (4 + params[P_BUFFER].round() as u32) * 48_000 != buffer_frames
        {
            return None;
        }
        let size = span as usize * channels as usize * 4;
        let pcm = data.get(HEADER..HEADER.checked_add(size)?)?;
        let continuation = data.get(HEADER + size..)?;
        if continuation.is_empty() {
            return None;
        }
        Some(Self { quality, rate, channels, buffer_frames, span, host_rate, pcm, continuation, header: data.get(..HEADER)? })
    }

    /// Rejoin the separately stored recording and continuation files for instance creation.
    pub fn join(header: &[u8], pcm: &[u8], continuation: &[u8]) -> Option<Vec<u8>> {
        let size = header.len().checked_add(pcm.len())?.checked_add(continuation.len())?;
        if size > CAPACITY || header.len() != HEADER {
            return None;
        }
        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(header);
        data.extend_from_slice(pcm);
        data.extend_from_slice(continuation);
        Recording::parse(&data)?;
        Some(data)
    }
}

pub(super) struct Capture {
    requests: Consumer<Vec<u8>>,
    replies: Producer<Vec<u8>>,
    // A disconnected or busy reader must never make the callback free the loan.
    pending: Option<Vec<u8>>,
}

pub(super) struct Reader {
    requests: Producer<Vec<u8>>,
    replies: Consumer<Vec<u8>>,
    buffer: Option<Vec<u8>>,
}

pub(super) fn channel() -> (Capture, Reader) {
    let (request, requests) = RingBuffer::new(1);
    let (replies, reply) = RingBuffer::new(1);
    (Capture { requests, replies, pending: None }, Reader { requests: request, replies: reply, buffer: Some(Vec::with_capacity(CAPACITY)) })
}

impl PluginStateReader for Reader {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.save_state_within(Duration::from_secs(5)).ok().flatten()
    }

    fn save_state_within(&mut self, busy: Duration) -> Result<Option<Vec<u8>>, StateBusy> {
        // A response to a previous timed-out request is stale: recycle its memory, then ask again.
        if self.buffer.is_none() {
            self.buffer = self.replies.pop().ok();
        }
        let Some(mut buffer) = self.buffer.take() else { return Err(StateBusy) };
        buffer.clear();
        if let Err(rtrb::PushError::Full(buffer)) = self.requests.push(buffer) {
            self.buffer = Some(buffer);
            return Err(StateBusy);
        }
        let start = Instant::now();
        loop {
            if let Ok(buffer) = self.replies.pop() {
                let data = buffer.clone();
                self.buffer = Some(buffer);
                return Ok(Some(data));
            }
            if start.elapsed() >= busy.min(Duration::from_secs(5)) {
                return Err(StateBusy);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl TextureFx {
    fn write_snapshot(&self, out: &mut Vec<u8>) {
        out.clear();
        if !self.p.on(P_FREEZE) || self.want != self.setup {
            return;
        }
        out.extend_from_slice(MAGIC);
        let channels = if self.setup.mono { 1u32 } else { 2 };
        [self.setup.media.index() as u32, self.codec_rate(), channels, self.setup.buffer.frames() as u32, self.texture.recording_frames() as u32]
            .write(out);
        self.sr.write(out);
        (self.ch as u32).write(out);
        for i in 0..TEXTURE_PARAMS.len() {
            self.p.v(i).write(out);
        }
        self.gate_flags.write(out);
        self.texture.write_recording(out);
        self.texture.write_checkpoint(out);
        self.stage.write(out);
        self.paths[self.active_path].write(out);
        let (kind, remaining): (u8, u32) = match self.fade {
            Fade::Steady => (0, 0),
            Fade::Out(n) => (1, n),
            Fade::In(n) => (2, n),
        };
        kind.write(out);
        remaining.write(out);
    }

    pub(super) fn snapshot(&self) -> Option<Vec<u8>> {
        if !self.p.on(P_FREEZE) {
            return None;
        }
        let mut data = Vec::with_capacity(CAPACITY);
        self.write_snapshot(&mut data);
        (!data.is_empty()).then_some(data)
    }

    pub(super) fn answer_capture(&mut self) {
        if self.p.on(P_FREEZE) && self.want != self.setup {
            return;
        }
        let Some(mut capture) = self.capture.take() else { return };
        if capture.pending.is_none() {
            capture.pending = capture.requests.pop().ok().map(|mut buffer| {
                self.write_snapshot(&mut buffer);
                buffer
            });
        }
        if let Some(buffer) = capture.pending.take()
            && let Err(rtrb::PushError::Full(buffer)) = capture.replies.push(buffer)
        {
            capture.pending = Some(buffer);
        }
        self.capture = Some(capture);
    }

    pub(super) fn restore_snapshot(&mut self, data: &[u8]) -> bool {
        // All parsing and construction happen on the non-realtime creation path. Refusal leaves
        // the old device intact. The first prepare/reset retains the restored chain.
        let Some(new) = Self::decode_snapshot(data) else { return false };
        *self = new;
        true
    }

    fn decode_snapshot(data: &[u8]) -> Option<Self> {
        let recording = Recording::parse(data)?;
        let mut t = Self::new();
        let mut input = &recording.header[28..];
        let mut sr = 0.0f32;
        let mut ch = 0u32;
        sr.read(&mut input)?;
        ch.read(&mut input)?;
        for p in TEXTURE_PARAMS.iter() {
            let mut v = 0.0f32;
            v.read(&mut input)?;
            t.set_param(p.id, v);
        }
        t.prepare(sr, 512, ch as usize);
        t.gate_flags.read(&mut input)?;
        if t.gate_flags >= 16 {
            return None;
        }
        let mut state = recording.continuation;
        t.texture.read_checkpoint(&mut state)?;
        t.stage.read(&mut state)?;
        t.paths[t.active_path].read(&mut state)?;
        let mut kind = 0u8;
        let mut remaining = 0u32;
        kind.read(&mut state)?;
        remaining.read(&mut state)?;
        if remaining > t.fade_len {
            return None;
        }
        t.fade = match kind {
            0 if remaining == 0 => Fade::Steady,
            1 => Fade::Out(remaining),
            2 => Fade::In(remaining),
            _ => return None,
        };
        if !state.is_empty() {
            return None;
        }
        let mut pcm = recording.pcm;
        t.texture.read_recording(&mut pcm)?;
        if !pcm.is_empty() {
            return None;
        }
        t.idle = false;
        t.restored = true;
        Some(t)
    }
}

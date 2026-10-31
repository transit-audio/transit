//! Frozen recording and versioned continuation state (#252).
//!
//! The control thread loans a preallocated byte buffer through an SPSC queue. At the next
//! block boundaries the device copies bounded chunks of raw memory and returns the loan through
//! a second queue. Copying and queue operations allocate nothing, take no lock and never wait.
//! Recording decoding, PCM encoding and file I/O
//! happen before a new instance reaches the render graph.

use super::*;
use crate::{PluginStateReader, StateBusy};
use rtrb::{Consumer, Producer, RingBuffer};
use std::time::{Duration, Instant};
use transit_texture::checkpoint::State;

const MAGIC: &[u8; 8] = b"TXFRZ002";
const LEGACY_MAGIC: &[u8; 8] = b"TXFRZ001";
const WAV_MAGIC: &[u8; 8] = b"TXWAV001";
const HEADER: usize = 8 + 7 * 4 + 23 * 4 + 1;
// Maximum codec-rate PCM is clean tape mono: 2,048,000 frames, plus continuation history.
const CAPACITY: usize = 9 * 1024 * 1024;
// At most 64 KiB of recording copied per callback, independent of quality and span.
const COPY_ITEMS: usize = 32 * 1024;
/// The host rates a frozen capture supports: one limit for `Recording::parse` and for the audio
/// thread's decision to answer at all (a session may run at up to 768 kHz, and a capture at a
/// rate that `parse` would refuse is never written).
pub const MIN_HOST_RATE: f32 = 8_000.0;
pub const MAX_HOST_RATE: f32 = 384_000.0;
/// Header plus the continuation excluding the rate path (the texture checkpoint, input stage and
/// fade): fixed in shape, 73,519 bytes measured; the test below fails if it outgrows this.
const FIXED_STATE: usize = 80 * 1024;

/// The state capacity of a loan: the worst case over every supported host rate and quality, from
/// the converter ring sizes the writer uses, plus a quarter of margin.
fn checkpoint_capacity() -> usize {
    let path = Media::ALL
        .iter()
        .flat_map(|m| [MIN_HOST_RATE, MAX_HOST_RATE].map(|host| Path::state_bound(host as u32, m.sample_rate())))
        .max()
        .unwrap_or(0);
    (FIXED_STATE + path) * 5 / 4
}

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
    /// The device has processed unfrozen input or successfully restored real media.
    pub fn has_recorded(&self) -> bool {
        self.header[HEADER - 1] & 0x80 != 0
    }
    /// Stable recording metadata, independent of the optional checkpoint's version.
    pub fn wav_header(&self) -> Vec<u8> {
        let mut header = self.header.to_vec();
        header[..8].copy_from_slice(WAV_MAGIC);
        header
    }
    /// Join a plain PCM payload and the companion file (header followed by continuation).
    pub fn from_files(pcm: &[u8], companion: &[u8]) -> Option<Vec<u8>> {
        Self::join(companion.get(..HEADER)?, pcm, companion.get(HEADER..)?)
    }
    /// Inspect a bounded packet; reject rate, layout, span or parameter mismatches.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() > CAPACITY || ![MAGIC.as_slice(), LEGACY_MAGIC.as_slice(), WAV_MAGIC.as_slice()].contains(&data.get(..8)?) {
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
        if !host_rate.is_finite() || !(MIN_HOST_RATE..=MAX_HOST_RATE).contains(&host_rate) || !(1..=MAX_CHANNELS as u32).contains(&host_channels) {
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
    requests: Consumer<Loan>,
    replies: Producer<Loan>,
    // A disconnected or busy reader must never make the callback free the loan.
    pending: Option<Loan>,
    copying: Option<Loan>,
}

pub(super) struct Reader {
    requests: Producer<Loan>,
    replies: Consumer<Loan>,
    buffer: Option<Loan>,
}

#[derive(Debug)]
struct Loan {
    raw: Vec<i16>,
    state: Vec<u8>,
    copied: usize,
    setup: Setup,
    canceled: bool,
    unavailable: bool,
    generation: u64,
}

impl Loan {
    fn packet(&self) -> Option<Vec<u8>> {
        if self.unavailable {
            return None;
        }
        if self.state.is_empty() {
            return Some(Vec::new());
        }
        let header = self.state.get(..HEADER)?;
        let mut device = TextureFx::from_header(header)?;
        let mut continuation = self.state.get(HEADER..)?;
        device.texture.read_checkpoint(&mut continuation)?;
        device.texture.restore_raw_recording(self.raw.get(..self.copied)?)?;
        let mut data = Vec::with_capacity(CAPACITY);
        data.extend_from_slice(header);
        device.texture.write_recording(&mut data);
        data.extend_from_slice(self.state.get(HEADER..)?);
        Some(data)
    }
}

pub(super) fn channel() -> (Capture, Reader) {
    let (request, requests) = RingBuffer::new(1);
    let (replies, reply) = RingBuffer::new(1);
    let loan = Loan {
        raw: vec![0; 2 * BufferSize::MAX.max_delay(Media::DirtyTape) as usize + 8],
        state: Vec::with_capacity(checkpoint_capacity()),
        copied: 0,
        setup: Setup::DEFAULT,
        canceled: false,
        unavailable: false,
        generation: 0,
    };
    (Capture { requests, replies, pending: None, copying: None }, Reader { requests: request, replies: reply, buffer: Some(loan) })
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
        buffer.state.clear();
        buffer.copied = 0;
        buffer.canceled = false;
        buffer.unavailable = false;
        if let Err(rtrb::PushError::Full(buffer)) = self.requests.push(buffer) {
            self.buffer = Some(buffer);
            return Err(StateBusy);
        }
        let start = Instant::now();
        loop {
            if let Ok(buffer) = self.replies.pop() {
                if buffer.canceled {
                    log::warn!(
                        "Texture frozen capture canceled: Freeze turned off or the recording reset; previous media retained when still frozen"
                    );
                }
                let data = buffer.packet();
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

impl TextureFx {
    fn write_header(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(MAGIC);
        let channels = if self.setup.mono { 1u32 } else { 2 };
        [self.setup.media.index() as u32, self.codec_rate(), channels, self.setup.buffer.frames() as u32, self.texture.recording_frames() as u32]
            .write(out);
        self.sr.write(out);
        (self.ch as u32).write(out);
        for i in 0..TEXTURE_PARAMS.len() {
            // An idle/bypassed insert may have pending setup edits. Capture the setup of the
            // recording we actually own, without applying those edits or rebuilding it.
            let value = match i {
                P_QUALITY => self.setup.media.index() as f32,
                P_MONO_INPUT => f32::from(self.setup.mono),
                P_BUFFER => self.setup.buffer.frames() as f32 / 48_000.0 - 4.0,
                _ => self.p.v(i),
            };
            value.write(out);
        }
        (self.gate_flags | if self.recorded { 0x80 } else { 0 }).write(out);
    }

    fn write_continuation(&self, out: &mut Vec<u8>) {
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
        // Version 2 preserves a pending setup edit independently of the recording's setup.
        // Restoring it resumes an in-progress restart fade; WAV-only restore starts fresh.
        [self.p.v(P_QUALITY), self.p.v(P_MONO_INPUT), self.p.v(P_BUFFER)].write(out);
    }

    fn write_snapshot(&self, out: &mut Vec<u8>) {
        out.clear();
        if !self.p.on(P_FREEZE) {
            return;
        }
        self.write_header(out);
        self.texture.write_recording(out);
        self.write_continuation(out);
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
        let Some(mut capture) = self.capture.take() else { return };
        if capture.pending.is_none() && capture.copying.is_none() {
            capture.copying = capture.requests.pop().ok().map(|mut loan| {
                loan.setup = self.setup;
                loan.generation = self.recording_generation;
                loan
            });
        }
        if let Some(mut loan) = capture.copying.take() {
            if !self.p.on(P_FREEZE) || loan.setup != self.setup || loan.generation != self.recording_generation {
                loan.canceled = loan.copied != 0;
                loan.unavailable = self.p.on(P_FREEZE);
                capture.pending = Some(loan);
            } else if self.sr > MAX_HOST_RATE {
                // Above the supported host rate `Recording::parse` would refuse the packet, and its
                // state could outgrow the loan: answer unavailable without writing anything.
                loan.canceled = false;
                loan.unavailable = true;
                capture.pending = Some(loan);
            } else {
                let raw = self.texture.raw_recording();
                let end = (loan.copied + COPY_ITEMS).min(raw.len());
                loan.raw[loan.copied..end].copy_from_slice(&raw[loan.copied..end]);
                loan.copied = end;
                if end == raw.len() {
                    // The recording is immutable while frozen. Take the moving continuation at
                    // the final boundary, so its next output is exactly the live device's.
                    self.write_header(&mut loan.state);
                    self.write_continuation(&mut loan.state);
                    capture.pending = Some(loan);
                } else {
                    capture.copying = Some(loan);
                }
            }
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
        let mut t = Self::from_header(recording.header)?;
        if !recording.continuation.is_empty() {
            t.read_continuation(recording.continuation, recording.header.get(..8)? == MAGIC)?;
        }
        let mut pcm = recording.pcm;
        t.texture.read_recording(&mut pcm)?;
        if !pcm.is_empty() {
            return None;
        }
        t.idle = false;
        t.restored = true;
        t.recorded = true;
        Some(t)
    }

    fn from_header(header: &[u8]) -> Option<Self> {
        let mut t = Self::new();
        let mut input = header.get(28..)?;
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
        t.gate_flags &= 0x7f;
        if t.gate_flags >= 16 {
            return None;
        }
        Some(t)
    }

    fn read_continuation(&mut self, mut state: &[u8], pending_setup: bool) -> Option<()> {
        self.texture.read_checkpoint(&mut state)?;
        self.stage.read(&mut state)?;
        self.paths[self.active_path].read(&mut state)?;
        let mut kind = 0u8;
        let mut remaining = 0u32;
        kind.read(&mut state)?;
        remaining.read(&mut state)?;
        if remaining > self.fade_len {
            return None;
        }
        self.fade = match kind {
            0 if remaining == 0 => Fade::Steady,
            1 => Fade::Out(remaining),
            2 => Fade::In(remaining),
            _ => return None,
        };
        if pending_setup {
            let mut values = [0.0f32; 3];
            values.read(&mut state)?;
            for (i, value) in [P_QUALITY, P_MONO_INPUT, P_BUFFER].into_iter().zip(values) {
                let param = &TEXTURE_PARAMS[i];
                if !value.is_finite() || param.clamp(value) != value {
                    return None;
                }
                self.set_param(param.id, value);
            }
        }
        if !state.is_empty() {
            return None;
        }
        Some(())
    }
}

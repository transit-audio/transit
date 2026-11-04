//! Bounded transport framing, independent of diagnostic capture and musical MIDI.

/// Capacity of the dedicated input transport lane.
pub const TRANSPORT_CAPACITY: usize = 2048;

/// A learned source to copy into the independent transport lane. It carries no action.
#[derive(Clone, Copy, Debug)]
pub struct TransportSource {
    /// None matches all channels; otherwise zero-based 0..15.
    pub channel: Option<u8>,
    /// True for note on/off; false for CC.
    pub note: bool,
    /// Note or controller number (0..127).
    pub number: u8,
}

pub(crate) struct SourceFilter([std::sync::atomic::AtomicU64; 64]);

impl Default for SourceFilter {
    fn default() -> Self {
        Self(std::array::from_fn(|_| std::sync::atomic::AtomicU64::new(0)))
    }
}

impl SourceFilter {
    pub(crate) fn set(&self, sources: &[TransportSource]) {
        use std::sync::atomic::Ordering;
        for word in &self.0 {
            word.store(0, Ordering::Release);
        }
        for source in sources.iter().filter(|s| s.number < 128 && s.channel.is_none_or(|c| c < 16)) {
            for channel in 0..16 {
                if source.channel.is_none_or(|c| c == channel) {
                    let slot = usize::from(channel) * 4 + usize::from(!source.note) * 2 + usize::from(source.number / 64);
                    self.0[slot].fetch_or(1 << (source.number % 64), Ordering::Release);
                }
            }
        }
    }

    pub(crate) fn matches(&self, bytes: [u8; 3]) -> bool {
        let cc = match bytes[0] & 0xf0 {
            0x80 | 0x90 => false,
            0xb0 => true,
            _ => return false,
        };
        if bytes[1] >= 128 {
            return false;
        }
        let slot = usize::from(bytes[0] & 15) * 4 + usize::from(cc) * 2 + usize::from(bytes[1] / 64);
        self.0[slot].load(std::sync::atomic::Ordering::Acquire) & (1 << (bytes[1] % 64)) != 0
    }
}

/// A complete envelope or system message. Decoding belongs on control, never on audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportEnvelope {
    /// Complete bytes, including F0/F7 for SysEx. No diagnostic prefixes enter this lane.
    pub bytes: [u8; 13],
    /// Number of initialized bytes.
    pub len: u8,
    /// Arrival stamp from the shared MIDI host origin.
    pub host_arrival_ns: u64,
    /// Ordering sequence shared with the musical input ring, scoped to this port opening.
    pub seq: u64,
}

impl TransportEnvelope {
    /// The complete wire message, with interleaved real-time bytes removed.
    #[must_use]
    pub fn message(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len).min(self.bytes.len())]
    }
}

/// Fixed storage for the largest admitted MMC command. Overlong and interrupted messages
/// cannot produce an envelope. It only frames bytes; command/id/timecode validation is control's.
#[derive(Default)]
pub(crate) struct Framer {
    bytes: [u8; 13],
    len: usize,
    host: u64,
}

impl Framer {
    pub(crate) fn start(&mut self, host: u64) {
        self.bytes = [0; 13];
        self.bytes[0] = 0xf0;
        self.len = 1;
        self.host = host;
    }

    pub(crate) fn byte(&mut self, byte: u8) -> Option<TransportEnvelope> {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
        }
        self.len = self.len.saturating_add(1);
        (byte == 0xf7 && self.len <= self.bytes.len()).then_some(TransportEnvelope {
            bytes: self.bytes,
            len: self.len.min(13) as u8,
            host_arrival_ns: self.host,
            seq: 0,
        })
    }
}

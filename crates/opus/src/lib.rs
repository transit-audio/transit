//! transit's Opus codec (#238, ADR 0048): the reference libopus 1.6.1, built from the vendored
//! source in `vendor/` by `build.rs` with `cc`, behind a safe [`Encoder`] and [`Decoder`].
//!
//! Raw Opus packets only: the Ogg container (RFC 7845) and everything a bounce or an import needs
//! around the codec live in `transit-audio-io`. Both directions run at 48 kHz, the codec's internal
//! rate, with one or two interleaved channels. The `unsafe` FFI is confined to the private `ffi`
//! module (ADR 0013 discipline, ADR 0048 allowlist entry); everything else denies it (`deny`, not
//! `forbid`, at the crate root only so that the one `allow` on `ffi` can take effect).

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[allow(unsafe_code)]
mod ffi;

/// The sample rate both directions run at: Opus's internal rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// The longest packet the decoder takes: 120 ms at 48 kHz, in samples per channel (opus.h).
pub const MAX_PACKET_SAMPLES: usize = 5760;

/// The most bytes one Opus packet holds (RFC 6716 section 3.4: 1275 per frame; the recommended
/// output buffer is 4000 bytes).
pub const MAX_PACKET_BYTES: usize = 4000;

/// A libopus error: its code and message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("libopus: {message} ({code})")]
pub struct Error {
    /// The libopus error code (negative).
    pub code: i32,
    /// libopus's message for it (`opus_strerror`).
    pub message: String,
}

impl Error {
    fn from_code(code: i32) -> Self {
        Error { code, message: ffi::strerror(code) }
    }
}

/// The libopus version string, `libopus 1.6.1`.
pub fn version() -> String {
    ffi::version_string()
}

/// Encoder settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    /// 1 or 2.
    pub channels: usize,
    /// The target bitrate in bits per second (libopus accepts 500 to 512000).
    pub bitrate_bps: i32,
    /// Variable bitrate, constrained (libopus's default VBR mode: the rate follows the material
    /// but stays near the target over a short window, so a file's size follows the bitrate asked
    /// for); `false` is constant bitrate.
    pub vbr: bool,
    /// 0 to 10; 10 is the slowest and best.
    pub complexity: i32,
}

/// An Opus encoder at 48 kHz, application `OPUS_APPLICATION_AUDIO`, signal `OPUS_SIGNAL_MUSIC`.
pub struct Encoder {
    raw: ffi::RawEncoder,
    channels: usize,
    lookahead: u32,
}

impl Encoder {
    /// A new encoder with `settings`.
    pub fn new(settings: EncoderSettings) -> Result<Self, Error> {
        if !(1..=2).contains(&settings.channels) {
            return Err(Error { code: -1, message: format!("{} channels: libopus encodes 1 or 2 per stream", settings.channels) });
        }
        let mut raw = ffi::RawEncoder::new(SAMPLE_RATE as i32, settings.channels).map_err(Error::from_code)?;
        raw.set(ffi::OPUS_SET_BITRATE_REQUEST, settings.bitrate_bps).map_err(Error::from_code)?;
        raw.set(ffi::OPUS_SET_VBR_REQUEST, i32::from(settings.vbr)).map_err(Error::from_code)?;
        raw.set(ffi::OPUS_SET_VBR_CONSTRAINT_REQUEST, 1).map_err(Error::from_code)?;
        raw.set(ffi::OPUS_SET_COMPLEXITY_REQUEST, settings.complexity).map_err(Error::from_code)?;
        raw.set(ffi::OPUS_SET_SIGNAL_REQUEST, ffi::OPUS_SIGNAL_MUSIC).map_err(Error::from_code)?;
        let lookahead = raw.lookahead().map_err(Error::from_code)?;
        let lookahead = u32::try_from(lookahead).map_err(|_| Error { code: -3, message: format!("negative lookahead {lookahead}") })?;
        Ok(Encoder { raw, channels: settings.channels, lookahead })
    }

    /// The channel count.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The encoder's delay in samples at 48 kHz (`OPUS_GET_LOOKAHEAD`): the Ogg Opus pre-skip.
    pub fn lookahead(&self) -> u32 {
        self.lookahead
    }

    /// Encodes one frame: `pcm` holds `frame_size` samples per channel, interleaved, where
    /// `frame_size` is an Opus frame size at 48 kHz (120, 240, 480, 960, 1920 or 2880). Writes the
    /// packet into `out` (at most `out.len()` bytes) and returns its length.
    pub fn encode(&mut self, pcm: &[f32], frame_size: usize, out: &mut [u8]) -> Result<usize, Error> {
        self.raw.encode(pcm, frame_size, out).map_err(Error::from_code)
    }
}

/// An Opus decoder at 48 kHz.
pub struct Decoder {
    raw: ffi::RawDecoder,
    channels: usize,
    pcm: Vec<f32>,
}

impl Decoder {
    /// A new decoder for `channels` (1 or 2) interleaved output channels.
    pub fn new(channels: usize) -> Result<Self, Error> {
        if !(1..=2).contains(&channels) {
            return Err(Error { code: -1, message: format!("{channels} channels: libopus decodes 1 or 2 per stream") });
        }
        let raw = ffi::RawDecoder::new(SAMPLE_RATE as i32, channels).map_err(Error::from_code)?;
        Ok(Decoder { raw, channels, pcm: vec![0.0; MAX_PACKET_SAMPLES * channels] })
    }

    /// The channel count.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decodes one packet; returns its samples, interleaved (valid until the next call).
    pub fn decode(&mut self, packet: &[u8]) -> Result<&[f32], Error> {
        let n = self.raw.decode(packet, &mut self.pcm, MAX_PACKET_SAMPLES).map_err(Error::from_code)?;
        Ok(self.pcm.get(..n * self.channels).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(channels: usize) -> EncoderSettings {
        EncoderSettings { channels, bitrate_bps: 160_000, vbr: true, complexity: 10 }
    }

    #[test]
    fn the_version_is_the_vendored_release() {
        assert_eq!(version(), "libopus 1.6.1");
    }

    #[test]
    fn a_frame_round_trips_through_the_codec() {
        for channels in [1, 2] {
            let mut enc = Encoder::new(settings(channels)).unwrap();
            assert!(enc.lookahead() > 0 && enc.lookahead() < 960, "lookahead {}", enc.lookahead());
            let mut dec = Decoder::new(channels).unwrap();
            let mut out = vec![0u8; MAX_PACKET_BYTES];
            let mut decoded = 0;
            for k in 0..10 {
                let pcm: Vec<f32> = (0..960 * channels).map(|i| ((k * 960 + i / channels) as f32 * 0.05).sin() * 0.5).collect();
                let n = enc.encode(&pcm, 960, &mut out).unwrap();
                assert!(n > 0 && n <= 1275 * 3);
                decoded += dec.decode(&out[..n]).unwrap().len();
            }
            assert_eq!(decoded, 10 * 960 * channels);
        }
    }

    #[test]
    fn bad_arguments_are_errors_not_calls() {
        assert!(Encoder::new(settings(3)).is_err());
        assert!(Encoder::new(settings(0)).is_err());
        assert!(Decoder::new(6).is_err());
        let mut enc = Encoder::new(settings(2)).unwrap();
        let mut out = vec![0u8; MAX_PACKET_BYTES];
        // Wrong length for the frame size, an illegal frame size, an empty output buffer.
        assert!(enc.encode(&[0.0; 960], 960, &mut out).is_err());
        let e = enc.encode(&[0.0; 2 * 1000], 1000, &mut out).unwrap_err();
        assert_eq!(e.code, -1, "{e}");
        assert!(enc.encode(&[0.0; 2 * 960], 960, &mut []).is_err());
        let mut dec = Decoder::new(2).unwrap();
        assert!(dec.decode(&[]).is_err());
        // A packet whose TOC claims more frames than it holds is refused by libopus.
        assert!(dec.decode(&[0xFF, 0xFF]).is_err());
    }

    #[test]
    fn an_out_of_range_bitrate_is_refused_by_libopus() {
        let e = Encoder::new(EncoderSettings { bitrate_bps: -5, ..settings(2) });
        assert!(e.is_err());
    }

    #[test]
    fn errors_carry_libopus_messages() {
        assert_eq!(Error::from_code(-1).message, "invalid argument");
        assert_eq!(Error::from_code(-99).message, "unknown error");
    }
}

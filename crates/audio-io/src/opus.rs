//! Ogg Opus (RFC 7845): the writer behind [`crate::FileFormat::Opus`] and the reader the import
//! path uses. The codec is libopus 1.6.1 from transit-opus (ADR 0048).
//!
//! The writer encodes at 48 kHz, the codec's internal rate: other session rates go through the
//! crate's streaming sinc converter ([`crate::resample`]). Mono and stereo use channel mapping
//! family 0; more channels are refused (surround Opus needs family 1 and a multistream encoder,
//! a known issue). 20 ms frames, constrained VBR at the bitrate asked for (libopus's default VBR
//! mode: the file size follows the bitrate), complexity 10, application AUDIO, signal MUSIC.
//! The OpusHead's pre-skip is the encoder's lookahead, the stream is padded with silence to the
//! next frame, and the last page's granule position trims it, so the decoded file is exactly the
//! bounce: `round(frames * 48000 / rate)` samples (RFC 7845 sections 4.2 and 4.5).

use crate::ogg::{PageWriter, packets};
use crate::resample::Resampler;
use crate::{AudioBuffer, AudioError, AudioInfo, FileFormat, Result, SampleFormat};
use std::io::{Seek, Write};

/// The bitrates the Bounce dialog offers for stereo, in kb/s (ADR 0048).
pub const OPUS_BITRATES_KBPS: [u16; 5] = [96, 128, 160, 192, 256];
/// The bitrates the Bounce dialog offers for mono, in kb/s: a mono stream needs about half the
/// rate of a stereo one (ADR 0048, pending the maintainer's confirmation).
pub const OPUS_MONO_BITRATES_KBPS: [u16; 3] = [96, 128, 160];
/// The stereo bitrate when none is asked for.
pub const DEFAULT_OPUS_KBPS: u16 = 160;
/// The mono bitrate when none is asked for.
pub const DEFAULT_OPUS_MONO_KBPS: u16 = 96;

/// The bitrates the dialog offers for a bounce of `channels` channels.
pub fn opus_bitrates_kbps(channels: usize) -> &'static [u16] {
    if channels == 1 { &OPUS_MONO_BITRATES_KBPS } else { &OPUS_BITRATES_KBPS }
}

/// The bitrate a bounce of `channels` channels gets when none is asked for: 96 kb/s mono,
/// 160 kb/s stereo.
pub fn default_opus_kbps(channels: usize) -> u16 {
    if channels == 1 { DEFAULT_OPUS_MONO_KBPS } else { DEFAULT_OPUS_KBPS }
}

/// 20 ms at 48 kHz.
const FRAME: usize = 960;
/// The stream serial number. One stream per file; a fixed serial keeps bounces reproducible.
const SERIAL: u32 = 0x7472_6E73;
/// A page is written once it holds this many bytes or a second of audio (libopusenc's limits).
const PAGE_BYTES: usize = 4096;
const PAGE_PACKETS: usize = 50;

fn codec_err(e: transit_opus::Error) -> AudioError {
    AudioError::Encode(e.to_string())
}

/// The OpusHead packet (RFC 7845 section 5.1), channel mapping family 0.
fn head_packet(channels: u8, pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1);
    h.push(channels);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(0);
    h
}

/// The OpusTags packet (RFC 7845 section 5.2): the vendor string and the comments.
fn tags_packet(vendor: &str, comments: &[String]) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(b"OpusTags");
    t.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    t.extend_from_slice(vendor.as_bytes());
    t.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in comments {
        t.extend_from_slice(&(c.len() as u32).to_le_bytes());
        t.extend_from_slice(c.as_bytes());
    }
    t
}

/// The bitrate in bits per second for `kbps` (`None`: [`default_opus_kbps`] for `channels`). Opus
/// takes 6 to 510 kb/s for a stereo stream; anything outside that is refused rather than clamped.
pub(crate) fn bitrate_bps(kbps: Option<u16>, channels: usize) -> Result<i32> {
    let k = kbps.unwrap_or(default_opus_kbps(channels));
    if !(6..=510).contains(&k) {
        return Err(AudioError::Unsupported(format!("an Opus bitrate of {k} kb/s (use 6 to 510; the dialog offers 96 to 256)")));
    }
    Ok(i32::from(k) * 1000)
}

/// Writes Ogg Opus block by block (see the module documentation).
pub(crate) struct Writer<W: Write + Seek> {
    pages: PageWriter<W>,
    encoder: transit_opus::Encoder,
    resampler: Option<Resampler>,
    channels: usize,
    pre_skip: u64,
    /// Input frames received, at the input rate.
    frames_in: u64,
    /// 48 kHz samples per channel waiting for a whole frame, interleaved.
    pending: Vec<f32>,
    /// 48 kHz samples per channel encoded so far (the granule position).
    encoded: u64,
    packet: Vec<u8>,
    packets_on_page: usize,
    bytes_on_page: usize,
}

impl<W: Write + Seek> Writer<W> {
    pub(crate) fn new(out: W, channels: usize, sample_rate: u32, kbps: Option<u16>, title: Option<&str>) -> Result<Self> {
        if channels == 0 || sample_rate == 0 {
            return Err(AudioError::Encode("an Opus file needs channels and a sample rate".into()));
        }
        if channels > 2 {
            return Err(AudioError::Unsupported(format!(
                "Opus export of {channels} channels: transit writes mono and stereo Opus only; fold the bounce down to stereo, or bounce WAV or FLAC"
            )));
        }
        let encoder = transit_opus::Encoder::new(transit_opus::EncoderSettings {
            channels,
            bitrate_bps: bitrate_bps(kbps, channels)?,
            vbr: true,
            complexity: 10,
        })
        .map_err(codec_err)?;
        let pre_skip = encoder.lookahead();
        let pre_skip16 = u16::try_from(pre_skip).map_err(|_| AudioError::Encode(format!("a pre-skip of {pre_skip} samples")))?;
        let mut pages = PageWriter::new(out, SERIAL);
        // The OpusHead page stands alone; the OpusTags end on their own page (RFC 7845 3).
        pages.packet(&head_packet(channels as u8, pre_skip16, sample_rate), 0)?;
        pages.flush(false)?;
        let mut comments = vec![format!("ENCODER=transit {}", env!("CARGO_PKG_VERSION"))];
        if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
            comments.push(format!("TITLE={t}"));
        }
        pages.packet(&tags_packet(&transit_opus::version(), &comments), 0)?;
        pages.flush(false)?;
        let resampler = (sample_rate != transit_opus::SAMPLE_RATE).then(|| Resampler::new(sample_rate, transit_opus::SAMPLE_RATE, channels));
        Ok(Writer {
            pages,
            encoder,
            resampler,
            channels,
            pre_skip: u64::from(pre_skip),
            frames_in: 0,
            pending: Vec::new(),
            encoded: 0,
            packet: vec![0; transit_opus::MAX_PACKET_BYTES],
            packets_on_page: 0,
            bytes_on_page: 0,
        })
    }

    /// Ogg granule positions are 64-bit: no bounce runs out of room. Returns the input frames
    /// the file would then hold.
    pub(crate) fn check_room(&self, frames: u64) -> Result<u64> {
        Ok(self.frames_in.saturating_add(frames))
    }

    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        self.frames_in += frames as u64;
        match &mut self.resampler {
            Some(r) => r.push(block, frames, &mut self.pending),
            None => {
                for i in 0..frames {
                    for c in 0..self.channels {
                        self.pending.push(block.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0));
                    }
                }
            }
        }
        self.encode_whole_frames()
    }

    /// Encodes every whole frame waiting.
    fn encode_whole_frames(&mut self) -> Result<()> {
        let n = FRAME * self.channels;
        let whole = self.pending.len() / n;
        for k in 0..whole {
            let frame: Vec<f32> = self.pending[k * n..(k + 1) * n].iter().map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }).collect();
            self.encode_frame(&frame, None)?;
        }
        self.pending.drain(..whole * n);
        Ok(())
    }

    /// Encodes one frame; `end` is the granule position to give it when it is the last.
    fn encode_frame(&mut self, frame: &[f32], end: Option<u64>) -> Result<()> {
        let len = self.encoder.encode(frame, FRAME, &mut self.packet).map_err(codec_err)?;
        self.encoded += FRAME as u64;
        let granule = end.unwrap_or(self.encoded);
        if self.pages.queued_segments() + len / 255 + 1 > 255 {
            self.flush_page()?;
        }
        self.pages.packet(&self.packet[..len], granule)?;
        self.packets_on_page += 1;
        self.bytes_on_page += len;
        if end.is_none() && (self.bytes_on_page >= PAGE_BYTES || self.packets_on_page >= PAGE_PACKETS) {
            self.flush_page()?;
        }
        Ok(())
    }

    fn flush_page(&mut self) -> Result<()> {
        self.pages.flush(false)?;
        self.packets_on_page = 0;
        self.bytes_on_page = 0;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<W> {
        if let Some(r) = &mut self.resampler {
            r.finish(&mut self.pending);
        }
        let length = match &self.resampler {
            Some(r) => r.output_len(self.frames_in),
            None => self.frames_in,
        };
        self.encode_whole_frames()?;
        // Pad with silence until the encoder has seen pre-skip + length samples; the last packet's
        // granule position is exactly that, which trims the padding (RFC 7845 4.5).
        let end = self.pre_skip + length;
        let n = FRAME * self.channels;
        loop {
            let mut frame = std::mem::take(&mut self.pending);
            frame.resize(n, 0.0);
            let last = self.encoded + FRAME as u64 >= end;
            self.encode_frame(&frame, last.then_some(end))?;
            if last {
                break;
            }
        }
        self.pages.flush(true)?;
        Ok(self.pages.into_inner())
    }
}

/// Encodes a whole buffer.
pub(crate) fn encode(buf: &AudioBuffer, kbps: Option<u16>, title: Option<&str>) -> Result<Vec<u8>> {
    if buf.channels.is_empty() || buf.sample_rate == 0 {
        return Err(AudioError::Encode("nothing to encode".into()));
    }
    let mut w = Writer::new(std::io::Cursor::new(Vec::new()), buf.channels.len(), buf.sample_rate, kbps, title)?;
    w.write(&buf.channels, buf.frames())?;
    Ok(w.finish()?.into_inner())
}

/// `OggS` with an `OpusHead` as its first packet.
pub(crate) fn is_opus(b: &[u8]) -> bool {
    // The first page: 27 header bytes, one lacing value for the 19-byte OpusHead.
    b.get(..4) == Some(b"OggS") && b.get(26).is_some_and(|&n| n >= 1) && {
        let n = usize::from(b[26]);
        b.get(27 + n..27 + n + 8) == Some(b"OpusHead")
    }
}

/// What an OpusHead says.
struct Head {
    channels: usize,
    pre_skip: u64,
    gain_db: f32,
}

fn parse_head(p: &[u8]) -> Result<Head> {
    let bad = |what: &str| AudioError::Malformed(format!("OpusHead: {what}"));
    if p.len() < 19 || p.get(..8) != Some(b"OpusHead") {
        return Err(bad("missing"));
    }
    if p[8] >> 4 != 0 {
        return Err(AudioError::Unsupported(format!("Ogg Opus version {}", p[8])));
    }
    let channels = usize::from(p[9]);
    let family = p[18];
    if channels == 0 {
        return Err(bad("no channels"));
    }
    if family != 0 || channels > 2 {
        return Err(AudioError::Unsupported(format!(
            "Opus channel mapping family {family} with {channels} channels (transit reads mono and stereo Opus)"
        )));
    }
    let pre_skip = u64::from(u16::from_le_bytes([p[10], p[11]]));
    // Bytes 12..16 are the input sample rate: informational only (the decoder runs at 48 kHz).
    let gain_db = f32::from(i16::from_le_bytes([p[16], p[17]])) / 256.0;
    Ok(Head { channels, pre_skip, gain_db })
}

/// The OpusHead and the samples per channel the file holds after pre-skip and end trimming.
fn layout(all: &[crate::ogg::Packet]) -> Result<(Head, u64)> {
    let head = parse_head(&all.first().ok_or_else(|| AudioError::Malformed("an empty Ogg stream".into()))?.data)?;
    if all.get(1).is_none_or(|p| p.data.get(..8) != Some(b"OpusTags")) {
        return Err(AudioError::Malformed("OpusTags missing".into()));
    }
    let last = all.iter().rev().find_map(|p| p.granule).unwrap_or(0);
    let frames = u64::try_from(last).unwrap_or(0).saturating_sub(head.pre_skip);
    Ok((head, frames))
}

fn info(head: &Head, frames: u64) -> AudioInfo {
    AudioInfo {
        format: FileFormat::Opus,
        sample_format: SampleFormat::Compressed,
        sample_rate: transit_opus::SAMPLE_RATE,
        channels: head.channels as u16,
        frames,
        bwf: None,
    }
}

/// Reads an Ogg Opus file's header and length without decoding it.
pub(crate) fn probe(b: &[u8]) -> Result<AudioInfo> {
    let all = packets(b)?;
    let (head, frames) = layout(&all)?;
    Ok(info(&head, frames))
}

/// Decodes a whole Ogg Opus file to planar `f32` at 48 kHz: the pre-skip dropped, the end
/// trimmed to the last granule position, the header's output gain applied.
pub(crate) fn decode(b: &[u8]) -> Result<(AudioInfo, AudioBuffer)> {
    let all = packets(b)?;
    let (head, frames) = layout(&all)?;
    let total = frames.saturating_mul(head.channels as u64);
    if total > crate::MAX_DECODE_SAMPLES {
        return Err(AudioError::TooLarge(format!("{total} samples exceeds the {}-sample decode limit", crate::MAX_DECODE_SAMPLES)));
    }
    let mut dec = transit_opus::Decoder::new(head.channels).map_err(|e| AudioError::Malformed(e.to_string()))?;
    let keep = usize::try_from(frames).map_err(|_| AudioError::TooLarge("Opus length".into()))?;
    let mut channels: Vec<Vec<f32>> = (0..head.channels).map(|_| Vec::with_capacity(keep)).collect();
    let mut skip = head.pre_skip;
    let gain = if head.gain_db == 0.0 { 1.0 } else { 10f32.powf(head.gain_db / 20.0) };
    'packets: for p in all.iter().skip(2) {
        if p.data.is_empty() {
            continue;
        }
        let pcm = dec.decode(&p.data).map_err(|e| AudioError::Malformed(e.to_string()))?;
        for frame in pcm.chunks_exact(head.channels) {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if channels[0].len() >= keep {
                break 'packets;
            }
            for (ch, &v) in channels.iter_mut().zip(frame) {
                ch.push(v * gain);
            }
        }
    }
    let buf = AudioBuffer { sample_rate: transit_opus::SAMPLE_RATE, channels };
    let frames = buf.frames() as u64;
    Ok((info(&head, frames), buf))
}

/// The comments of an Ogg Opus file's OpusTags (`TITLE=...`), and its vendor string.
pub fn opus_tags(b: &[u8]) -> Result<(String, Vec<String>)> {
    let all = packets(b)?;
    let tags = &all.get(1).ok_or_else(|| AudioError::Malformed("OpusTags missing".into()))?.data;
    let bad = || AudioError::Malformed("OpusTags truncated".into());
    let mut at = 8usize;
    let read_len = |at: &mut usize| -> Result<usize> {
        let v = tags.get(*at..*at + 4).ok_or_else(bad)?;
        *at += 4;
        Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]) as usize)
    };
    let n = read_len(&mut at)?;
    let vendor = String::from_utf8_lossy(tags.get(at..at + n).ok_or_else(bad)?).into_owned();
    at += n;
    let count = read_len(&mut at)?;
    let mut comments = Vec::new();
    for _ in 0..count.min(4096) {
        let n = read_len(&mut at)?;
        comments.push(String::from_utf8_lossy(tags.get(at..at + n).ok_or_else(bad)?).into_owned());
        at += n;
    }
    Ok((vendor, comments))
}

/// The OpusHead's pre-skip and the last page's granule position.
pub fn opus_granules(b: &[u8]) -> Result<(u16, i64)> {
    let all = packets(b)?;
    let (head, _) = layout(&all)?;
    let last = all.iter().rev().find_map(|p| p.granule).unwrap_or(0);
    Ok((head.pre_skip as u16, last))
}

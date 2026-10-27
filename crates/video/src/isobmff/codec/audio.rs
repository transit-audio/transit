use super::{AudioParams, CodecConfig};
use crate::bitstream::BitReader;
use crate::isobmff::bytes::{BoxBuf, Cur};
use crate::isobmff::error::{Error, Result};

/// AAC (MPEG-4 audio) configuration from `esds`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AacConfig {
    /// AudioSpecificConfig bytes (DecoderSpecificInfo).
    pub asc: Vec<u8>,
    /// Audio object type (2 = AAC-LC, 5 = SBR/HE-AAC, 29 = PS …) as signalled first in the ASC.
    pub object_type: u8,
    /// Sampling rate signalled by the ASC (core rate; 0 if the ASC is missing).
    pub sample_rate: u32,
    /// Channel configuration from the ASC (0 = defined elsewhere).
    pub channel_config: u8,
    /// DecoderConfigDescriptor fields.
    pub max_bitrate: u32,
    pub avg_bitrate: u32,
    pub buffer_size: u32,
}

const AAC_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

impl AacConfig {
    /// Build from an AudioSpecificConfig, decoding its header fields.
    pub fn from_asc(asc: Vec<u8>) -> Self {
        let (object_type, sample_rate, channel_config) = parse_asc(&asc).unwrap_or((0, 0, 0));
        AacConfig { asc, object_type, sample_rate, channel_config, max_bitrate: 0, avg_bitrate: 0, buffer_size: 0 }
    }
    /// Channel count implied by the channel configuration (0 if unspecified).
    pub fn channels(&self) -> u32 {
        match self.channel_config {
            1..=6 => self.channel_config as u32,
            7 => 8,
            _ => 0,
        }
    }
}

/// Decode (audio object type, sampling frequency, channel configuration) from an AudioSpecificConfig.
pub fn parse_asc(asc: &[u8]) -> Option<(u8, u32, u8)> {
    let mut r = BitReader::new(asc);
    let aot = |r: &mut BitReader| -> Option<u8> {
        let t = r.read_bits(5).ok()? as u8;
        if t == 31 { Some(32 + r.read_bits(6).ok()? as u8) } else { Some(t) }
    };
    let rate = |r: &mut BitReader| -> Option<u32> {
        let i = r.read_bits(4).ok()? as usize;
        if i == 15 { r.read_bits(24).ok() } else { AAC_RATES.get(i).copied() }
    };
    let t = aot(&mut r)?;
    let sr = rate(&mut r)?;
    let ch = r.read_bits(4).ok()? as u8;
    Some((t, sr, ch))
}

/// Uncompressed PCM description.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PcmConfig {
    pub bits: u16,
    pub float: bool,
    pub big_endian: bool,
    /// Signed integer samples (8-bit `raw ` is unsigned).
    pub signed: bool,
    pub channels: u32,
    pub sample_rate: f64,
}

impl PcmConfig {
    /// Bytes per interleaved audio frame (all channels).
    pub fn bytes_per_frame(&self) -> u32 {
        (self.bits as u32).div_ceil(8) * self.channels.max(1)
    }
}

/// `dOps` — Opus specific box.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OpusConfig {
    pub output_channels: u8,
    pub pre_skip: u16,
    pub input_sample_rate: u32,
    pub output_gain: i16,
    pub channel_mapping_family: u8,
    /// StreamCount, CoupledCount and ChannelMapping bytes when family != 0.
    pub channel_mapping: Vec<u8>,
}

impl OpusConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "dOps");
        let _version = c.u8()?;
        Ok(OpusConfig {
            output_channels: c.u8()?,
            pre_skip: c.u16()?,
            input_sample_rate: c.u32()?,
            output_gain: c.i16()?,
            channel_mapping_family: c.u8()?,
            channel_mapping: c.rest().to_vec(),
        })
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = BoxBuf::new();
        b.u8(0);
        b.u8(self.output_channels);
        b.u16(self.pre_skip);
        b.u32(self.input_sample_rate);
        b.i16(self.output_gain);
        b.u8(self.channel_mapping_family);
        b.bytes(&self.channel_mapping);
        b.buf
    }
}

/// `dfLa` — FLAC metadata blocks, with STREAMINFO decoded.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FlacConfig {
    /// Raw metadata blocks (each with its 4-byte header) as stored in `dfLa`.
    pub metadata_blocks: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
}

impl FlacConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "dfLa");
        c.full_header()?;
        let blocks = c.rest().to_vec();
        let mut cfg = FlacConfig { metadata_blocks: blocks.clone(), ..Default::default() };
        let mut b = Cur::new(&blocks, "dfLa");
        let hdr = b.u32()?;
        if (hdr >> 24) & 0x7F == 0 {
            let si = b.bytes(((hdr & 0xFF_FFFF) as usize).min(34))?;
            if si.len() >= 18 {
                let mut r = BitReader::new(&si[10..18]);
                cfg.sample_rate = r.read_bits(20).unwrap_or(0);
                cfg.channels = r.read_bits(3).unwrap_or(0) as u8 + 1;
                cfg.bits_per_sample = r.read_bits(5).unwrap_or(0) as u8 + 1;
                cfg.total_samples = r.read_bits_u64(36).unwrap_or(0);
            }
        }
        Ok(cfg)
    }
}

pub(super) fn qt_pcm(f: &[u8; 4], a: &AudioParams) -> PcmConfig {
    let mut p = PcmConfig { channels: a.channels, sample_rate: a.sample_rate, signed: true, ..Default::default() };
    match f {
        b"lpcm" => {
            p.bits = a.const_bits_per_channel as u16;
            p.float = a.lpcm_flags & 1 != 0;
            p.big_endian = a.lpcm_flags & 2 != 0;
            p.signed = a.lpcm_flags & 4 != 0 || p.float;
            if p.bits == 0 {
                p.bits = a.sample_size;
            }
        }
        b"sowt" => {
            p.bits = if a.sample_size == 0 { 16 } else { a.sample_size };
        }
        b"twos" => {
            p.bits = if a.sample_size == 0 { 16 } else { a.sample_size };
            p.big_endian = true;
        }
        b"raw " | b"NONE" => {
            p.bits = if a.sample_size == 0 { 8 } else { a.sample_size };
            p.signed = p.bits != 8;
            p.big_endian = true;
        }
        b"in24" | b"in32" | b"fl32" | b"fl64" => {
            p.bits = match f {
                b"in24" => 24,
                b"in32" | b"fl32" => 32,
                _ => 64,
            };
            p.float = f[0] == b'f';
            p.big_endian = !a.little_endian.unwrap_or(false);
        }
        _ => {}
    }
    p
}

fn descriptor(c: &mut Cur) -> Result<(u8, usize)> {
    let tag = c.u8()?;
    let mut len = 0usize;
    for _ in 0..4 {
        let b = c.u8()?;
        len = (len << 7) | (b & 0x7F) as usize;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((tag, len))
}

pub(super) fn parse_esds(p: &[u8]) -> Result<CodecConfig> {
    let mut c = Cur::new(p, "esds");
    c.full_header()?;
    let (tag, len) = descriptor(&mut c)?;
    if tag != 3 {
        return Err(Error::Invalid(format!("esds: expected ES_Descriptor, got tag {tag}")));
    }
    let es = c.bytes(len.min(c.remaining()))?;
    let mut c = Cur::new(es, "ES_Descriptor");
    c.u16()?;
    let fl = c.u8()?;
    if fl & 0x80 != 0 {
        c.u16()?;
    }
    if fl & 0x40 != 0 {
        let n = c.u8()? as usize;
        c.skip(n)?;
    }
    if fl & 0x20 != 0 {
        c.u16()?;
    }
    while c.remaining() > 0 {
        let (tag, len) = descriptor(&mut c)?;
        let body = c.bytes(len.min(c.remaining()))?;
        if tag != 4 {
            continue;
        }
        let mut d = Cur::new(body, "DecoderConfigDescriptor");
        let oti = d.u8()?;
        let _stream_type = d.u8()?;
        let buffer_size = d.u24()?;
        let max_bitrate = d.u32()?;
        let avg_bitrate = d.u32()?;
        let mut asc = Vec::new();
        while d.remaining() > 0 {
            let (t, l) = descriptor(&mut d)?;
            let b = d.bytes(l.min(d.remaining()))?;
            if t == 5 {
                asc = b.to_vec();
                break;
            }
        }
        return Ok(match oti {
            0x69 | 0x6B => CodecConfig::Mp3,
            _ => {
                let mut a = AacConfig::from_asc(asc);
                a.max_bitrate = max_bitrate;
                a.avg_bitrate = avg_bitrate;
                a.buffer_size = buffer_size;
                CodecConfig::Aac(a)
            }
        });
    }
    Err(Error::Invalid("esds without DecoderConfigDescriptor".into()))
}

fn put_descriptor(b: &mut BoxBuf, tag: u8, body: &[u8]) {
    b.u8(tag);
    let n = body.len() as u32;
    // Always use the 4-byte length form for simplicity (as many encoders do).
    b.u8(0x80 | ((n >> 21) & 0x7F) as u8);
    b.u8(0x80 | ((n >> 14) & 0x7F) as u8);
    b.u8(0x80 | ((n >> 7) & 0x7F) as u8);
    b.u8((n & 0x7F) as u8);
    b.bytes(body);
}

pub(crate) fn esds_payload(oti: u8, asc: &[u8], max_bitrate: u32, avg_bitrate: u32, buffer_size: u32) -> Vec<u8> {
    let mut dcd = BoxBuf::new();
    dcd.u8(oti);
    dcd.u8(0x15); // streamType audio (5) << 2 | upstream 0 | reserved 1
    dcd.u8((buffer_size >> 16) as u8);
    dcd.u16(buffer_size as u16);
    dcd.u32(max_bitrate);
    dcd.u32(avg_bitrate);
    if !asc.is_empty() {
        put_descriptor(&mut dcd, 5, asc);
    }
    let mut es = BoxBuf::new();
    es.u16(1); // ES_ID
    es.u8(0);
    put_descriptor(&mut es, 4, &dcd.buf);
    put_descriptor(&mut es, 6, &[2]); // SLConfigDescriptor, predefined = 2 (MP4)
    let mut b = BoxBuf::new();
    b.u32(0);
    put_descriptor(&mut b, 3, &es.buf);
    b.buf
}

#[cfg(test)]
mod tests {
    use super::super::SampleEntry;
    use super::super::tests::roundtrip;
    use super::*;

    #[test]
    fn aac_esds_roundtrip() {
        let e = SampleEntry::aac(vec![0x12, 0x10], 2, 44100);
        match &e.codec {
            CodecConfig::Aac(a) => {
                assert_eq!((a.object_type, a.sample_rate, a.channel_config), (2, 44100, 2));
            }
            _ => panic!(),
        }
        assert_eq!(roundtrip(&e, false, b"soun"), e);
        assert_eq!(roundtrip(&e, true, b"soun"), e);
    }

    #[test]
    fn pcm_entries() {
        for (bits, float, be, qt) in [
            (16, false, false, true),
            (16, false, true, true),
            (24, false, false, true),
            (32, true, false, true),
            (24, false, true, false),
            (32, true, false, false),
        ] {
            let p = PcmConfig { bits, float, big_endian: be, signed: true, channels: 2, sample_rate: 48000.0 };
            let e = SampleEntry::pcm(p);
            let back = roundtrip(&e, qt, b"soun");
            assert_eq!(back.codec, CodecConfig::Pcm(p), "{bits} {float} {be} {qt}");
        }
    }

    #[test]
    fn asc_parse() {
        assert_eq!(parse_asc(&[0x11, 0x90]), Some((2, 48000, 2)));
        assert_eq!(parse_asc(&[]), None);
    }
}

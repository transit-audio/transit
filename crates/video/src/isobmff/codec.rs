//! Sample entries (`stsd` children) and codec configuration records: parsing and serialization.

mod audio;
mod timecode;
mod video;

pub(crate) use audio::esds_payload;
pub use audio::{AacConfig, FlacConfig, OpusConfig, PcmConfig, parse_asc};
use audio::{parse_esds, qt_pcm};
pub use timecode::TimecodeConfig;
pub use video::{Av1Config, AvcConfig, HevcConfig, HevcNalArray, VpcConfig};

use crate::isobmff::bytes::{BoxBuf, Cur, FourCc, boxes};
use crate::isobmff::error::{Error, Result};

/// One sample description (an entry of `stsd`).
#[derive(Clone, Debug, PartialEq)]
pub struct SampleEntry {
    /// Sample entry format (`avc1`, `mp4a`, `apch`, …).
    pub format: FourCc,
    pub data_reference_index: u16,
    pub codec: CodecConfig,
    /// Present for visual sample entries.
    pub video: Option<VideoParams>,
    /// Present for audio sample entries.
    pub audio: Option<AudioParams>,
    /// `btrt` box.
    pub bitrate: Option<BitRate>,
}

/// Fields common to all visual sample entries plus the optional descriptive boxes.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct VideoParams {
    pub width: u16,
    pub height: u16,
    /// Horizontal/vertical resolution in pixels per inch (16.16 fixed point; 72 dpi = 0x0048_0000).
    pub horiz_resolution: u32,
    pub vert_resolution: u32,
    pub frame_count: u16,
    pub compressor_name: String,
    pub depth: u16,
    /// QuickTime `vendor` / version fields (zero in ISO files).
    pub vendor: FourCc,
    pub temporal_quality: u32,
    pub spatial_quality: u32,
    pub color: Option<ColorInfo>,
    /// `pasp`: (hSpacing, vSpacing).
    pub pixel_aspect: Option<(u32, u32)>,
    pub clean_aperture: Option<CleanAperture>,
    pub field_info: Option<FieldInfo>,
    /// QuickTime `gama` (16.16 fixed point).
    pub gamma: Option<u32>,
    /// `mdcv`: mastering display colour volume (SMPTE ST 2086).
    pub mastering_display: Option<MasteringDisplay>,
    /// `clli`: content light level (MaxCLL, MaxFALL in cd/m²; 0 = unknown).
    pub content_light: Option<(u16, u16)>,
}

/// SMPTE ST 2086 mastering display metadata, in the units of the `mdcv` box and the H.264/HEVC
/// SEI: chromaticities in 0.00002 steps (order G, B, R as the SEI), luminance in 0.0001 cd/m².
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MasteringDisplay {
    pub primaries: [(u16, u16); 3],
    pub white_point: (u16, u16),
    pub max_luminance: u32,
    pub min_luminance: u32,
}

impl MasteringDisplay {
    /// BT.2020 primaries, D65, with the given peak and black in cd/m².
    pub fn bt2020(max_nits: f64, min_nits: f64) -> Self {
        let q = |v: f64| (v / 0.00002).round() as u16;
        MasteringDisplay {
            primaries: [(q(0.170), q(0.797)), (q(0.131), q(0.046)), (q(0.708), q(0.292))],
            white_point: (q(0.3127), q(0.3290)),
            max_luminance: (max_nits * 10_000.0).round() as u32,
            min_luminance: (min_nits * 10_000.0).round() as u32,
        }
    }
    pub fn max_nits(&self) -> f64 {
        self.max_luminance as f64 / 10_000.0
    }
    /// The 24-byte payload shared by `mdcv` and the SEI message.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(24);
        for (x, y) in self.primaries {
            v.extend_from_slice(&x.to_be_bytes());
            v.extend_from_slice(&y.to_be_bytes());
        }
        v.extend_from_slice(&self.white_point.0.to_be_bytes());
        v.extend_from_slice(&self.white_point.1.to_be_bytes());
        v.extend_from_slice(&self.max_luminance.to_be_bytes());
        v.extend_from_slice(&self.min_luminance.to_be_bytes());
        v
    }
    pub fn parse(p: &[u8]) -> Option<Self> {
        if p.len() < 24 {
            return None;
        }
        let u16_at = |i: usize| u16::from_be_bytes([p[i], p[i + 1]]);
        let u32_at = |i: usize| u32::from_be_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
        Some(MasteringDisplay {
            primaries: [(u16_at(0), u16_at(2)), (u16_at(4), u16_at(6)), (u16_at(8), u16_at(10))],
            white_point: (u16_at(12), u16_at(14)),
            max_luminance: u32_at(16),
            min_luminance: u32_at(20),
        })
    }
}

/// `colr` box contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorInfo {
    /// ISO `nclx` (with full-range flag).
    Nclx { primaries: u16, transfer: u16, matrix: u16, full_range: bool },
    /// QuickTime `nclc`.
    Nclc { primaries: u16, transfer: u16, matrix: u16 },
    /// Embedded ICC profile (`prof` restricted/unrestricted `rICC`).
    Icc { kind: FourCc, profile: Vec<u8> },
}

/// `clap` clean aperture (all rationals num/den; offsets are signed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanAperture {
    pub width: (u32, u32),
    pub height: (u32, u32),
    pub horiz_offset: (i32, u32),
    pub vert_offset: (i32, u32),
}

/// QuickTime `fiel`: field count (1 progressive, 2 interlaced) and ordering detail
/// (0, 1 = top first, 6 = bottom first, 9 / 14 per the QuickTime spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub fields: u8,
    pub detail: u8,
}

/// `btrt` box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitRate {
    pub buffer_size: u32,
    pub max_bitrate: u32,
    pub avg_bitrate: u32,
}

/// Fields of audio sample entries (ISO v0 plus QuickTime sound description v1/v2 extensions).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioParams {
    /// QuickTime sound description version (0, 1, 2). ISO files use 0.
    pub qt_version: u16,
    pub channels: u32,
    /// Bits per sample from the entry (16 for most compressed formats).
    pub sample_size: u16,
    pub sample_rate: f64,
    pub compression_id: i16,
    /// v1 fields.
    pub samples_per_packet: u32,
    pub bytes_per_packet: u32,
    pub bytes_per_frame: u32,
    pub bytes_per_sample: u32,
    /// v2 fields.
    pub const_bits_per_channel: u32,
    pub lpcm_flags: u32,
    pub const_bytes_per_packet: u32,
    pub const_frames_per_packet: u32,
    /// `enda` little-endian flag inside a QuickTime `wave` box.
    pub little_endian: Option<bool>,
}

/// Codec configuration decoded from the sample entry.
#[derive(Clone, Debug, PartialEq)]
pub enum CodecConfig {
    Avc(AvcConfig),
    Hevc(HevcConfig),
    Av1(Av1Config),
    Vp9(VpcConfig),
    /// Apple ProRes; fourcc is `apch`/`apcn`/`apcs`/`apco`/`ap4h`/`ap4x`.
    ProRes {
        fourcc: FourCc,
    },
    /// Motion JPEG (`jpeg`, `mjpa`, `mjpb`).
    Jpeg {
        fourcc: FourCc,
    },
    /// Avid DNxHD/DNxHR (`AVdn`, `AVdh`).
    Dnx {
        fourcc: FourCc,
    },
    Aac(AacConfig),
    /// MPEG-1/2 audio layer III (esds object type 0x69/0x6B, or QuickTime `.mp3`).
    Mp3,
    Pcm(PcmConfig),
    /// ALAC magic cookie (ALACSpecificConfig, 24 bytes, optionally followed by a channel layout).
    Alac {
        cookie: Vec<u8>,
    },
    Opus(OpusConfig),
    Flac(FlacConfig),
    /// `dac3` payload.
    Ac3 {
        dac3: Vec<u8>,
    },
    /// `dec3` payload.
    Eac3 {
        dec3: Vec<u8>,
    },
    Timecode(TimecodeConfig),
    /// Anything else; `raw` is the full sample entry payload (after the 8-byte box header).
    Unknown {
        fourcc: FourCc,
        raw: Vec<u8>,
    },
}

impl CodecConfig {
    /// A short human-readable codec name.
    pub fn name(&self) -> &'static str {
        match self {
            CodecConfig::Avc(_) => "h264",
            CodecConfig::Hevc(_) => "hevc",
            CodecConfig::Av1(_) => "av1",
            CodecConfig::Vp9(_) => "vp9",
            CodecConfig::ProRes { .. } => "prores",
            CodecConfig::Jpeg { .. } => "mjpeg",
            CodecConfig::Dnx { .. } => "dnxhd",
            CodecConfig::Aac(_) => "aac",
            CodecConfig::Mp3 => "mp3",
            CodecConfig::Pcm(_) => "pcm",
            CodecConfig::Alac { .. } => "alac",
            CodecConfig::Opus(_) => "opus",
            CodecConfig::Flac(_) => "flac",
            CodecConfig::Ac3 { .. } => "ac3",
            CodecConfig::Eac3 { .. } => "eac3",
            CodecConfig::Timecode(_) => "timecode",
            CodecConfig::Unknown { .. } => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing

fn is_video_fourcc(f: &[u8; 4]) -> bool {
    matches!(
        f,
        b"avc1"
            | b"avc3"
            | b"hvc1"
            | b"hev1"
            | b"av01"
            | b"vp09"
            | b"vp08"
            | b"apch"
            | b"apcn"
            | b"apcs"
            | b"apco"
            | b"ap4h"
            | b"ap4x"
            | b"jpeg"
            | b"mjpa"
            | b"mjpb"
            | b"AVdn"
            | b"AVdh"
            | b"mp4v"
            | b"encv"
            | b"raw "
            | b"2vuy"
            | b"v210"
    )
}

fn is_audio_fourcc(f: &[u8; 4]) -> bool {
    matches!(
        f,
        b"mp4a"
            | b"lpcm"
            | b"sowt"
            | b"twos"
            | b"in24"
            | b"in32"
            | b"fl32"
            | b"fl64"
            | b"ipcm"
            | b"fpcm"
            | b"alac"
            | b"Opus"
            | b"fLaC"
            | b"ac-3"
            | b"ec-3"
            | b".mp3"
            | b"enca"
            | b"ulaw"
            | b"alaw"
            | b"NONE"
    )
}

/// Parse one `stsd` entry. `handler` is the track handler type; `is_qt` selects QuickTime
/// interpretation of the sound description version field.
pub(crate) fn parse_sample_entry(format: FourCc, payload: &[u8], handler: FourCc, is_qt: bool) -> Result<SampleEntry> {
    let mut c = Cur::new(payload, "sample entry");
    c.skip(6)?;
    let data_reference_index = c.u16()?;
    let f = &format.0;
    let unknown = || CodecConfig::Unknown { fourcc: format, raw: payload.to_vec() };
    if f == b"tmcd" || (handler == b"tmcd" && !is_video_fourcc(f)) {
        let mut tc = TimecodeConfig::default();
        c.skip(4)?;
        tc.flags = c.u32()?;
        tc.timescale = c.u32()?;
        tc.frame_duration = c.u32()?;
        tc.frames_per_second = c.u8()?;
        c.skip(1)?;
        for b in boxes(c.rest()).map_while(|b| b.ok()) {
            if b.kind == b"name" && b.payload.len() >= 4 {
                let n = u16::from_be_bytes([b.payload[0], b.payload[1]]) as usize;
                let s = b.payload.get(4..4 + n).unwrap_or(&b.payload[4..]);
                tc.name = Some(String::from_utf8_lossy(s).into_owned());
            }
        }
        return Ok(SampleEntry { format, data_reference_index, codec: CodecConfig::Timecode(tc), video: None, audio: None, bitrate: None });
    }
    let video_like = is_video_fourcc(f) || (handler == b"vide" && !is_audio_fourcc(f));
    let audio_like = is_audio_fourcc(f) || (handler == b"soun" && !video_like);
    if video_like {
        let mut v = VideoParams::default();
        let _version = c.u16()?;
        let _revision = c.u16()?;
        v.vendor = c.fourcc()?;
        v.temporal_quality = c.u32()?;
        v.spatial_quality = c.u32()?;
        v.width = c.u16()?;
        v.height = c.u16()?;
        v.horiz_resolution = c.u32()?;
        v.vert_resolution = c.u32()?;
        c.skip(4)?;
        v.frame_count = c.u16()?;
        let name = c.bytes(32)?;
        let n = (name[0] as usize).min(31);
        v.compressor_name = String::from_utf8_lossy(&name[1..1 + n]).into_owned();
        v.depth = c.u16()?;
        let _ctab = c.i16()?;
        let mut codec = None;
        let mut bitrate = None;
        for b in boxes(c.rest()).map_while(|b| b.ok()) {
            let p = b.payload;
            match &b.kind.0 {
                b"avcC" if matches!(f, b"avc1" | b"avc3") => codec = Some(CodecConfig::Avc(AvcConfig::parse(p)?)),
                b"hvcC" if matches!(f, b"hvc1" | b"hev1") => codec = Some(CodecConfig::Hevc(HevcConfig::parse(p)?)),
                b"av1C" if f == b"av01" => codec = Some(CodecConfig::Av1(Av1Config::parse(p)?)),
                b"vpcC" if f == b"vp09" => codec = Some(CodecConfig::Vp9(VpcConfig::parse(p)?)),
                b"colr" => v.color = parse_colr(p).ok(),
                b"mdcv" => v.mastering_display = MasteringDisplay::parse(p),
                b"clli" if p.len() >= 4 => v.content_light = Some((u16::from_be_bytes([p[0], p[1]]), u16::from_be_bytes([p[2], p[3]]))),
                b"pasp" => {
                    let mut c = Cur::new(p, "pasp");
                    v.pixel_aspect = Some((c.u32()?, c.u32()?));
                }
                b"clap" => {
                    let mut c = Cur::new(p, "clap");
                    v.clean_aperture = Some(CleanAperture {
                        width: (c.u32()?, c.u32()?),
                        height: (c.u32()?, c.u32()?),
                        horiz_offset: (c.i32()?, c.u32()?),
                        vert_offset: (c.i32()?, c.u32()?),
                    });
                }
                b"fiel" => {
                    let mut c = Cur::new(p, "fiel");
                    v.field_info = Some(FieldInfo { fields: c.u8()?, detail: c.u8()? });
                }
                b"gama" => v.gamma = Some(Cur::new(p, "gama").u32()?),
                b"btrt" => bitrate = Some(parse_btrt(p)?),
                _ => {}
            }
        }
        let codec = match codec {
            Some(c) => c,
            None => match f {
                b"apch" | b"apcn" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => CodecConfig::ProRes { fourcc: format },
                b"jpeg" | b"mjpa" | b"mjpb" => CodecConfig::Jpeg { fourcc: format },
                b"AVdn" | b"AVdh" => CodecConfig::Dnx { fourcc: format },
                _ => unknown(),
            },
        };
        return Ok(SampleEntry { format, data_reference_index, codec, video: Some(v), audio: None, bitrate });
    }
    if audio_like {
        let mut a = AudioParams { qt_version: c.u16()?, ..Default::default() };
        let _revision = c.u16()?;
        let _vendor = c.u32()?;
        a.channels = c.u16()? as u32;
        a.sample_size = c.u16()?;
        a.compression_id = c.i16()?;
        let _packet_size = c.u16()?;
        a.sample_rate = c.u32()? as f64 / 65536.0;
        if is_qt && a.qt_version == 1 {
            a.samples_per_packet = c.u32()?;
            a.bytes_per_packet = c.u32()?;
            a.bytes_per_frame = c.u32()?;
            a.bytes_per_sample = c.u32()?;
        } else if is_qt && a.qt_version == 2 {
            let _size_of_struct = c.u32()?;
            a.sample_rate = c.f64()?;
            a.channels = c.u32()?;
            let _always = c.u32()?;
            a.const_bits_per_channel = c.u32()?;
            a.lpcm_flags = c.u32()?;
            a.const_bytes_per_packet = c.u32()?;
            a.const_frames_per_packet = c.u32()?;
        }
        // Flatten QuickTime `wave` children into the list of extension boxes.
        let mut ext: Vec<(FourCc, &[u8])> = Vec::new();
        for b in boxes(c.rest()).map_while(|b| b.ok()) {
            if b.kind == b"wave" {
                for w in boxes(b.payload).map_while(|b| b.ok()) {
                    ext.push((w.kind, w.payload));
                }
            } else {
                ext.push((b.kind, b.payload));
            }
        }
        let get = |k: &[u8; 4]| ext.iter().find(|(kk, _)| kk == k).map(|(_, p)| *p);
        if let Some(p) = get(b"enda") {
            let v = if p.len() >= 2 { u16::from_be_bytes([p[0], p[1]]) } else { p.first().copied().unwrap_or(0) as u16 };
            a.little_endian = Some(v != 0);
        }
        let bitrate = get(b"btrt").and_then(|p| parse_btrt(p).ok());
        let codec = match f {
            b"mp4a" => match get(b"esds") {
                Some(p) => parse_esds(p)?,
                None => unknown(),
            },
            b".mp3" => CodecConfig::Mp3,
            b"alac" => match get(b"alac") {
                Some(p) if p.len() >= 4 => CodecConfig::Alac { cookie: p[4..].to_vec() },
                _ => unknown(),
            },
            b"Opus" => match get(b"dOps") {
                Some(p) => CodecConfig::Opus(OpusConfig::parse(p)?),
                None => unknown(),
            },
            b"fLaC" => match get(b"dfLa") {
                Some(p) => CodecConfig::Flac(FlacConfig::parse(p)?),
                None => unknown(),
            },
            b"ac-3" => CodecConfig::Ac3 { dac3: get(b"dac3").unwrap_or(&[]).to_vec() },
            b"ec-3" => CodecConfig::Eac3 { dec3: get(b"dec3").unwrap_or(&[]).to_vec() },
            b"lpcm" | b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"raw " | b"NONE" => CodecConfig::Pcm(qt_pcm(f, &a)),
            b"ipcm" | b"fpcm" => {
                let mut pcm = PcmConfig {
                    bits: a.sample_size,
                    float: f == b"fpcm",
                    big_endian: true,
                    signed: true,
                    channels: a.channels,
                    sample_rate: a.sample_rate,
                };
                if let Some(p) = get(b"pcmC") {
                    let mut c = Cur::new(p, "pcmC");
                    c.full_header()?;
                    let flags = c.u8()?;
                    pcm.big_endian = flags & 1 == 0;
                    pcm.bits = c.u8()? as u16;
                }
                CodecConfig::Pcm(pcm)
            }
            _ => unknown(),
        };
        return Ok(SampleEntry { format, data_reference_index, codec, video: None, audio: Some(a), bitrate });
    }
    Ok(SampleEntry { format, data_reference_index, codec: unknown(), video: None, audio: None, bitrate: None })
}

fn parse_btrt(p: &[u8]) -> Result<BitRate> {
    let mut c = Cur::new(p, "btrt");
    Ok(BitRate { buffer_size: c.u32()?, max_bitrate: c.u32()?, avg_bitrate: c.u32()? })
}

fn parse_colr(p: &[u8]) -> Result<ColorInfo> {
    let mut c = Cur::new(p, "colr");
    let kind = c.fourcc()?;
    Ok(match &kind.0 {
        b"nclx" => {
            ColorInfo::Nclx { primaries: c.u16()?, transfer: c.u16()?, matrix: c.u16()?, full_range: c.u8().map(|v| v & 0x80 != 0).unwrap_or(false) }
        }
        b"nclc" => ColorInfo::Nclc { primaries: c.u16()?, transfer: c.u16()?, matrix: c.u16()? },
        _ => ColorInfo::Icc { kind, profile: c.rest().to_vec() },
    })
}

// ---------------------------------------------------------------------------------------------
// Serialization (muxer)

impl SampleEntry {
    /// Convenience constructor for a video entry.
    pub fn video(format: FourCc, codec: CodecConfig, width: u16, height: u16) -> Self {
        let compressor_name = match &codec {
            CodecConfig::ProRes { fourcc } => match &fourcc.0 {
                b"apch" => "Apple ProRes 422 HQ",
                b"apcn" => "Apple ProRes 422",
                b"apcs" => "Apple ProRes 422 LT",
                b"apco" => "Apple ProRes 422 Proxy",
                b"ap4h" => "Apple ProRes 4444",
                _ => "Apple ProRes 4444 XQ",
            },
            CodecConfig::Jpeg { .. } => "Photo - JPEG",
            _ => "",
        };
        SampleEntry {
            format,
            data_reference_index: 1,
            codec,
            video: Some(VideoParams {
                width,
                height,
                horiz_resolution: 0x0048_0000,
                vert_resolution: 0x0048_0000,
                frame_count: 1,
                compressor_name: compressor_name.into(),
                depth: 24,
                ..Default::default()
            }),
            audio: None,
            bitrate: None,
        }
    }

    /// Convenience constructor for an audio entry (ISO v0 layout; the muxer upgrades PCM in MOV to v2).
    pub fn audio(format: FourCc, codec: CodecConfig, channels: u32, sample_rate: f64, sample_size: u16) -> Self {
        SampleEntry {
            format,
            data_reference_index: 1,
            codec,
            video: None,
            audio: Some(AudioParams { channels, sample_rate, sample_size, ..Default::default() }),
            bitrate: None,
        }
    }

    /// H.264 entry (`avc1`).
    pub fn avc(cfg: AvcConfig, width: u16, height: u16) -> Self {
        Self::video(FourCc(*b"avc1"), CodecConfig::Avc(cfg), width, height)
    }
    /// HEVC entry (`hvc1`, parameter sets in the config only).
    pub fn hevc(cfg: HevcConfig, width: u16, height: u16) -> Self {
        Self::video(FourCc(*b"hvc1"), CodecConfig::Hevc(cfg), width, height)
    }
    /// ProRes entry; `fourcc` one of `apch`/`apcn`/`apcs`/`apco`/`ap4h`/`ap4x`.
    pub fn prores(fourcc: FourCc, width: u16, height: u16) -> Self {
        let mut e = Self::video(fourcc, CodecConfig::ProRes { fourcc }, width, height);
        if let Some(v) = e.video.as_mut()
            && matches!(&fourcc.0, b"ap4h" | b"ap4x")
        {
            v.depth = 32;
        }
        e
    }
    /// Avid DNxHD / DNxHR entry (`AVdn`, or `AVdh` for DNxHR).
    pub fn dnx(fourcc: FourCc, width: u16, height: u16) -> Self {
        let mut e = Self::video(fourcc, CodecConfig::Dnx { fourcc }, width, height);
        if let Some(v) = e.video.as_mut() {
            v.depth = 24;
        }
        e
    }
    /// Motion-JPEG entry (`jpeg`).
    pub fn jpeg(width: u16, height: u16) -> Self {
        let f = FourCc(*b"jpeg");
        Self::video(f, CodecConfig::Jpeg { fourcc: f }, width, height)
    }
    /// AAC entry (`mp4a` + `esds`) from an AudioSpecificConfig.
    pub fn aac(asc: Vec<u8>, channels: u32, sample_rate: u32) -> Self {
        Self::audio(FourCc(*b"mp4a"), CodecConfig::Aac(AacConfig::from_asc(asc)), channels, sample_rate as f64, 16)
    }
    /// PCM entry. The muxer picks the on-disk format (`sowt`/`twos`/`lpcm` for MOV, `ipcm`/`fpcm` for MP4).
    pub fn pcm(cfg: PcmConfig) -> Self {
        Self::audio(FourCc(*b"lpcm"), CodecConfig::Pcm(cfg), cfg.channels, cfg.sample_rate, cfg.bits)
    }

    /// Serialize as a complete sample entry box. For PCM the format is chosen from the brand.
    pub(crate) fn write(&self, b: &mut BoxBuf, qt: bool) -> Result<()> {
        if let CodecConfig::Unknown { fourcc, raw } = &self.codec {
            let m = b.start(&fourcc.0);
            b.bytes(raw);
            b.end(m);
            return Ok(());
        }
        if let CodecConfig::Timecode(tc) = &self.codec {
            let m = b.start(b"tmcd");
            b.zeros(6);
            b.u16(self.data_reference_index);
            b.u32(0);
            b.u32(tc.flags);
            b.u32(tc.timescale);
            b.u32(tc.frame_duration);
            b.u8(tc.frames_per_second);
            b.u8(0);
            if let Some(name) = &tc.name {
                let n = b.start(b"name");
                b.u16(name.len() as u16);
                b.u16(0);
                b.bytes(name.as_bytes());
                b.end(n);
            }
            b.end(m);
            return Ok(());
        }
        if let Some(v) = &self.video {
            let m = b.start(&self.format.0);
            b.zeros(6);
            b.u16(self.data_reference_index);
            b.u16(0);
            b.u16(0);
            b.u32(v.vendor.as_u32());
            b.u32(v.temporal_quality);
            b.u32(v.spatial_quality);
            b.u16(v.width);
            b.u16(v.height);
            b.u32(v.horiz_resolution);
            b.u32(v.vert_resolution);
            b.u32(0);
            b.u16(v.frame_count);
            let name = v.compressor_name.as_bytes();
            let n = name.len().min(31);
            let mut nb = [0u8; 32];
            nb[0] = n as u8;
            nb[1..1 + n].copy_from_slice(&name[..n]);
            b.bytes(&nb);
            b.u16(v.depth);
            b.i16(-1);
            match &self.codec {
                CodecConfig::Avc(c) => b.leaf(b"avcC", &c.to_bytes()),
                CodecConfig::Hevc(c) => b.leaf(b"hvcC", &c.to_bytes()),
                CodecConfig::Av1(c) => b.leaf(b"av1C", &c.to_bytes()),
                CodecConfig::Vp9(c) => b.leaf(b"vpcC", &c.to_bytes()),
                _ => {}
            }
            if let Some(g) = v.gamma {
                b.leaf(b"gama", &g.to_be_bytes());
            }
            if let Some(fi) = v.field_info {
                b.leaf(b"fiel", &[fi.fields, fi.detail]);
            }
            if let Some(ci) = &v.color {
                let m = b.start(b"colr");
                match ci {
                    ColorInfo::Nclx { primaries, transfer, matrix, full_range } => {
                        b.bytes(b"nclx");
                        b.u16(*primaries);
                        b.u16(*transfer);
                        b.u16(*matrix);
                        b.u8(if *full_range { 0x80 } else { 0 });
                    }
                    ColorInfo::Nclc { primaries, transfer, matrix } => {
                        b.bytes(b"nclc");
                        b.u16(*primaries);
                        b.u16(*transfer);
                        b.u16(*matrix);
                    }
                    ColorInfo::Icc { kind, profile } => {
                        b.bytes(&kind.0);
                        b.bytes(profile);
                    }
                }
                b.end(m);
            }
            if let Some(md) = &v.mastering_display {
                b.leaf(b"mdcv", &md.to_bytes());
            }
            if let Some((cll, fall)) = v.content_light {
                let mut d = cll.to_be_bytes().to_vec();
                d.extend_from_slice(&fall.to_be_bytes());
                b.leaf(b"clli", &d);
            }
            if let Some((h, vv)) = v.pixel_aspect {
                let m = b.start(b"pasp");
                b.u32(h);
                b.u32(vv);
                b.end(m);
            }
            if let Some(c) = v.clean_aperture {
                let m = b.start(b"clap");
                b.u32(c.width.0);
                b.u32(c.width.1);
                b.u32(c.height.0);
                b.u32(c.height.1);
                b.i32(c.horiz_offset.0);
                b.u32(c.horiz_offset.1);
                b.i32(c.vert_offset.0);
                b.u32(c.vert_offset.1);
                b.end(m);
            }
            if let Some(br) = self.bitrate {
                write_btrt(b, br);
            }
            b.end(m);
            return Ok(());
        }
        let a = self.audio.clone().ok_or_else(|| Error::Mux("sample entry has neither video nor audio params".into()))?;
        // Choose the on-disk format and sound description version.
        let (format, version, pcm) = match &self.codec {
            CodecConfig::Pcm(p) => {
                if qt {
                    if p.bits == 16 && !p.float {
                        (if p.big_endian { *b"twos" } else { *b"sowt" }, 0u16, Some(*p))
                    } else if p.bits == 8 && !p.float {
                        (*b"raw ", 0, Some(*p))
                    } else {
                        (*b"lpcm", 2, Some(*p))
                    }
                } else {
                    (if p.float { *b"fpcm" } else { *b"ipcm" }, 0, Some(*p))
                }
            }
            _ => (self.format.0, if qt { a.qt_version.min(2) } else { 0 }, None),
        };
        let m = b.start(&format);
        b.zeros(6);
        b.u16(self.data_reference_index);
        if version == 2 {
            let p = pcm.unwrap_or_default();
            b.u16(2);
            b.u16(0);
            b.u32(0);
            b.u16(3);
            b.u16(16);
            b.i16(-2);
            b.u16(0);
            b.u32(65536);
            b.u32(72);
            b.f64(p.sample_rate);
            b.u32(p.channels);
            b.u32(0x7F00_0000);
            b.u32(p.bits as u32);
            let flags = (p.float as u32) | ((p.big_endian as u32) << 1) | (((p.signed && !p.float) as u32) << 2) | 8;
            b.u32(flags);
            b.u32(p.bytes_per_frame());
            b.u32(1);
        } else {
            b.u16(version);
            b.u16(0);
            b.u32(0);
            b.u16(a.channels.min(u16::MAX as u32) as u16);
            b.u16(match pcm {
                Some(p) => p.bits,
                None => {
                    if a.sample_size == 0 {
                        16
                    } else {
                        a.sample_size
                    }
                }
            });
            b.i16(if version == 1 { a.compression_id } else { 0 });
            b.u16(0);
            let sr = if a.sample_rate < 65536.0 { (a.sample_rate * 65536.0).round() as u32 } else { 0 };
            b.u32(sr);
            if version == 1 {
                b.u32(a.samples_per_packet);
                b.u32(a.bytes_per_packet);
                b.u32(a.bytes_per_frame);
                b.u32(a.bytes_per_sample);
            }
        }
        match &self.codec {
            CodecConfig::Aac(c) => {
                b.leaf(b"esds", &esds_payload(0x40, &c.asc, c.max_bitrate, c.avg_bitrate, c.buffer_size));
            }
            CodecConfig::Mp3 if format == *b"mp4a" => {
                b.leaf(b"esds", &esds_payload(0x6B, &[], 0, 0, 0));
            }
            CodecConfig::Alac { cookie } => {
                let m = b.start_full(b"alac", 0, 0);
                b.bytes(cookie);
                b.end(m);
            }
            CodecConfig::Opus(o) => b.leaf(b"dOps", &o.to_bytes()),
            CodecConfig::Flac(fl) => {
                let m = b.start_full(b"dfLa", 0, 0);
                b.bytes(&fl.metadata_blocks);
                b.end(m);
            }
            CodecConfig::Ac3 { dac3 } => b.leaf(b"dac3", dac3),
            CodecConfig::Eac3 { dec3 } => b.leaf(b"dec3", dec3),
            CodecConfig::Pcm(p) if !qt => {
                let m = b.start_full(b"pcmC", 0, 0);
                b.u8(if p.big_endian { 0 } else { 1 });
                b.u8(p.bits as u8);
                b.end(m);
            }
            CodecConfig::Pcm(p) if p.channels == 6 => {
                // QuickTime channel layout: kAudioChannelLayoutTag_MPEG_5_1_A (L R C LFE Ls Rs)
                let m = b.start_full(b"chan", 0, 0);
                b.u32((121 << 16) | 6);
                b.u32(0);
                b.u32(0);
                b.end(m);
            }
            _ => {}
        }
        if let Some(br) = self.bitrate {
            write_btrt(b, br);
        }
        b.end(m);
        Ok(())
    }
}

fn write_btrt(b: &mut BoxBuf, br: BitRate) {
    let m = b.start(b"btrt");
    b.u32(br.buffer_size);
    b.u32(br.max_bitrate);
    b.u32(br.avg_bitrate);
    b.end(m);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn roundtrip(e: &SampleEntry, qt: bool, handler: &[u8; 4]) -> SampleEntry {
        let mut b = BoxBuf::new();
        e.write(&mut b, qt).unwrap();
        let bx = crate::isobmff::bytes::boxes(&b.buf).next().unwrap().unwrap();
        parse_sample_entry(bx.kind, bx.payload, FourCc(*handler), qt).unwrap()
    }

    #[test]
    fn avcc_roundtrip() {
        let cfg = AvcConfig::new(vec![vec![0x67, 0x64, 0x00, 0x1F, 0xAC]], vec![vec![0x68, 0xEB]], 4);
        assert_eq!(cfg.profile, 0x64);
        assert_eq!(AvcConfig::parse(&cfg.to_bytes()).unwrap(), cfg);
        let mut e = SampleEntry::avc(cfg, 1920, 1080);
        e.video.as_mut().unwrap().color = Some(ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: false });
        e.video.as_mut().unwrap().pixel_aspect = Some((4, 3));
        e.video.as_mut().unwrap().field_info = Some(FieldInfo { fields: 2, detail: 9 });
        e.video.as_mut().unwrap().clean_aperture =
            Some(CleanAperture { width: (1920, 1), height: (1080, 1), horiz_offset: (-2, 1), vert_offset: (0, 1) });
        e.video.as_mut().unwrap().mastering_display = Some(MasteringDisplay::bt2020(1000.0, 0.0001));
        e.video.as_mut().unwrap().content_light = Some((1000, 400));
        e.bitrate = Some(BitRate { buffer_size: 1, max_bitrate: 2, avg_bitrate: 3 });
        assert_eq!(roundtrip(&e, false, b"vide"), e);
        assert_eq!(MasteringDisplay::bt2020(1000.0, 0.0001).max_nits(), 1000.0);
    }

    #[test]
    fn av1_vp9_opus_roundtrip() {
        let a = Av1Config { seq_profile: 1, seq_level_idx_0: 8, high_bitdepth: true, config_obus: vec![1, 2], ..Default::default() };
        assert_eq!(Av1Config::parse(&a.to_bytes()).unwrap(), a);
        let v = VpcConfig {
            profile: 2,
            level: 31,
            bit_depth: 10,
            chroma_subsampling: 1,
            full_range: true,
            colour_primaries: 9,
            transfer_characteristics: 16,
            matrix_coefficients: 9,
            codec_init: vec![],
        };
        assert_eq!(VpcConfig::parse(&v.to_bytes()).unwrap(), v);
        let o = OpusConfig {
            output_channels: 2,
            pre_skip: 312,
            input_sample_rate: 48000,
            output_gain: 0,
            channel_mapping_family: 0,
            channel_mapping: vec![],
        };
        assert_eq!(OpusConfig::parse(&o.to_bytes()).unwrap(), o);
    }

    #[test]
    fn timecode_and_prores() {
        let tc =
            TimecodeConfig { flags: 1, timescale: 30000, frame_duration: 1001, frames_per_second: 30, start_frame: None, name: Some("reel".into()) };
        let e = SampleEntry {
            format: FourCc(*b"tmcd"),
            data_reference_index: 1,
            codec: CodecConfig::Timecode(tc.clone()),
            video: None,
            audio: None,
            bitrate: None,
        };
        assert_eq!(roundtrip(&e, true, b"tmcd"), e);
        assert_eq!(tc.format_frame(107892), "01:00:00;00");
        let nd = TimecodeConfig { flags: 0, frames_per_second: 24, ..Default::default() };
        assert_eq!(nd.format_frame(86400), "01:00:00:00");
        let p = SampleEntry::prores(FourCc(*b"apch"), 1920, 1080);
        assert_eq!(roundtrip(&p, true, b"vide"), p);
    }
}

use crate::isobmff::bytes::{BoxBuf, Cur};
use crate::isobmff::error::{Error, Result};

/// `avcC` — AVCDecoderConfigurationRecord (ISO/IEC 14496-15 §5.3.3).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AvcConfig {
    pub profile: u8,
    pub compatibility: u8,
    pub level: u8,
    /// NAL length field size in bytes (1, 2 or 4).
    pub length_size: u8,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
    /// Bytes after the PPS list (high-profile extension: chroma format, bit depths, SPS-ext).
    pub ext: Vec<u8>,
}

impl AvcConfig {
    /// Build from parameter sets (NAL units without start codes); profile/level taken from the first SPS.
    pub fn new(sps: Vec<Vec<u8>>, pps: Vec<Vec<u8>>, length_size: u8) -> Self {
        let (profile, compatibility, level) = sps.first().filter(|s| s.len() >= 4).map(|s| (s[1], s[2], s[3])).unwrap_or((0, 0, 0));
        AvcConfig { profile, compatibility, level, length_size, sps, pps, ext: Vec::new() }
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "avcC");
        let _version = c.u8()?;
        let profile = c.u8()?;
        let compatibility = c.u8()?;
        let level = c.u8()?;
        let length_size = (c.u8()? & 3) + 1;
        let nsps = (c.u8()? & 0x1F) as usize;
        let mut sps = Vec::with_capacity(nsps);
        for _ in 0..nsps {
            let n = c.u16()? as usize;
            sps.push(c.bytes(n)?.to_vec());
        }
        let npps = c.u8()? as usize;
        let mut pps = Vec::with_capacity(npps);
        for _ in 0..npps {
            let n = c.u16()? as usize;
            pps.push(c.bytes(n)?.to_vec());
        }
        Ok(AvcConfig { profile, compatibility, level, length_size, sps, pps, ext: c.rest().to_vec() })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = BoxBuf::new();
        b.u8(1);
        b.u8(self.profile);
        b.u8(self.compatibility);
        b.u8(self.level);
        b.u8(0xFC | (self.length_size.clamp(1, 4) - 1));
        b.u8(0xE0 | (self.sps.len().min(31) as u8));
        for s in self.sps.iter().take(31) {
            b.u16(s.len() as u16);
            b.bytes(s);
        }
        b.u8(self.pps.len().min(255) as u8);
        for p in self.pps.iter().take(255) {
            b.u16(p.len() as u16);
            b.bytes(p);
        }
        b.bytes(&self.ext);
        b.buf
    }
}

/// One NAL array of an `hvcC` record.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HevcNalArray {
    pub completeness: bool,
    pub nal_type: u8,
    pub nalus: Vec<Vec<u8>>,
}

/// `hvcC` — HEVCDecoderConfigurationRecord (ISO/IEC 14496-15 §8.3.3).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct HevcConfig {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    /// 48 bits of general constraint indicator flags.
    pub general_constraint_indicator_flags: u64,
    pub general_level_idc: u8,
    pub min_spatial_segmentation_idc: u16,
    pub parallelism_type: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub avg_frame_rate: u16,
    pub constant_frame_rate: u8,
    pub num_temporal_layers: u8,
    pub temporal_id_nested: bool,
    pub length_size: u8,
    pub arrays: Vec<HevcNalArray>,
}

impl HevcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "hvcC");
        let _version = c.u8()?;
        let b = c.u8()?;
        let compat = c.u32()?;
        let hi = c.u16()? as u64;
        let lo = c.u32()? as u64;
        let level = c.u8()?;
        let min_seg = c.u16()? & 0x0FFF;
        let par = c.u8()? & 3;
        let chroma = c.u8()? & 3;
        let bdl = (c.u8()? & 7) + 8;
        let bdc = (c.u8()? & 7) + 8;
        let afr = c.u16()?;
        let x = c.u8()?;
        let narrays = c.u8()? as usize;
        let mut arrays = Vec::new();
        for _ in 0..narrays {
            let t = c.u8()?;
            let n = c.u16()? as usize;
            c.check_count(n, 2)?;
            let mut nalus = Vec::with_capacity(n);
            for _ in 0..n {
                let len = c.u16()? as usize;
                nalus.push(c.bytes(len)?.to_vec());
            }
            arrays.push(HevcNalArray { completeness: t & 0x80 != 0, nal_type: t & 0x3F, nalus });
        }
        Ok(HevcConfig {
            general_profile_space: b >> 6,
            general_tier_flag: b & 0x20 != 0,
            general_profile_idc: b & 0x1F,
            general_profile_compatibility_flags: compat,
            general_constraint_indicator_flags: (hi << 32) | lo,
            general_level_idc: level,
            min_spatial_segmentation_idc: min_seg,
            parallelism_type: par,
            chroma_format_idc: chroma,
            bit_depth_luma: bdl,
            bit_depth_chroma: bdc,
            avg_frame_rate: afr,
            constant_frame_rate: x >> 6,
            num_temporal_layers: (x >> 3) & 7,
            temporal_id_nested: x & 4 != 0,
            length_size: (x & 3) + 1,
            arrays,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = BoxBuf::new();
        b.u8(1);
        b.u8((self.general_profile_space << 6) | ((self.general_tier_flag as u8) << 5) | (self.general_profile_idc & 0x1F));
        b.u32(self.general_profile_compatibility_flags);
        b.u16((self.general_constraint_indicator_flags >> 32) as u16);
        b.u32(self.general_constraint_indicator_flags as u32);
        b.u8(self.general_level_idc);
        b.u16(0xF000 | (self.min_spatial_segmentation_idc & 0x0FFF));
        b.u8(0xFC | (self.parallelism_type & 3));
        b.u8(0xFC | (self.chroma_format_idc & 3));
        b.u8(0xF8 | (self.bit_depth_luma.saturating_sub(8) & 7));
        b.u8(0xF8 | (self.bit_depth_chroma.saturating_sub(8) & 7));
        b.u16(self.avg_frame_rate);
        b.u8((self.constant_frame_rate << 6)
            | ((self.num_temporal_layers & 7) << 3)
            | ((self.temporal_id_nested as u8) << 2)
            | (self.length_size.clamp(1, 4) - 1));
        b.u8(self.arrays.len() as u8);
        for a in &self.arrays {
            b.u8(((a.completeness as u8) << 7) | (a.nal_type & 0x3F));
            b.u16(a.nalus.len() as u16);
            for n in &a.nalus {
                b.u16(n.len() as u16);
                b.bytes(n);
            }
        }
        b.buf
    }

    fn nalus_of(&self, t: u8) -> Vec<&[u8]> {
        self.arrays.iter().filter(|a| a.nal_type == t).flat_map(|a| a.nalus.iter().map(|n| n.as_slice())).collect()
    }
    pub fn vps(&self) -> Vec<&[u8]> {
        self.nalus_of(32)
    }
    pub fn sps(&self) -> Vec<&[u8]> {
        self.nalus_of(33)
    }
    pub fn pps(&self) -> Vec<&[u8]> {
        self.nalus_of(34)
    }
}

/// `av1C` — AV1CodecConfigurationRecord.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Av1Config {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub initial_presentation_delay_minus_one: Option<u8>,
    pub config_obus: Vec<u8>,
}

impl Av1Config {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "av1C");
        let m = c.u8()?;
        if m & 0x80 == 0 {
            return Err(Error::Invalid("av1C marker bit not set".into()));
        }
        let a = c.u8()?;
        let b = c.u8()?;
        let d = c.u8()?;
        Ok(Av1Config {
            seq_profile: a >> 5,
            seq_level_idx_0: a & 0x1F,
            seq_tier_0: b & 0x80 != 0,
            high_bitdepth: b & 0x40 != 0,
            twelve_bit: b & 0x20 != 0,
            monochrome: b & 0x10 != 0,
            chroma_subsampling_x: b & 0x08 != 0,
            chroma_subsampling_y: b & 0x04 != 0,
            chroma_sample_position: b & 3,
            initial_presentation_delay_minus_one: if d & 0x10 != 0 { Some(d & 0x0F) } else { None },
            config_obus: c.rest().to_vec(),
        })
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = BoxBuf::new();
        b.u8(0x81);
        b.u8((self.seq_profile << 5) | (self.seq_level_idx_0 & 0x1F));
        b.u8(((self.seq_tier_0 as u8) << 7)
            | ((self.high_bitdepth as u8) << 6)
            | ((self.twelve_bit as u8) << 5)
            | ((self.monochrome as u8) << 4)
            | ((self.chroma_subsampling_x as u8) << 3)
            | ((self.chroma_subsampling_y as u8) << 2)
            | (self.chroma_sample_position & 3));
        b.u8(match self.initial_presentation_delay_minus_one {
            Some(d) => 0x10 | (d & 0x0F),
            None => 0,
        });
        b.bytes(&self.config_obus);
        b.buf
    }
}

/// `vpcC` — VP codec configuration (version 1).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct VpcConfig {
    pub profile: u8,
    pub level: u8,
    pub bit_depth: u8,
    pub chroma_subsampling: u8,
    pub full_range: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub codec_init: Vec<u8>,
}

impl VpcConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut c = Cur::new(data, "vpcC");
        let (version, _) = c.full_header()?;
        if version == 0 {
            // Version 0 draft layout: profile, level, bitDepth(4)|colorSpace(4), chroma(4)|transfer(3)|range(1)
            let profile = c.u8()?;
            let level = c.u8()?;
            let a = c.u8()?;
            let b = c.u8()?;
            return Ok(VpcConfig { profile, level, bit_depth: a >> 4, chroma_subsampling: b >> 4, full_range: b & 1 != 0, ..Default::default() });
        }
        let profile = c.u8()?;
        let level = c.u8()?;
        let a = c.u8()?;
        let colour_primaries = c.u8()?;
        let transfer_characteristics = c.u8()?;
        let matrix_coefficients = c.u8()?;
        let n = c.u16()? as usize;
        let codec_init = c.bytes(n)?.to_vec();
        Ok(VpcConfig {
            profile,
            level,
            bit_depth: a >> 4,
            chroma_subsampling: (a >> 1) & 7,
            full_range: a & 1 != 0,
            colour_primaries,
            transfer_characteristics,
            matrix_coefficients,
            codec_init,
        })
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = BoxBuf::new();
        b.u32(1 << 24);
        b.u8(self.profile);
        b.u8(self.level);
        b.u8((self.bit_depth << 4) | ((self.chroma_subsampling & 7) << 1) | self.full_range as u8);
        b.u8(self.colour_primaries);
        b.u8(self.transfer_characteristics);
        b.u8(self.matrix_coefficients);
        b.u16(self.codec_init.len() as u16);
        b.bytes(&self.codec_init);
        b.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hvcc_roundtrip() {
        let cfg = HevcConfig {
            general_profile_idc: 1,
            general_profile_compatibility_flags: 0x6000_0000,
            general_constraint_indicator_flags: 0x9000_0000_0000,
            general_level_idc: 93,
            chroma_format_idc: 1,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            length_size: 4,
            num_temporal_layers: 1,
            temporal_id_nested: true,
            arrays: vec![
                HevcNalArray { completeness: true, nal_type: 32, nalus: vec![vec![0x40, 1]] },
                HevcNalArray { completeness: true, nal_type: 33, nalus: vec![vec![0x42, 1, 2]] },
                HevcNalArray { completeness: true, nal_type: 34, nalus: vec![vec![0x44, 1]] },
            ],
            ..Default::default()
        };
        let back = HevcConfig::parse(&cfg.to_bytes()).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.sps(), vec![&[0x42u8, 1, 2][..]]);
    }
}

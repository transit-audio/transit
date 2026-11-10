//! Header-only probing for folder admission. Never constructs an audio decoder.

use crate::{AudioError, AudioInfo, FileFormat, Result, SampleFormat, aiff, wav};
use std::io::{Read, Seek, SeekFrom};

/// Probe WAV, AIFF or FLAC without reading sample bodies. Chunk payloads unrelated to the
/// format and length are skipped. The native parsers validate compacted PCM headers, using
/// the actual available data length; no allocation depends on a declared audio length.
pub fn probe_metadata(input: &mut (impl Read + Seek)) -> Result<AudioInfo> {
    let len = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 12];
    input.read_exact(&mut head)?;
    if &head[..4] == b"fLaC" {
        input.seek(SeekFrom::Start(4))?;
        return flac_info(input);
    }
    let aiff = aiff::is_aiff(&head);
    if !aiff && !wav::is_wav(&head) {
        return Err(AudioError::Unsupported("expected WAV, AIFF or FLAC audio".into()));
    }
    let mut compact = head.to_vec();
    let mut at = 12u64;
    let mut data = None;
    let mut rf64_data = None;
    for _ in 0..100_000 {
        if at.saturating_add(8) > len {
            break;
        }
        input.seek(SeekFrom::Start(at))?;
        let mut chunk = [0u8; 8];
        input.read_exact(&mut chunk)?;
        let size_bytes = [chunk[4], chunk[5], chunk[6], chunk[7]];
        let declared = u64::from(if aiff { u32::from_be_bytes(size_bytes) } else { u32::from_le_bytes(size_bytes) });
        let body = at + 8;
        let size = if &chunk[..4] == b"data" && declared == u64::from(u32::MAX) { rf64_data.unwrap_or(declared) } else { declared };
        let available = size.min(len.saturating_sub(body));
        let sound = (!aiff && &chunk[..4] == b"data") || (aiff && &chunk[..4] == b"SSND");
        if sound && data.is_none() {
            let mut prefix = Vec::new();
            if aiff {
                input.take(available.min(8)).read_to_end(&mut prefix)?;
            }
            data = Some((chunk, prefix, available));
        } else if matches!(&chunk[..4], b"fmt " | b"COMM" | b"FVER" | b"ds64" | b"fact") {
            if available != size || size > 64 * 1024 {
                return Err(AudioError::Malformed("invalid audio format header length".into()));
            }
            let mut bytes = Vec::new();
            input.take(size).read_to_end(&mut bytes)?;
            if &chunk[..4] == b"ds64" && bytes.len() >= 16 {
                let mut n = [0u8; 8];
                n.copy_from_slice(&bytes[8..16]);
                rf64_data = Some(u64::from_le_bytes(n));
            }
            compact.extend_from_slice(&chunk);
            compact.extend_from_slice(&bytes);
            if size & 1 != 0 {
                compact.push(0);
            }
            if compact.len() > 1024 * 1024 {
                return Err(AudioError::TooLarge("audio format headers exceed 1 MiB".into()));
            }
        }
        at = body.saturating_add(size).saturating_add(size & 1);
    }
    let (chunk, prefix, available) = data.ok_or_else(|| AudioError::Malformed("no audio data chunk".into()))?;
    compact.extend_from_slice(&chunk);
    let total = (compact.len() as u64).checked_add(available).ok_or_else(|| AudioError::TooLarge("audio extent overflow".into()))?;
    compact.extend_from_slice(&prefix);
    let total = usize::try_from(total).map_err(|_| AudioError::TooLarge("audio extent exceeds the address space".into()))?;
    if aiff {
        return match aiff::parse_sized(&compact, total)? {
            aiff::AiffParse::Native(l) => Ok(l.info),
            aiff::AiffParse::Foreign => Err(AudioError::Unsupported("this AIFF codec has no header-only preview".into())),
        };
    }
    match wav::parse_sized(&compact, total)? {
        wav::WavParse::Native(l) => Ok(l.info),
        wav::WavParse::Foreign { .. } => Err(AudioError::Unsupported("this WAV codec has no header-only preview".into())),
    }
}

fn flac_info(input: &mut impl Read) -> Result<AudioInfo> {
    let mut header = [0u8; 4];
    input.read_exact(&mut header)?;
    if header[0] & 0x7f != 0 || header[1..] != [0, 0, 34] {
        return Err(AudioError::Malformed("FLAC must begin with a 34-byte STREAMINFO block".into()));
    }
    let mut info = [0u8; 34];
    input.read_exact(&mut info)?;
    let mut packed = [0u8; 8];
    packed.copy_from_slice(&info[10..18]);
    let word = u64::from_be_bytes(packed);
    let rate = (word >> 44) as u32;
    let channels = ((word >> 41) & 7) as u16 + 1;
    let bits = ((word >> 36) & 31) as u8 + 1;
    let frames = word & 0xf_ffff_ffff;
    if rate == 0 || frames == 0 {
        return Err(AudioError::Malformed("FLAC header must declare a sample rate and frame count".into()));
    }
    let sample_format = match bits {
        1..=8 => SampleFormat::Int8,
        9..=16 => SampleFormat::Int16,
        17..=24 => SampleFormat::Int24,
        _ => SampleFormat::Int32,
    };
    Ok(AudioInfo { format: FileFormat::Flac, sample_format, sample_rate: rate, channels, frames, bwf: None })
}

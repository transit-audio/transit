//! Frozen Texture media uses the session's immutable Audio Files naming rule.

use super::*;
use transit_dsp::plugins::texture::freeze_state::Recording;
use transit_model::{FrozenRecording, Insert};

#[cfg(test)]
#[path = "frozen_recording_tests.rs"]
mod tests;

fn enabled(ins: &Insert) -> bool {
    ins.plugin == "texture" && ins.params.get("freeze").is_some_and(|v| *v >= 0.5)
}

fn matches(ins: &Insert, saved: &FrozenRecording, r: &Recording<'_>) -> bool {
    let quality = ins.params.get("quality").copied().unwrap_or(3.0).round().clamp(0.0, 3.0) as u32;
    let channels = if ins.params.get("mono_input").is_some_and(|v| *v >= 0.5) { 1 } else { 2 };
    let buffer = (4 + ins.params.get("buffer").copied().unwrap_or(0.0).round().clamp(0.0, 4.0) as u32) * 48_000;
    quality == saved.quality
        && channels == saved.channels
        && buffer == saved.buffer_frames
        && saved.quality == r.quality
        && saved.rate == r.rate
        && saved.channels == r.channels
        && saved.span == r.span
        && saved.buffer_frames == r.buffer_frames
}

/// A plain IEEE float WAV. Keeping this header fixed makes validation exact and bounded.
fn wav(r: &Recording<'_>) -> Vec<u8> {
    let size = r.pcm.len() as u32;
    let channels = r.channels as u16;
    let align = channels * 4;
    let mut out = Vec::with_capacity(44 + size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + size).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&r.rate.to_le_bytes());
    out.extend_from_slice(&(r.rate * u32::from(align)).to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(r.pcm);
    out
}

pub(super) fn save(s: &mut Session, dir: &Path) -> Result<usize> {
    let audio_dir = dir.join("Audio Files");
    let mut written = 0;
    store(s, &mut |stem, extension, bytes| {
        make_dir(&audio_dir)?;
        let (name, new) = super::session::store_media(&audio_dir, stem, extension, bytes)?;
        written += usize::from(new);
        Ok(format!("Audio Files/{name}"))
    })?;
    Ok(written)
}

pub(super) fn store(s: &mut Session, put: &mut impl FnMut(&str, &str, &[u8]) -> Result<String>) -> Result<()> {
    for track in &mut s.tracks {
        for (slot, ins) in track.mixer.inserts.iter_mut().enumerate().filter_map(|(slot, ins)| Some((slot, ins.as_mut()?))) {
            if ins.plugin != "texture" {
                continue;
            }
            if !enabled(ins) {
                // Like recorded takes, old media remains for backups and other session copies.
                ins.frozen_recording = None;
                continue;
            }
            let Some(saved) = &ins.frozen_recording else { continue };
            let Some(data) = &saved.data else { continue };
            let Some(r) = Recording::parse(data) else {
                return Err(EngineError::Io(format!("{}: Texture insert {} has invalid frozen state", track.name, slot + 1)));
            };
            if !matches(ins, saved, &r) {
                return Err(EngineError::Io(format!(
                    "{}: Texture insert {} changed quality or buffer; capture its state again",
                    track.name,
                    slot + 1
                )));
            }
            let stem = format!("{}_Texture_{}", sanitize_name(&track.name), slot + 1);
            let name = put(&stem, "wav", &wav(&r))?;
            let mut continuation = r.header.to_vec();
            continuation.extend_from_slice(r.continuation);
            let state_name = put(&stem, "texture-state", &continuation)?;
            if let Some(saved) = &mut ins.frozen_recording {
                saved.path = name;
                saved.continuation = state_name;
            }
        }
    }
    Ok(())
}

fn read(saved: &FrozenRecording, links: &Path) -> std::result::Result<Vec<u8>, String> {
    let audio = stored_path(&saved.path, links)?;
    let state = stored_path(&saved.continuation, links)?;
    // Bound reads before allocating; read_file also checks the automation roots and regular-file rule.
    let bounded = |p: &Path| {
        crate::file_access::check_read(p)?;
        let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > 9 * 1024 * 1024 {
            return Err("invalid frozen media size".into());
        }
        read_file(p).map_err(|e| e.to_string())
    };
    let audio = bounded(&audio)?;
    let state = bounded(&state)?;
    from_files(&audio, &state)
}

pub(super) fn from_files(audio: &[u8], state: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let pcm = audio.get(44..).ok_or("invalid float WAV")?;
    let data = Recording::from_files(pcm, state).ok_or("invalid frozen continuation")?;
    let r = Recording::parse(&data).ok_or("invalid frozen metadata")?;
    if audio != wav(&r) {
        return Err("frozen WAV format, rate, channels or span mismatch".into());
    }
    Ok(data)
}

pub(super) fn load(s: &mut Session, links: &Path) -> Vec<String> {
    load_with(s, &mut |saved| read(saved, links))
}

pub(super) fn load_with(s: &mut Session, read: &mut impl FnMut(&FrozenRecording) -> std::result::Result<Vec<u8>, String>) -> Vec<String> {
    let mut notes = Vec::new();
    for track in &mut s.tracks {
        for (slot, ins) in track.mixer.inserts.iter_mut().enumerate().filter_map(|(slot, ins)| Some((slot, ins.as_mut()?))) {
            if !enabled(ins) {
                continue;
            }
            let Some(saved) = &ins.frozen_recording else { continue };
            let loaded = read(saved).and_then(|data| {
                let r = Recording::parse(&data).ok_or("invalid frozen metadata")?;
                if !matches(ins, saved, &r) || r.host_rate != s.sample_rate.hz() as f32 {
                    return Err("quality, rate, channels or span mismatch".into());
                }
                // Validate every continuation field before the graph can see it.
                let mut device = transit_dsp::plugins::texture::TextureFx::new();
                if !transit_dsp::Plugin::load_state(&mut device, &data) {
                    return Err("invalid frozen continuation".into());
                }
                Ok(data)
            });
            match loaded {
                Ok(data) => {
                    if let Some(saved) = &mut ins.frozen_recording {
                        saved.data = Some(data.into());
                    }
                }
                Err(error) => {
                    notes.push(format!(
                        "track \"{}\", Texture insert {}: frozen recording unavailable ({error}); opens silent",
                        track.name,
                        slot + 1
                    ));
                    if let Some(saved) = &mut ins.frozen_recording {
                        saved.data = None;
                    }
                }
            }
        }
    }
    notes
}

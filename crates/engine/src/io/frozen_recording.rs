//! Frozen Texture media uses immutable, content-addressed files beside the session.

use super::*;
use transit_dsp::plugins::texture::freeze_state::Recording;
use transit_model::FrozenRecording;

#[cfg(test)]
#[path = "frozen_recording_tests.rs"]
mod tests;

fn matches(saved: &FrozenRecording, r: &Recording<'_>) -> bool {
    saved.quality == r.quality
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

// A name hint, not a trust boundary: store_media compares bytes before reusing a file,
// and allocates another immutable name on a hash collision.
fn content_stem(stem: &str, bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3));
    format!("{stem}_{hash:016x}")
}

pub(crate) fn save(s: &mut Session, dir: &Path) -> Result<usize> {
    // A missing reference must still identify the original file after Save Copy or autosave.
    if let Some(from) = &s.folder {
        for saved in s.tracks.iter_mut().flat_map(|t| t.mixer.inserts.iter_mut().flatten()).filter_map(|i| i.frozen_recording.as_mut()) {
            for path in [&mut saved.path, &mut saved.continuation] {
                if !path.is_empty() && Path::new(path).is_relative() {
                    *path = from.join(&*path).to_string_lossy().into_owned();
                }
            }
        }
    }
    let audio_dir = dir.join("Audio Files");
    let mut written = 0;
    store(s, &mut |stem, extension, bytes| {
        make_dir(&audio_dir)?;
        let (name, new) = super::session::store_media(&audio_dir, &content_stem(stem, bytes), extension, bytes)?;
        written += usize::from(new);
        Ok(format!("Audio Files/{name}"))
    })?;
    Ok(written)
}

/// Frozen media is best effort: it must never prevent saving the rest of a session.
/// Captures are validated against their own header, not base values overridden by automation.
pub(super) fn store(s: &mut Session, put: &mut impl FnMut(&str, &str, &[u8]) -> Result<String>) -> Result<()> {
    for track in &mut s.tracks {
        for (slot, ins) in track.mixer.inserts.iter_mut().enumerate().filter_map(|(slot, ins)| Some((slot, ins.as_mut()?))) {
            if ins.plugin != "texture" {
                continue;
            }
            // Presence is the live Freeze value captured by mix.insert_state. Base parameters
            // cannot override it (automation and rack macros need not equal their base).
            let Some(saved) = &mut ins.frozen_recording else { continue };
            let Some(data) = &saved.data else { continue };
            let stored: Result<_> = (|| {
                let r = Recording::parse(data).ok_or_else(|| EngineError::Io("invalid captured frozen header".into()))?;
                let stem = format!("{}_Texture_{}", sanitize_name(&track.name), slot + 1);
                let name = put(&stem, "wav", &wav(&r))?;
                let state_name = if r.continuation.is_empty() {
                    String::new()
                } else {
                    let mut continuation = r.header.to_vec();
                    continuation.extend_from_slice(r.continuation);
                    // WAV remains usable if the optional continuation cannot be written.
                    match put(&stem, "texture-state", &continuation) {
                        Ok(name) => name,
                        Err(error) => {
                            log::warn!("track {:?}, Texture insert {}: continuation not saved ({error}); WAV retained", track.name, slot + 1);
                            String::new()
                        }
                    }
                };
                Ok((name, state_name, r.wav_header(), r.quality, r.rate, r.channels, r.span, r.buffer_frames))
            })();
            match stored {
                Ok((path, continuation, header, quality, rate, channels, span, buffer_frames)) => {
                    saved.path = path;
                    saved.continuation = continuation;
                    saved.header = header;
                    saved.quality = quality;
                    saved.rate = rate;
                    saved.channels = channels;
                    saved.span = span;
                    saved.buffer_frames = buffer_frames;
                    saved.report.clear();
                }
                Err(error) => {
                    saved.report = format!(
                        "track {:?}, Texture insert {}: frozen recording not saved ({error}); previous reference retained",
                        track.name,
                        slot + 1
                    );
                    log::warn!("{}", saved.report);
                }
            }
        }
    }
    Ok(())
}

fn bounded(p: &Path) -> std::result::Result<Vec<u8>, String> {
    crate::file_access::check_read(p)?;
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > 9 * 1024 * 1024 {
        return Err("invalid frozen media size".into());
    }
    read_file(p).map_err(|e| e.to_string())
}

fn read(saved: &FrozenRecording, links: &Path) -> std::result::Result<Vec<u8>, String> {
    let audio = bounded(&stored_path(&saved.path, links)?)?;
    let state = stored_path(&saved.continuation, links).ok().and_then(|p| bounded(&p).ok()).unwrap_or_default();
    from_files(saved, &audio, &state)
}

pub(super) fn from_files(saved: &FrozenRecording, audio: &[u8], state: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let pcm = audio.get(44..).ok_or("invalid float WAV")?;
    // Prefer the independently saved header. Legacy files can obtain it from their companion.
    let full = Recording::from_files(pcm, state);
    let header = if saved.header.is_empty() {
        full.as_ref().and_then(|data| Recording::parse(data)).map(|r| r.wav_header()).ok_or("missing captured setup header")?
    } else {
        saved.header.clone()
    };
    let bare = Recording::join(&header, pcm, &[]).ok_or("invalid frozen metadata")?;
    let r = Recording::parse(&bare).ok_or("invalid frozen metadata")?;
    if !matches(saved, &r) || audio != wav(&r) {
        return Err("frozen WAV format, quality, rate, channels or span mismatch".into());
    }
    if let Some(data) = full {
        let consistent = Recording::parse(&data).is_some_and(|full| full.header.get(8..) == header.get(8..));
        let mut device = transit_dsp::plugins::texture::TextureFx::new();
        if consistent && transit_dsp::Plugin::load_state(&mut device, &data) {
            return Ok(data);
        }
    }
    Ok(bare)
}

pub(super) fn load(s: &mut Session, links: &Path) -> Vec<String> {
    load_with(s, &mut |saved| read(saved, links))
}

/// Older sessions kept their setup only in the companion and scalar metadata. Reconstruct
/// the recording-only header when that companion is gone, without depending on its version.
fn legacy_header(saved: &FrozenRecording, params: &std::collections::BTreeMap<String, f32>, rate: f32) -> Option<Vec<u8>> {
    use transit_dsp::Plugin;
    let mut device = transit_dsp::plugins::texture::TextureFx::new();
    for (id, value) in params {
        device.set_param(id, *value);
    }
    device.set_param("quality", saved.quality as f32);
    device.set_param("buffer", saved.buffer_frames as f32 / 48_000.0 - 4.0);
    device.set_param("mono_input", f32::from(saved.channels == 1));
    device.set_param("freeze", 1.0);
    device.prepare(rate, 512, 2);
    let packet = device.save_state()?;
    Some(Recording::parse(&packet)?.wav_header())
}

pub(super) fn load_with(s: &mut Session, read: &mut impl FnMut(&FrozenRecording) -> std::result::Result<Vec<u8>, String>) -> Vec<String> {
    let mut notes = Vec::new();
    for track in &mut s.tracks {
        for (slot, ins) in track.mixer.inserts.iter_mut().enumerate().filter_map(|(slot, ins)| Some((slot, ins.as_mut()?))) {
            if ins.plugin != "texture" {
                continue;
            }
            let Some(saved) = &mut ins.frozen_recording else { continue };
            let loaded = read(saved)
                .or_else(|error| {
                    if error == "missing captured setup header" && saved.header.is_empty() {
                        saved.header = legacy_header(saved, &ins.params, s.sample_rate.hz() as f32).ok_or("invalid legacy frozen setup")?;
                        read(saved)
                    } else {
                        Err(error)
                    }
                })
                .and_then(|data| {
                    let r = Recording::parse(&data).ok_or("invalid frozen metadata")?;
                    if !matches(saved, &r) || r.host_rate != s.sample_rate.hz() as f32 {
                        return Err("quality, rate, channels or span mismatch".into());
                    }
                    let mut device = transit_dsp::plugins::texture::TextureFx::new();
                    if !transit_dsp::Plugin::load_state(&mut device, &data) {
                        return Err("invalid frozen recording".into());
                    }
                    Ok(data)
                });
            match loaded {
                Ok(data) => {
                    if Recording::parse(&data).is_some_and(|r| r.continuation.is_empty()) {
                        notes.push(format!(
                            "track {:?}, Texture insert {}: frozen WAV restored; continuation not restored (missing or invalid companion)",
                            track.name,
                            slot + 1
                        ));
                    }
                    if let Some(r) = Recording::parse(&data) {
                        saved.header = r.wav_header();
                    }
                    saved.data = Some(data.into());
                    saved.restore_failed = false;
                    saved.report.clear();
                }
                Err(error) => {
                    saved.report =
                        format!("track {:?}, Texture insert {}: frozen recording unavailable ({error}); opens silent", track.name, slot + 1);
                    notes.push(saved.report.clone());
                    saved.data = None;
                    saved.restore_failed = true;
                }
            }
        }
    }
    notes
}

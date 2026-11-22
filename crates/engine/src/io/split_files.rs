//! The files of Split to Mono Tracks (`clip.split_to_mono`) for a take played from its file (ADR
//! 0035): one mono float WAV per channel written beside it, opened to play from disk, and removed
//! again when the split fails.

use super::*;
use std::io::Write;
use transit_audio_io::{BitDepth, FileEncoder};
use transit_model::disk::DiskSource;

/// The file `path`, written by [`write`], as a source played from disk at `rate`.
pub(crate) fn open(path: &Path, rate: u32) -> Result<Arc<SourceAudio>> {
    let len = std::fs::metadata(path).map_err(|err| EngineError::Io(format!("{}: {err}", path.display())))?.len();
    disk::open_disk_audio(path, len, rate).map_err(EngineError::Io)
}

/// Removes the files a failed split wrote, and any peaks sidecar begun beside one.
pub(crate) fn remove(paths: &[PathBuf]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(transit_model::disk::sidecar_path(p));
    }
}

/// One mono float WAV per channel of `disk`'s file, beside it, named `names` (`<name>.wav`, or
/// `<name>-2.wav` and on while that is taken), in one pass over the file. Each path is pushed to
/// `written` as soon as it is created.
pub(crate) fn write(disk: &DiskSource, names: &[String], written: &mut Vec<PathBuf>) -> Result<()> {
    let io = |p: &Path, err: &dyn std::fmt::Display| EngineError::Io(format!("{}: {err}", p.display()));
    // A reader that failed once reads nothing more: opened anew, as before an offline render, so a
    // take whose file is back splits again.
    disk.check().map_err(|why| io(&disk.path(), &why))?;
    let dir = disk.path().parent().map(Path::to_path_buf).unwrap_or_default();
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None };
    let mut encoders = Vec::with_capacity(names.len());
    for name in names {
        let stem = crate::io::sanitize_name(name);
        let mut found = None;
        for i in 1..1000 {
            let p = dir.join(if i == 1 { format!("{stem}.wav") } else { format!("{stem}-{i}.wav") });
            crate::file_access::check_write(&p).map_err(EngineError::Io)?;
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
                Ok(f) => {
                    found = Some((p, f));
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(io(&p, &err)),
            }
        }
        let (path, file) = found.ok_or_else(|| EngineError::Io(format!("{}: no file name free for {stem}.wav", dir.display())))?;
        written.push(path.clone());
        let enc = FileEncoder::new(std::io::BufWriter::new(file), &opts, 1, disk.rate(), None).map_err(|err| io(&path, &err))?;
        enc.check_room(disk.frames()).map_err(|err| io(&path, &err))?;
        encoders.push((path, enc));
    }
    const BLOCK: usize = 64 * 1024;
    let frames = usize::try_from(disk.frames()).unwrap_or(usize::MAX);
    let mut block = vec![vec![0.0f32; BLOCK]; encoders.len()];
    let unread = || EngineError::Io(format!("{}: the file could not be read", disk.path().display()));
    disk.with_sync(|read| {
        let mut at = 0;
        while at < frames {
            let len = BLOCK.min(frames - at);
            // Frame by frame across the channels, so the reader passes over the file once.
            for i in 0..len {
                for (ch, out) in block.iter_mut().enumerate() {
                    out[i] = read(ch, at + i).ok_or_else(unread)?;
                }
            }
            for ((path, enc), out) in encoders.iter_mut().zip(&block) {
                enc.write(std::slice::from_ref(out), len).map_err(|err| io(path, &err))?;
            }
            at += len;
        }
        Ok::<(), EngineError>(())
    })?;
    for (path, enc) in encoders {
        let mut w = enc.finish().map_err(|err| io(&path, &err))?;
        w.flush().map_err(|err| io(&path, &err))?;
        w.into_inner().map_err(|err| io(&path, &err.error()))?.sync_all().map_err(|err| io(&path, &err))?;
    }
    Ok(())
}

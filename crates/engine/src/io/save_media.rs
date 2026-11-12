//! A disk-backed source's file brought into the session folder by a save (#573): a long take
//! (ADR 0035) is played from its file, so a save that names it from another folder copies that
//! file in, byte for byte, before the session file is written.

use super::*;
use std::io::Read;

/// Bytes compared at a time: a take's file is never held in memory whole.
const CHUNK: usize = 1 << 20;

/// The path, relative to the session folder `dir`, of `from` when it already lives there: in `dir`
/// itself or in its `Audio Files`. A file anywhere else (another session's folder, the unsaved
/// recordings folder, an autosave folder, or deeper under `dir`, which may be a home folder holding
/// all of those) is not the session's own.
pub(super) fn in_session_folder(dir: &Path, from: &Path) -> Option<String> {
    let own = std::fs::canonicalize(from).ok()?;
    let name = own.file_name()?.to_str()?;
    let parent = own.parent()?;
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let audio = std::fs::canonicalize(dir.join("Audio Files")).ok();
    if audio.as_deref() == Some(parent) {
        Some(format!("Audio Files/{name}"))
    } else if std::fs::canonicalize(dir).ok().as_deref() == Some(parent) {
        Some(name.to_string())
    } else {
        None
    }
}

/// The name in `audio_dir` of a complete copy of `from`, the file of a disk-backed source of
/// `frames` frames and `channels` channels, and whether it was written now.
///
/// `from` itself, when it is already in `audio_dir`, or a file there holding exactly its bytes is
/// used as it is. Otherwise the first free `<stem>.<ext>`, `<stem>_01.<ext>`, … is reserved
/// (`create_new`, so no file is ever replaced), the file is copied to a temporary name beside it,
/// synced, checked (every byte copied, and its header reads back as the same audio) and renamed
/// over the reservation. Any failure removes what this call wrote and names the file.
pub(super) fn bring_into(audio_dir: &Path, from: &Path, frames: u64, channels: usize) -> Result<(String, bool)> {
    let fail = |why: &dyn std::fmt::Display| EngineError::Io(format!("{} could not be copied into {}: {why}", from.display(), audio_dir.display()));
    crate::file_access::check_read(from).map_err(|why| fail(&why))?;
    let len = std::fs::metadata(from).map_err(|err| fail(&err))?.len();
    if let (Ok(own), Ok(dir)) = (std::fs::canonicalize(from), std::fs::canonicalize(audio_dir))
        && own.parent() == Some(dir.as_path())
        && let Some(name) = own.file_name().and_then(|n| n.to_str())
    {
        return Ok((name.to_string(), false));
    }
    let stem = sanitize_name(from.file_stem().and_then(|x| x.to_str()).unwrap_or("Take"));
    let extension = from.extension().and_then(|x| x.to_str()).filter(|x| !x.is_empty()).unwrap_or("wav");
    for k in 0..10_000u32 {
        let name = if k == 0 { format!("{stem}.{extension}") } else { format!("{stem}_{k:02}.{extension}") };
        let path = audio_dir.join(&name);
        crate::file_access::check_write(&path).map_err(|why| fail(&why))?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(reserved) => {
                drop(reserved);
                let tmp = audio_dir.join(format!("{name}.copying"));
                let copied = crate::file_access::check_write(&tmp)
                    .map_err(|why| why.to_string())
                    .and_then(|()| copy_checked(from, &tmp, len, frames, channels))
                    .and_then(|()| std::fs::rename(&tmp, &path).map_err(|err| err.to_string()));
                if let Err(why) = copied {
                    let _ = std::fs::remove_file(&tmp);
                    let _ = std::fs::remove_file(&path);
                    return Err(fail(&why));
                }
                return Ok((name, true));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if same_bytes(from, &path, len) {
                    return Ok((name, false));
                }
            }
            Err(err) => return Err(fail(&format!("{}: {err}", path.display()))),
        }
    }
    Err(fail(&"no free file name"))
}

/// Copy `from` (`len` bytes) to `to` and sync it; an error unless every byte arrived and the copy's
/// header says it holds `frames` frames of `channels` channels, as the source does.
fn copy_checked(from: &Path, to: &Path, len: u64, frames: u64, channels: usize) -> std::result::Result<(), String> {
    let mut src = std::fs::File::open(from).map_err(|err| err.to_string())?;
    let mut dst = std::fs::File::create(to).map_err(|err| format!("{}: {err}", to.display()))?;
    // `io::copy` streams through a small buffer (or the system's own file copy).
    let copied = std::io::copy(&mut src, &mut dst).map_err(|err| format!("{}: {err}", to.display()))?;
    dst.sync_all().map_err(|err| format!("{}: {err}", to.display()))?;
    drop(dst);
    let on_disk = std::fs::metadata(to).map_err(|err| format!("{}: {err}", to.display()))?.len();
    if copied != len || on_disk != len {
        return Err(format!("{on_disk} of {len} bytes were copied"));
    }
    match super::disk::probe(to) {
        Some((info, _)) if info.frames == frames && usize::from(info.channels) == channels => Ok(()),
        Some((info, _)) => {
            Err(format!("the copy reads as {} frames of {} channels, the source has {frames} of {channels}", info.frames, info.channels))
        }
        None => Err("the copy's header cannot be read back".into()),
    }
}

/// True when the file at `other` holds exactly the `len` bytes of `from`, compared a chunk at a time.
fn same_bytes(from: &Path, other: &Path, len: u64) -> bool {
    if crate::file_access::check_read(other).is_err() || !std::fs::metadata(other).is_ok_and(|m| m.is_file() && m.len() == len) {
        return false;
    }
    let (Ok(mut a), Ok(mut b)) = (std::fs::File::open(from), std::fs::File::open(other)) else { return false };
    let (mut x, mut y) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    loop {
        let n = match fill(&mut a, &mut x) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if !fill(&mut b, &mut y).is_ok_and(|m| m == n) || x[..n] != y[..n] {
            return false;
        }
        if n < CHUNK {
            return true;
        }
    }
}

/// Read into `buf` until it is full or the file ends; how many bytes were read.
fn fill(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(n)
}

#[cfg(test)]
#[path = "save_media_tests.rs"]
mod tests;

//! What an offline render reads from streamed takes, checked before and after it (issue #523): a
//! take whose file cannot be opened or read must never render as silence without a word.

use super::*;
use transit_model::disk::DiskSource;

/// The streamed takes of a session with each one's failure count when an offline render began, so
/// [`DiskReads::failed`] can tell which ones a read failed in since; the takes the render plays are
/// also opened anew first ([`DiskReads::check`]).
pub struct DiskReads {
    reads: Vec<Read>,
}

struct Read {
    disk: Arc<DiskSource>,
    name: String,
    mark: u64,
}

/// The line naming a streamed take that cannot be read, and why.
pub fn unreadable_line(name: &str, path: &Path, why: &str) -> String {
    format!("`{name}` cannot be read from {}: {why}", path.display())
}

impl DiskReads {
    /// Before an offline render of `tracks` (every track when `None`) over `r`: the streamed
    /// takes of the unmuted clips it plays on active, unfrozen tracks, each opened anew
    /// ([`DiskSource::check`], so a file that is back renders again), and a line for each one that
    /// cannot be opened. Every other streamed take in the pool is watched too (not opened), so a
    /// take the render reads another way (a bus or aux fed by another track) is still caught
    /// afterwards. Reads no audio; a session without streamed takes costs nothing.
    pub fn check(s: &Session, tracks: Option<&[TrackId]>, r: Range) -> (DiskReads, Vec<String>) {
        let mut reads: Vec<Read> = Vec::new();
        let mut lines = Vec::new();
        if !s.pool.has_disk() {
            return (DiskReads { reads }, lines);
        }
        let picked = s.tracks.iter().filter(|t| tracks.is_none_or(|ids| ids.contains(&t.id)));
        for t in picked.filter(|t| !t.inactive && t.freeze.is_none()) {
            for clip in t.clips().iter().filter(|c| !c.muted && c.range().overlaps(&r)) {
                let Some(id) = clip.source() else { continue };
                let Some(disk) = s.pool.get(id).and_then(|a| a.disk.clone()) else { continue };
                if reads.iter().any(|x| Arc::ptr_eq(&x.disk, &disk)) {
                    continue;
                }
                let name = s.sources.iter().find(|m| m.id == id).map_or_else(|| clip.name.clone(), |m| m.name.clone());
                if let Err(why) = disk.check() {
                    lines.push(unreadable_line(&name, &disk.path(), &why));
                }
                reads.push(Read { mark: disk.sync_failures(), disk, name });
            }
        }
        for id in s.pool.ids() {
            let Some(disk) = s.pool.get(id).and_then(|a| a.disk.clone()) else { continue };
            if reads.iter().any(|x| Arc::ptr_eq(&x.disk, &disk)) {
                continue;
            }
            let name = s.sources.iter().find(|m| m.id == id).map_or_else(|| format!("source {}", id.0), |m| m.name.clone());
            reads.push(Read { mark: disk.sync_failures(), disk, name });
        }
        (DiskReads { reads }, lines)
    }

    /// After the render: a line for each take a read failed in since [`DiskReads::check`] (a
    /// file that went away, or a drive that dropped out, mid-render).
    pub fn failed(&self) -> Vec<String> {
        self.reads
            .iter()
            .filter(|x| x.disk.sync_failures() != x.mark)
            .map(|x| unreadable_line(&x.name, &x.disk.path(), x.disk.fault().as_deref().unwrap_or("a read failed during the render")))
            .collect()
    }
}

impl DiskReads {
    /// [`DiskReads::check`] as an offline render's first step: `cmd` is refused, naming every
    /// streamed take it plays that cannot be opened, before anything renders or is written.
    pub(crate) fn refuse(s: &Session, cmd: &str, tracks: Option<&[TrackId]>, r: Range) -> Result<DiskReads> {
        let (reads, lines) = DiskReads::check(s, tracks, r);
        if lines.is_empty() { Ok(reads) } else { Err(refusal(cmd, &lines)) }
    }

    /// The pre-flight of a render that reads one streamed take `disk` (named `name`) directly.
    pub(crate) fn refuse_one(cmd: &str, name: &str, disk: &Arc<DiskSource>) -> Result<DiskReads> {
        if let Err(why) = disk.check() {
            return Err(refusal(cmd, &[unreadable_line(name, &disk.path(), &why)]));
        }
        Ok(DiskReads { reads: vec![Read { mark: disk.sync_failures(), disk: Arc::clone(disk), name: name.to_string() }] })
    }

    /// [`DiskReads::failed`] as an offline render's last step, before its result is kept: `cmd`
    /// is refused, and the render discarded, when a read failed in any take it played.
    pub(crate) fn verify(&self, cmd: &str) -> Result<()> {
        let lines = self.failed();
        if lines.is_empty() { Ok(()) } else { Err(refusal(cmd, &lines)) }
    }
}

/// The refusal of an offline render over streamed takes that cannot be read: it would print them
/// as silence without a word (issue #523).
fn refusal(cmd: &str, lines: &[String]) -> EngineError {
    crate::cmd::bad(cmd, format!("{}. Nothing was rendered: reconnect the drive or restore the file, then try again", lines.join("; ")))
}

#[cfg(test)]
#[path = "disk_reads_tests.rs"]
mod tests;

//! What an offline render reads from streamed takes, checked before and after it (issue #523): a
//! take whose file cannot be opened or read must never render as silence without a word.

use super::*;
use transit_model::disk::DiskSource;

/// The streamed takes an offline render reads, each opened anew by [`DiskReads::check`], with its
/// failure count then, so [`DiskReads::failed`] can tell which ones a read failed in since.
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
    /// cannot be opened. Reads no audio; a session without streamed takes costs nothing.
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
                    lines.push(unreadable_line(&name, disk.path(), &why));
                }
                reads.push(Read { mark: disk.sync_failures(), disk, name });
            }
        }
        (DiskReads { reads }, lines)
    }

    /// After the render: a line for each take a read failed in since [`DiskReads::check`] (a
    /// file that went away, or a drive that dropped out, mid-render).
    pub fn failed(&self) -> Vec<String> {
        self.reads
            .iter()
            .filter(|x| x.disk.sync_failures() != x.mark)
            .map(|x| unreadable_line(&x.name, x.disk.path(), x.disk.fault().as_deref().unwrap_or("a read failed during the render")))
            .collect()
    }
}

#[cfg(test)]
#[path = "disk_reads_tests.rs"]
mod tests;

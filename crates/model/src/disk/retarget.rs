//! A disk-backed source moved onto a copy of its file (#629): a save that copies a long take into
//! the session's folder has the open session read that copy from then on, so playback, offline
//! renders and later saves no longer need the folder the take came from.
//!
//! The copy holds the same bytes, so what the rings already hold stays valid: a stream that is
//! playing or priming goes on without a gap, and the streamer's next pass reads on from the copy.
//! The audio thread is not involved (it only drains rings). Every reader of the old file is let
//! go before [`DiskSource::retarget`] returns, so the old folder can be deleted, moved or ejected.

use super::{DiskSource, Opener, PoisonError, Target, block_opener, peaks, stream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl DiskSource {
    /// Control side: read `path`, the native PCM file of `len` bytes a save copied this source's
    /// file to, from now on ([`DiskSource::retarget`]). `Err`, and nothing changed, when the copy
    /// cannot be opened or its header does not describe the same audio (frames, channels, rate).
    pub fn retarget_file(self: &Arc<Self>, path: &Path, len: u64) -> Result<(), String> {
        let (opener, info) = block_opener(path, len)?;
        if info.frames != self.frames || usize::from(info.channels) != self.channels || info.sample_rate != self.rate {
            return Err(format!(
                "{}: holds {} frames of {} channels at {} Hz, the source {} of {} at {} Hz",
                path.display(),
                info.frames,
                info.channels,
                info.sample_rate,
                self.frames,
                self.channels,
                self.rate
            ));
        }
        self.retarget(path.to_path_buf(), opener);
        Ok(())
    }

    /// Control side: read the file at `path`, through `opener`, from now on. The caller vouches
    /// that it holds the same audio as the file read so far (a checked copy).
    ///
    /// The streamer's and the synchronous reader's readers of the old file are dropped (each opens
    /// the new file on its next read); rings keep what they hold, unless the old file had failed
    /// and they hold silence, in which case they are freed as a prime after a failure frees them.
    /// Recorded faults are cleared. The peaks build of the old file is stopped and waited for (at
    /// most one read); peaks already built are written to the new file's sidecar, else built
    /// from the new file when the source is streamed ([`super::register`]). Never on the audio
    /// thread: it locks, may write a sidecar and may wait for a read.
    pub fn retarget(self: &Arc<Self>, path: PathBuf, opener: Opener) {
        if self.path() == path {
            return;
        }
        {
            let mut p = self.pump.lock().unwrap_or_else(PoisonError::into_inner);
            let mut sync = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
            *self.target.lock().unwrap_or_else(PoisonError::into_inner) = Target { path: path.clone(), opener };
            *sync = None;
            self.drop_reader(&mut p);
            *self.faults.lock().unwrap_or_else(PoisonError::into_inner) = [None, None];
        }
        let fresh = Arc::new(peaks::PeaksGate::default());
        let old = std::mem::replace(&mut *self.peaks_gate.lock().unwrap_or_else(PoisonError::into_inner), Arc::clone(&fresh));
        old.close_and_release();
        if let Some(built) = self.peaks() {
            if let Err(e) = peaks::save_sidecar(&path, built, &fresh) {
                log::info!("{}: the peaks sidecar was not written: {e}", path.display());
            }
        } else if self.registered.load(Ordering::Acquire) {
            stream::spawn_peaks(self);
        }
    }
}

#[cfg(test)]
#[path = "retarget_tests.rs"]
mod tests;

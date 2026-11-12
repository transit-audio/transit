//! Consolidated export on a worker: the UI polls progress and never joins a stuck render.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use transit_engine::io::{ConsolidatedBounce, ConsolidatedBounced};

pub struct ConsolidatedJob {
    dir: PathBuf,
    files: Vec<PathBuf>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<ConsolidatedBounced, String>>,
    #[cfg(test)]
    completion: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

fn stopped_with(job: &ConsolidatedBounce, reason: String) -> String {
    job.report_stopped(&reason)
}

impl ConsolidatedJob {
    pub fn new(job: ConsolidatedBounce) -> Result<Self, String> {
        let dir = job.dir().to_path_buf();
        let files = job.files();
        Self::spawn(dir, files, move |cancel, progress| {
            let mut job = job;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped_with(&job, "Bounce cancelled".into()));
                }
                let until = std::time::Instant::now() + std::time::Duration::from_millis(30);
                match job.step(|| !cancel.load(Ordering::Relaxed) && std::time::Instant::now() < until) {
                    Ok(Some(done)) => return Ok(done),
                    Ok(None) => progress.store(job.progress().to_bits(), Ordering::Relaxed),
                    Err(e) => return Err(stopped_with(&job, e.to_string())),
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped_with(&job, "Bounce cancelled".into()));
                }
            }
        })
    }

    fn spawn(
        dir: PathBuf,
        files: Vec<PathBuf>,
        task: impl FnOnce(Arc<AtomicBool>, Arc<AtomicU32>) -> Result<ConsolidatedBounced, String> + Send + 'static,
    ) -> Result<Self, String> {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let completion = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let (send, result) = std::sync::mpsc::channel();
        let task_cancel = Arc::clone(&cancel);
        let task_progress = Arc::clone(&progress);
        let task_completion = Arc::clone(&completion);
        std::thread::Builder::new()
            .name("consolidated-bounce".into())
            .spawn(move || {
                let done = task(task_cancel, task_progress);
                let _ = send.send(done);
                if let Ok(mut complete) = task_completion.0.lock() {
                    *complete = true;
                    task_completion.1.notify_all();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            dir,
            files,
            progress,
            cancel,
            result,
            #[cfg(test)]
            completion,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Option<Result<ConsolidatedBounced, String>> {
        match self.result.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("consolidated export worker stopped without a result".into())),
        }
    }
}

impl Drop for ConsolidatedJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
impl ConsolidatedJob {
    fn with_task(task: impl FnOnce(Arc<AtomicBool>, Arc<AtomicU32>) -> Result<ConsolidatedBounced, String> + Send + 'static) -> Result<Self, String> {
        Self::spawn(PathBuf::new(), Vec::new(), task)
    }

    pub fn wait_for_completion(&self) {
        let mut complete = self.completion.0.lock().unwrap();
        while !*complete {
            complete = self.completion.1.wait(complete).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_render_does_not_block_cancel() {
        let (started_send, started) = std::sync::mpsc::channel();
        let (release_send, release) = std::sync::mpsc::channel();
        let (finished_send, finished) = std::sync::mpsc::channel();
        let job = ConsolidatedJob::with_task(move |_, _| {
            started_send.send(()).unwrap();
            release.recv().unwrap();
            finished_send.send(()).unwrap();
            Err("stopped by the test".into())
        })
        .unwrap();
        let mut app = crate::tests::app();
        app.finish.consolidated_job = Some(job);
        started.recv().unwrap();
        app.cancel_bounce();
        assert_eq!(app.status, "Bounce cancelled");
        assert!(app.finish.consolidated_job.is_some(), "the worker stays registered until it exits, preventing an overlapping restart");
        release_send.send(()).unwrap();
        finished.recv().unwrap();
        app.finish.consolidated_job.as_ref().unwrap().wait_for_completion();
        crate::finish::step_bounce(&mut app, &egui::Context::default());
        assert!(app.finish.consolidated_job.is_none());
    }

    #[test]
    fn the_real_worker_stops_after_the_current_block_and_leaves_no_files() {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("transit-consolidated-cancel-{}-{stamp}-{serial}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut session = transit_model::Session::default();
        let track = session.add_track(transit_model::TrackKind::Instrument, transit_model::ChannelFormat::Stereo, Some("Long"));
        let clip = session.new_clip_id();
        session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(
            clip,
            "hour",
            0,
            48_000 * 3600,
            Default::default(),
        ));
        let bounce = ConsolidatedBounce::new(Arc::new(session), &dir, &transit_audio_io::EncodeOptions::default(), false).unwrap();
        let job = ConsolidatedJob::new(bounce).unwrap();
        job.cancel();
        job.wait_for_completion();
        assert!(matches!(job.poll(), Some(Err(e)) if e == "Bounce cancelled"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no final or part files");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

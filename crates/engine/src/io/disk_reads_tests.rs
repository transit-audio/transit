//! A streamed take whose file cannot be opened or read is named before and after an offline render
//! (issue #523). Fake files only: no real file or device is touched.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use transit_model::disk::{DiskFile, Opener};
use transit_model::{ClipId, TrackKind};

const FRAMES: usize = 48_000;

/// A file held in memory whose open fails while `gone` is set and whose reads fail while
/// `failing` is set.
struct Fake {
    data: Arc<Vec<f32>>,
    failing: Arc<AtomicBool>,
}

impl DiskFile for Fake {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        if self.failing.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("device not ready"));
        }
        let at = usize::try_from(at).unwrap();
        out.resize_with(1, Vec::new);
        out[0].clear();
        out[0].extend_from_slice(self.data.get(at..self.data.len().min(at + frames)).unwrap_or_default());
        Ok(out[0].len())
    }
}

struct Rig {
    s: Session,
    t: TrackId,
    gone: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
}

/// A track playing one clip of a one-second streamed take named "Take 3" at `lost.wav`.
fn rig() -> Rig {
    let (gone, failing) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let data: Arc<Vec<f32>> = Arc::new((0..FRAMES).map(|i| (i % 100) as f32 / 100.0 + 0.01).collect());
    let (g, f) = (Arc::clone(&gone), Arc::clone(&failing));
    let opener: Opener = Arc::new(move || {
        if g.load(Ordering::Relaxed) {
            return Err("lost.wav: gone".into());
        }
        Ok(Box::new(Fake { data: Arc::clone(&data), failing: Arc::clone(&f) }) as Box<dyn DiskFile>)
    });
    let disk = DiskSource::with_window(PathBuf::from("lost.wav"), 1, FRAMES as u64, 48_000, 4_096, opener);
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let src = add_disk_source(&mut s, "Take 3.wav", Arc::new(SourceAudio::disk(disk)), "lost.wav");
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(ClipId(10), "take", src, 0, 0, FRAMES as i64));
    Rig { s, t, gone, failing }
}

fn all() -> Range {
    Range::new(0, FRAMES as i64)
}

fn silent(out: &[Vec<f32>]) -> bool {
    out.iter().flatten().all(|v| *v == 0.0)
}

#[test]
fn a_take_that_cannot_be_opened_is_named_before_the_render_and_a_later_good_open_clears_it() {
    let r = rig();
    r.gone.store(true, Ordering::Relaxed);
    let (reads, lines) = DiskReads::check(&r.s, None, all());
    assert_eq!(lines, vec!["`Take 3` cannot be read from lost.wav: could not be opened: gone".to_string()]);
    assert!(silent(&transit_mix::render_range(&r.s, all(), 1024)), "what the render would print");
    assert_eq!(reads.failed(), lines, "and the render met it again");
    r.gone.store(false, Ordering::Relaxed);
    let (reads, lines) = DiskReads::check(&r.s, Some(&[r.t]), all());
    assert!(lines.is_empty(), "{lines:?}");
    assert!(!silent(&transit_mix::render_range(&r.s, all(), 1024)));
    assert!(reads.failed().is_empty());
}

#[test]
fn a_read_that_fails_during_the_render_is_named_after_it() {
    let r = rig();
    let (reads, lines) = DiskReads::check(&r.s, None, all());
    assert!(lines.is_empty());
    r.failing.store(true, Ordering::Relaxed);
    transit_mix::render_range(&r.s, all(), 1024);
    assert_eq!(reads.failed(), vec!["`Take 3` cannot be read from lost.wav: reading failed: device not ready".to_string()]);
}

#[test]
fn only_takes_the_render_plays_are_checked() {
    let mut r = rig();
    r.gone.store(true, Ordering::Relaxed);
    assert!(DiskReads::check(&r.s, None, Range::new(FRAMES as i64, 2 * FRAMES as i64)).1.is_empty(), "outside the range");
    let other = r.s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    assert!(DiskReads::check(&r.s, Some(&[other]), all()).1.is_empty(), "another track");
    r.s.track_mut(r.t).unwrap().playlist_mut().unwrap().clips[0].muted = true;
    assert!(DiskReads::check(&r.s, None, all()).1.is_empty(), "a muted clip");
    r.s.track_mut(r.t).unwrap().playlist_mut().unwrap().clips[0].muted = false;
    r.s.track_mut(r.t).unwrap().inactive = true;
    assert!(DiskReads::check(&r.s, None, all()).1.is_empty(), "an inactive track");
}

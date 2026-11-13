//! A streamed take whose file cannot be opened, or fails to read mid-render, refuses every offline
//! render that plays it, naming the take, its file and why, and leaves the session and the disk as
//! they were (issue #523). The take is a fake file: nothing real is opened, and files are written
//! only under a temporary folder.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use transit_engine::Engine;
use transit_model::disk::{DiskFile, DiskSource, Opener};
use transit_model::{ChannelFormat, Clip, ClipId, SourceAudio, TrackId, TrackKind};

const FRAMES: usize = 48_000;

/// A take held in memory: its open fails while `gone` is set, its reads while `failing` is.
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
    e: Engine,
    t: TrackId,
    clip: ClipId,
    gone: Arc<AtomicBool>,
    failing: Arc<AtomicBool>,
    /// Opens tried, failed or not.
    opens: Arc<AtomicUsize>,
    dir: transit_test_support::TestDir,
}

/// An Elastic Audio track playing one clip of a one-second streamed take "Take 3" at `lost.wav`.
fn rig(tag: &str) -> Rig {
    let (gone, failing) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let data: Arc<Vec<f32>> = Arc::new((0..FRAMES).map(|i| if i % 4_800 < 200 { 0.8 } else { 0.05 }).collect());
    let opens = Arc::new(AtomicUsize::new(0));
    let (g, f, n) = (Arc::clone(&gone), Arc::clone(&failing), Arc::clone(&opens));
    let opener: Opener = Arc::new(move || {
        n.fetch_add(1, Ordering::Relaxed);
        if g.load(Ordering::Relaxed) {
            return Err("lost.wav: gone".into());
        }
        Ok(Box::new(Fake { data: Arc::clone(&data), failing: Arc::clone(&f) }) as Box<dyn DiskFile>)
    });
    let disk = DiskSource::with_window(PathBuf::from("lost.wav"), 1, FRAMES as u64, 48_000, 4_096, opener);
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let src = transit_engine::io::add_disk_source(s, "Take 3.wav", Arc::new(SourceAudio::disk(disk)), "lost.wav");
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "take", src, 0, 0, FRAMES as i64));
    e.execute("track.elastic", &json!({"track": t.0, "algorithm": "polyphonic"})).unwrap();
    let dir = transit_test_support::TestDir::new(&format!("unreadable-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Rig { e, t, clip, gone, failing, opens, dir }
}

/// Every offline render that plays the take, as a command and its parameters.
fn renders(r: &Rig) -> Vec<(&'static str, Value)> {
    let (t, c, end) = (r.t.0, r.clip.0, FRAMES as i64);
    let out = |name: &str| r.dir.join(name).to_string_lossy().into_owned();
    vec![
        ("file.bounce_mix", json!({"path": out("Mix.wav"), "start": 0, "end": end})),
        ("file.bounce_mix", json!({"path": out("Mix normalized.wav"), "start": 0, "end": end, "normalize": true})),
        ("file.bounce_stems", json!({"dir": out("stems"), "start": 0, "end": end})),
        ("file.bounce_paths", json!({"dir": out("paths"), "paths": ["main"], "start": 0, "end": end})),
        ("file.export_clips", json!({"dir": out("clips"), "clips": [c]})),
        ("track.bounce", json!({"tracks": [t]})),
        ("track.commit", json!({"tracks": [t]})),
        ("clip.bounce", json!({"clips": [c]})),
        ("track.freeze", json!({"tracks": [t]})),
        ("edit.consolidate", json!({"tracks": [t], "start": 0, "end": end})),
        ("clip.gain_render", json!({"clips": [c]})),
        ("audiosuite.process", json!({"process": "reverse", "clips": [c]})),
        ("edit.copy_audio_as_midi", json!({"track": t, "start": 0, "end": end})),
        ("edit.strip_silence", json!({"tracks": [t], "start": 0, "end": end})),
        ("edit.separate_at_transients", json!({"tracks": [t], "start": 0, "end": end})),
        ("event.beat_detective", json!({"tracks": [t], "start": 0, "end": end})),
        ("clip.elastic_properties", json!({"clips": [c], "ratio": 0.5})),
    ]
}

/// The files under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() { out.extend(files(&p)) } else { out.push(p) }
    }
    out
}

/// Each render is refused with a message naming the take, its file and `why`, and leaves the
/// session and the folder untouched. With `opens`, it tried to open the file exactly that many
/// times: refused before rendering, not after a render that read silence block by block.
fn all_refused(r: &mut Rig, why: &str, opens: Option<usize>) {
    for (cmd, p) in renders(r) {
        let before = r.e.session_arc();
        let opened = r.opens.load(Ordering::Relaxed);
        let err = match r.e.execute(cmd, &p) {
            Ok(v) => panic!("{cmd} rendered the unreadable take as silence: {v}"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("`Take 3` cannot be read from lost.wav") && err.contains(why), "{cmd}: {err}");
        assert_eq!(*before, *r.e.session(), "{cmd} changed the session");
        assert_eq!(files(&r.dir), Vec::<PathBuf>::new(), "{cmd} left a file");
        if let Some(n) = opens {
            assert_eq!(r.opens.load(Ordering::Relaxed) - opened, n, "{cmd} was not refused before it rendered");
        }
    }
}

#[test]
fn every_offline_render_refuses_a_take_whose_file_cannot_be_opened_and_renders_it_once_it_is_back() {
    let mut r = rig("open");
    r.gone.store(true, Ordering::Relaxed);
    all_refused(&mut r, "could not be opened: gone", Some(1));
    // The drive is back: the next try opens the file again, with no other step.
    r.gone.store(false, Ordering::Relaxed);
    let path = r.dir.join("Mix.wav");
    let done = r.e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "start": 0, "end": FRAMES as i64})).unwrap();
    assert!(done["peak_db"].as_f64().unwrap() > -10.0, "{done}");
    r.e.execute("edit.consolidate", &json!({"tracks": [r.t.0], "start": 0, "end": FRAMES as i64})).unwrap();
}

#[test]
fn every_offline_render_refuses_a_take_whose_reads_fail_mid_render_and_discards_what_it_rendered() {
    let mut r = rig("read");
    r.failing.store(true, Ordering::Relaxed);
    all_refused(&mut r, "reading failed: device not ready", None);
}

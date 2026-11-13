//! Disk-backed sources, block-driven: the tests call [`DiskSource::pump`] themselves in place of
//! the streaming thread, so nothing depends on a clock or a scheduler (E49, E62).

use super::*;
use std::sync::atomic::AtomicUsize;

/// A file held in memory, counting how many readers were opened.
struct Fake {
    data: Arc<Vec<Vec<f32>>>,
}

impl DiskFile for Fake {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        out.resize_with(self.data.len(), Vec::new);
        let at = usize::try_from(at).unwrap_or(usize::MAX);
        let mut n = 0;
        for (o, d) in out.iter_mut().zip(self.data.iter()) {
            o.clear();
            o.extend_from_slice(d.get(at..d.len().min(at.saturating_add(frames))).unwrap_or_default());
            n = o.len();
        }
        Ok(n)
    }
}

fn ramp(channels: usize, frames: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| (0..frames).map(|i| c as f32 * 10.0 + i as f32 / frames as f32).collect()).collect()
}

fn fake(data: Vec<Vec<f32>>, window: usize) -> (Arc<DiskSource>, Arc<AtomicUsize>) {
    let opened = Arc::new(AtomicUsize::new(0));
    let frames = data.first().map_or(0, Vec::len) as u64;
    let channels = data.len();
    let data = Arc::new(data);
    let count = Arc::clone(&opened);
    let opener: Opener = Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(Fake { data: Arc::clone(&data) }) as Box<dyn DiskFile>)
    });
    (DiskSource::with_window(PathBuf::from("fake.wav"), channels, frames, 48_000, window, opener), opened)
}

fn map(clip: u64, length: i64) -> StreamMap {
    StreamMap { clip, offset: 0, looping: None, stretch: 1.0, length }
}

#[test]
fn a_ring_is_empty_until_the_streamer_fills_it_and_a_miss_asks_for_it() {
    let data = ramp(2, 10_000);
    let (d, _) = fake(data.clone(), 1_000);
    let m = map(7, 10_000);
    assert!(d.stream(&m, 0, 64).is_none());
    d.underrun(&m, 0);
    assert_eq!(d.underruns(), 1);
    assert!(d.stream(&m, 0, 64).is_none(), "nothing is read until the streamer runs");
    assert_eq!(d.pump(), 1_000, "one pass fills one window");
    let v = d.stream(&m, 0, 64).expect("filled");
    for k in 0..64 {
        assert_eq!(v.get(0, k), data[0][k as usize]);
        assert_eq!(v.get(1, k), data[1][k as usize]);
    }
}

#[test]
fn the_streamer_keeps_a_window_ahead_of_the_reader_and_never_overwrites_what_is_unread() {
    let data = ramp(1, 10_000);
    let (d, _) = fake(data.clone(), 256);
    let m = map(1, 10_000);
    d.cue(&m, 0);
    d.pump();
    let mut k = 0;
    while k + 64 <= 10_000 {
        let v = d.stream(&m, k, 64).unwrap_or_else(|| panic!("ring short at {k}"));
        for i in k..k + 64 {
            assert_eq!(v.get(0, i), data[0][i as usize], "clip time {i}");
        }
        drop(v);
        k += 64;
        // A block-driven streamer: one pass per block.
        d.pump();
    }
    assert_eq!(d.underruns(), 0);
}

#[test]
fn a_locate_is_a_new_stream_and_reading_behind_the_reader_is_a_miss() {
    let (d, _) = fake(ramp(1, 10_000), 512);
    let m = map(3, 10_000);
    d.cue(&m, 5_000);
    d.pump();
    assert!(d.stream(&m, 5_000, 64).is_some());
    assert!(d.stream(&m, 4_000, 64).is_none(), "behind the stream");
    d.underrun(&m, 4_000);
    d.pump();
    assert!(d.stream(&m, 4_000, 64).is_some(), "re-cued");
}

#[test]
fn looped_and_stretched_clips_stream_what_the_in_memory_read_gives() {
    let data = ramp(1, 4_000);
    let (d, _) = fake(data.clone(), 2_048);
    let maps = [
        StreamMap { clip: 1, offset: 100, looping: Some(ClipLoop { start: 200, end: 1_000 }), stretch: 1.0, length: 3_000 },
        StreamMap { clip: 2, offset: 7, looping: None, stretch: 1.5, length: 3_000 },
        StreamMap { clip: 3, offset: 0, looping: Some(ClipLoop { start: -50, end: 700 }), stretch: 0.75, length: 3_000 },
    ];
    for m in maps {
        let mut k = 0;
        d.cue(&m, 0);
        d.pump();
        while k < 3_000 {
            let n = 100.min(3_000 - k) as usize;
            let v = d.stream(&m, k, n).unwrap_or_else(|| panic!("{m:?} short at {k}"));
            for i in k..k + n as i64 {
                let want = m.sample(i, |x| data[0].get(x).copied());
                assert_eq!(v.get(0, i).to_bits(), want.to_bits(), "{m:?} at {i}");
            }
            drop(v);
            k += n as i64;
            d.pump();
        }
    }
}

#[test]
fn the_synchronous_reader_reads_the_file_as_memory_does() {
    let data = ramp(2, 200_000);
    let (d, opened) = fake(data.clone(), 1_024);
    d.with_sync(|get| {
        for x in [0usize, 1, 65_535, 65_536, 199_999, 3, 150_000] {
            assert_eq!(get(1, x), Some(data[1][x]));
        }
        assert_eq!(get(0, 200_000), None, "past the file");
    });
    d.with_sync(|get| assert_eq!(get(0, 9), Some(data[0][9])));
    assert_eq!(opened.load(Ordering::Relaxed), 1, "one reader for every synchronous read");
}

#[test]
fn eight_streams_fit_and_a_ninth_takes_the_longest_idle() {
    let (d, _) = fake(ramp(1, 10_000), 128);
    for clip in 1..=SLOTS as u64 {
        d.cue(&map(clip, 10_000), 0);
        d.pump();
    }
    for clip in 1..=SLOTS as u64 {
        assert!(d.stream(&map(clip, 10_000), 0, 16).is_some(), "clip {clip}");
    }
    // Every stream was read in this pass; the next pass sees them all still, then clip 1 reads on.
    d.pump();
    drop(d.stream(&map(1, 10_000), 16, 16));
    d.cue(&map(99, 10_000), 0);
    d.pump();
    assert!(d.stream(&map(99, 10_000), 0, 16).is_some(), "the new stream got a slot");
    assert!(d.stream(&map(1, 10_000), 32, 16).is_some(), "the stream read since is kept");
}

#[test]
fn prime_fills_a_window_on_the_calling_thread() {
    let (d, _) = fake(ramp(1, 100_000), 4_096);
    let m = map(5, 100_000);
    assert!(d.prime(&m, 1_000));
    assert!(d.ready(&m, 1_000, 4_096));
    assert!(d.stream(&m, 1_000, 4_096).is_some());
}

/// A source whose file opens only while `gone` is false (a drive unplugged and plugged back),
/// counting the readers opened.
fn flaky(data: Vec<Vec<f32>>, window: usize, gone: &Arc<AtomicBool>) -> (Arc<DiskSource>, Arc<AtomicUsize>) {
    let opened = Arc::new(AtomicUsize::new(0));
    let frames = data.first().map_or(0, Vec::len) as u64;
    let channels = data.len();
    let data = Arc::new(data);
    let (count, gone) = (Arc::clone(&opened), Arc::clone(gone));
    let opener: Opener = Arc::new(move || {
        if gone.load(Ordering::Relaxed) {
            return Err("gone".into());
        }
        count.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(Fake { data: Arc::clone(&data) }) as Box<dyn DiskFile>)
    });
    (DiskSource::with_window(PathBuf::from("lost.wav"), channels, frames, 48_000, window, opener), opened)
}

#[test]
fn a_streamed_file_that_could_not_be_opened_is_opened_again_on_the_next_prime_issue_523() {
    let data = ramp(1, 10_000);
    let gone = Arc::new(AtomicBool::new(true));
    let (d, opened) = flaky(data.clone(), 1_024, &gone);
    let m = map(2, 10_000);
    assert!(!d.prime(&m, 0), "nothing to play while the file is gone");
    gone.store(false, Ordering::Relaxed);
    assert!(d.prime(&m, 0), "the drive is back: the next prime opens the file again");
    assert_eq!(opened.load(Ordering::Relaxed), 1);
    let v = d.stream(&m, 0, 64).expect("filled");
    assert_eq!(v.get(0, 63), data[0][63]);
    drop(v);
    assert_eq!(d.fault(), None, "the good open cleared the fault");
}

#[test]
fn an_unopenable_file_is_reported_with_its_reason_until_a_later_open_succeeds_issue_523() {
    let data = ramp(1, 10_000);
    let gone = Arc::new(AtomicBool::new(true));
    let (d, _) = flaky(data.clone(), 1_024, &gone);
    assert_eq!(d.fault(), None, "nothing tried yet");
    d.with_sync(|get| assert_eq!(get(0, 5), None));
    assert_eq!(d.fault().as_deref(), Some("could not be opened: gone"));
    assert_eq!(d.sync_failures(), 1);
    assert_eq!(d.check(), Err("could not be opened: gone".into()), "the pre-flight says why");
    assert_eq!(d.sync_failures(), 2);
    gone.store(false, Ordering::Relaxed);
    assert!(d.prime(&map(1, 10_000), 0));
    assert!(d.fault().is_some(), "the streamer's good open does not clear the offline reader's fault");
    assert_eq!(d.check(), Ok(()));
    assert_eq!(d.fault(), None);
    d.with_sync(|get| assert_eq!(get(0, 5), Some(data[0][5])));
    assert_eq!(d.sync_failures(), 2);
}

/// A file whose reads fail while `failing` is set (a drive that drops out mid-take).
struct Dropping {
    inner: Fake,
    failing: Arc<AtomicBool>,
}

impl DiskFile for Dropping {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        if self.failing.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("device not ready"));
        }
        self.inner.read_at(at, frames, out)
    }
}

fn dropping(data: Vec<Vec<f32>>, window: usize, failing: &Arc<AtomicBool>) -> Arc<DiskSource> {
    let frames = data.first().map_or(0, Vec::len) as u64;
    let channels = data.len();
    let data = Arc::new(data);
    let failing = Arc::clone(failing);
    let opener: Opener =
        Arc::new(move || Ok(Box::new(Dropping { inner: Fake { data: Arc::clone(&data) }, failing: Arc::clone(&failing) }) as Box<dyn DiskFile>));
    DiskSource::with_window(PathBuf::from("dropped.wav"), channels, frames, 48_000, window, opener)
}

#[test]
fn a_read_that_fails_is_counted_and_the_next_check_reads_the_file_again_issue_523() {
    let data = ramp(1, 200_000);
    let failing = Arc::new(AtomicBool::new(false));
    let d = dropping(data.clone(), 1_024, &failing);
    d.with_sync(|get| assert_eq!(get(0, 10), Some(data[0][10])));
    assert_eq!((d.fault(), d.sync_failures()), (None, 0));
    failing.store(true, Ordering::Relaxed);
    d.with_sync(|get| assert_eq!(get(0, 150_000), None, "past the cached window: a read, which fails"));
    assert_eq!(d.fault().as_deref(), Some("reading failed: device not ready"));
    assert_eq!(d.sync_failures(), 1);
    failing.store(false, Ordering::Relaxed);
    d.with_sync(|get| assert_eq!(get(0, 150_000), None, "a reader that failed reads nothing more"));
    assert_eq!(d.sync_failures(), 1, "and does not count the same failure twice");
    assert_eq!(d.check(), Ok(()));
    d.with_sync(|get| assert_eq!(get(0, 150_000), Some(data[0][150_000])));
    assert_eq!(d.fault(), None);
}

#[test]
fn a_live_read_that_fails_is_reported_and_the_next_prime_streams_the_file_not_the_silence_issue_523() {
    let data = ramp(1, 10_000);
    let failing = Arc::new(AtomicBool::new(true));
    let d = dropping(data.clone(), 1_024, &failing);
    let m = map(6, 10_000);
    d.prime(&m, 0);
    assert_eq!(d.fault().as_deref(), Some("reading failed: device not ready"));
    failing.store(false, Ordering::Relaxed);
    assert!(d.prime(&m, 0));
    assert_eq!(d.fault(), None);
    let v = d.stream(&m, 0, 64).expect("filled");
    for k in 0..64 {
        assert_eq!(v.get(0, k), data[0][k as usize], "clip time {k} is the file's, not the silence the failed pass wrote");
    }
}

#[test]
fn the_threshold_reads_the_session_setting() {
    let mut s = Session::default();
    if std::env::var(STREAM_MINUTES_ENV).is_ok() {
        return;
    }
    assert_eq!(stream_threshold_frames(&s, 48_000), 10 * 60 * 48_000);
    s.edit.values.insert(STREAM_MINUTES_KEY.into(), 0.5);
    assert_eq!(stream_threshold_frames(&s, 48_000), 30 * 48_000);
    s.edit.values.insert(STREAM_MINUTES_KEY.into(), -1.0);
    assert_eq!(stream_threshold_frames(&s, 48_000), 10 * 60 * 48_000, "a nonsense setting is the default");
}

/// A unique temp dir, removed on drop (E51).
struct Dir(transit_test_support::TestDir);
impl Dir {
    fn new(tag: &str) -> Dir {
        let p = transit_test_support::TestDir::new(&format!("disk-{tag}"));
        let _ = std::fs::create_dir_all(&p);
        Dir(p)
    }
}

fn write_wav(path: &Path, data: &[f32]) {
    let mut bytes = transit_audio_io::wav_float_header(1, 48_000, data.len() as u64).unwrap_or_default();
    bytes.extend(data.iter().flat_map(|v| v.to_le_bytes()));
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn peaks_are_built_once_then_read_from_the_sidecar_until_the_file_changes() {
    let dir = Dir::new("peaks");
    let path = dir.0.join("Take_01.wav");
    let data: Vec<f32> = (0..5_000).map(|i| ((i % 700) as f32 / 350.0) - 1.0).collect();
    write_wav(&path, &data);
    let d = DiskSource::open(&path, std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)).unwrap_or_else(|e| panic!("{e}"));
    let (built, origin) = load_or_build_peaks(&path, d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(origin, PeaksOrigin::Built);
    assert!(sidecar_path(&path).is_file());
    assert_eq!(built.channels[0].len(), 5_000usize.div_ceil(PEAKS_BUCKET));
    let first = data[..PEAKS_BUCKET].iter().fold((f32::INFINITY, f32::NEG_INFINITY), |a, &v| (a.0.min(v), a.1.max(v)));
    assert_eq!(built.channels[0][0], first);
    assert_eq!(built.minmax(0, 0.0, 5_000.0), Some((-1.0, data.iter().copied().fold(f32::NEG_INFINITY, f32::max))));

    let (again, origin) = load_or_build_peaks(&path, d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((origin, &again), (PeaksOrigin::Sidecar, &built), "reused, unread");

    // Same length, new content and a new modification time: the sidecar is stale.
    let louder: Vec<f32> = data.iter().map(|v| v * 0.5).collect();
    write_wav(&path, &louder);
    let f = std::fs::File::options().write(true).open(&path).unwrap_or_else(|e| panic!("{e}"));
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap_or_else(|e| panic!("{e}"));
    drop(f);
    let (rebuilt, origin) = load_or_build_peaks(&path, d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(origin, PeaksOrigin::Built);
    assert_ne!(rebuilt, built);
}

#[test]
fn a_ring_cued_under_another_mapping_of_the_clip_is_a_miss() {
    let data = ramp(1, 10_000);
    let (d, _) = fake(data.clone(), 1_024);
    let m = map(4, 5_000);
    d.cue(&m, 0);
    d.pump();
    assert!(d.stream(&m, 0, 64).is_some());
    let slipped = StreamMap { offset: 100, ..m };
    let longer = StreamMap { length: 6_000, ..m };
    let looped = StreamMap { looping: Some(ClipLoop { start: 0, end: 500 }), ..m };
    for other in [slipped, longer, looped] {
        assert!(d.stream(&other, 64, 64).is_none(), "{other:?}");
        d.underrun(&other, 64);
        d.pump();
        let v = d.stream(&other, 64, 64).expect("re-cued with the new mapping");
        assert_eq!(v.get(0, 64), other.sample(64, |x| data[0].get(x).copied()));
    }
}

#[test]
fn a_sidecar_that_cannot_be_written_leaves_no_temporary_file() {
    let dir = Dir::new("peaks-fail");
    let path = dir.0.join("Take_02.wav");
    write_wav(&path, &[0.25; 3_000]);
    // A folder where the sidecar would go: the rename fails.
    std::fs::create_dir_all(sidecar_path(&path).join("x")).unwrap_or_else(|e| panic!("{e}"));
    let d = DiskSource::open(&path, std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)).unwrap_or_else(|e| panic!("{e}"));
    let (peaks, origin) = load_or_build_peaks(&path, d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((origin, peaks.channels[0][0]), (PeaksOrigin::Built, (0.25, 0.25)), "kept in memory");
    let mut tmp = sidecar_path(&path).into_os_string();
    tmp.push(".tmp");
    assert!(!PathBuf::from(tmp).exists());
}

/// What a test sees of a peaks build's reader (#663): `entered` when its first read starts (that
/// read then waits for `go`), `closed` when the reader is dropped, after any sidecar write.
struct Events {
    entered: std::sync::mpsc::Sender<()>,
    go: Mutex<std::sync::mpsc::Receiver<()>>,
    closed: std::sync::mpsc::Sender<()>,
}

/// The peaks build's reader: its first read waits for the test; every read is counted.
struct Held {
    inner: Fake,
    events: Arc<Events>,
    reads: Arc<AtomicUsize>,
}

impl DiskFile for Held {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = self.events.entered.send(());
            let _ = self.events.go.lock().unwrap_or_else(PoisonError::into_inner).recv();
        }
        self.inner.read_at(at, frames, out)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.events.closed.send(());
    }
}

/// A registered source of `frames` frames at `path` (a real file there, for the sidecar's stamp),
/// held by the streaming thread's way too (`source`) and owned by `owner`; its peaks build reads
/// through a [`Held`] reader, the streaming thread through a plain one.
struct Rig {
    source: Arc<DiskSource>,
    owner: crate::SourceAudio,
    entered: std::sync::mpsc::Receiver<()>,
    go: std::sync::mpsc::Sender<()>,
    closed: std::sync::mpsc::Receiver<()>,
    reads: Arc<AtomicUsize>,
}

fn held(path: &Path, frames: usize) -> Rig {
    let data = Arc::new(ramp(1, frames));
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel();
    let (closed_tx, closed) = std::sync::mpsc::channel();
    let events = Arc::new(Events { entered: entered_tx, go: Mutex::new(go_rx), closed: closed_tx });
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reads);
    let opener: Opener = Arc::new(move || {
        let inner = Fake { data: Arc::clone(&data) };
        if std::thread::current().name() == Some(stream::PEAKS_THREAD) {
            Ok(Box::new(Held { inner, events: Arc::clone(&events), reads: Arc::clone(&counted) }) as Box<dyn DiskFile>)
        } else {
            Ok(Box::new(inner) as Box<dyn DiskFile>)
        }
    });
    let source = DiskSource::with_window(path.to_path_buf(), 1, frames as u64, 48_000, 1_024, opener);
    let owner = crate::SourceAudio::disk(Arc::clone(&source));
    register(&source);
    Rig { source, owner, entered, go, closed, reads }
}

fn sidecar_tmp_of(path: &Path) -> PathBuf {
    let mut tmp = sidecar_path(path).into_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

#[test]
fn a_source_let_go_while_its_peaks_are_built_writes_no_sidecar_after_issue_663() {
    let dir = Dir::new("peaks-let-go");
    let path = dir.0.join("Take_03.wav");
    write_wav(&path, &[0.25; 3_000]);
    // One read builds these peaks: the owner goes during it, after the build's last look at the
    // gate before the sidecar write (a session closed and its folder deleted as its peaks finish).
    let Rig { source, owner, entered, go, closed, .. } = held(&path, 3_000);
    entered.recv().unwrap_or_else(|e| panic!("{e}"));
    // The streaming thread can hold the source a pass longer than its owner: `source` does here.
    drop(owner);
    go.send(()).unwrap_or_else(|e| panic!("{e}"));
    closed.recv().unwrap_or_else(|e| panic!("{e}"));
    assert!(!sidecar_path(&path).exists(), "a sidecar was written after the source's owner let it go");
    assert!(!sidecar_tmp_of(&path).exists(), "a temporary sidecar was written after the source's owner let it go");
    assert!(source.peaks().is_none());
}

#[test]
fn a_source_let_go_stops_its_peaks_build_at_the_next_read_issue_663() {
    let dir = Dir::new("peaks-stop");
    let path = dir.0.join("Take_04.wav");
    write_wav(&path, &[0.25; 3_000]);
    let rig = held(&path, 3 * peaks::READ_FRAMES + 1);
    rig.entered.recv().unwrap_or_else(|e| panic!("{e}"));
    let Rig { source, owner, go, closed, reads, .. } = rig;
    drop(owner);
    go.send(()).unwrap_or_else(|e| panic!("{e}"));
    closed.recv().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the build read on after the source's owner let it go");
    assert!(!sidecar_path(&path).exists());
    drop(source);
}

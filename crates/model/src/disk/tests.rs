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
struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Dir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("transit-disk-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::create_dir_all(&p);
        Dir(p)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

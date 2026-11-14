//! A disk-backed source moved onto a copy of its file (#629), block-driven as the other disk
//! tests: the tests call [`DiskSource::pump`] themselves, so nothing depends on a clock.

use super::super::tests::{Dir, Fake, map, ramp, write_wav};
use super::super::*;
use std::sync::atomic::AtomicUsize;

/// A reader that counts itself among the file's open readers while it lives.
struct Counted {
    inner: Fake,
    open: Arc<AtomicUsize>,
}

impl DiskFile for Counted {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        self.inner.read_at(at, frames, out)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An opener of `data` whose readers are counted in the returned counter while they are open.
fn counted(data: &Arc<Vec<Vec<f32>>>) -> (Opener, Arc<AtomicUsize>) {
    let open = Arc::new(AtomicUsize::new(0));
    let (data, count) = (Arc::clone(data), Arc::clone(&open));
    let opener: Opener = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Counted { inner: Fake { data: Arc::clone(&data) }, open: Arc::clone(&count) }) as Box<dyn DiskFile>)
    });
    (opener, open)
}

#[test]
fn a_take_playing_while_a_save_moves_it_onto_its_copy_plays_on_without_a_gap_and_lets_the_old_file_go_issue_629() {
    let data = Arc::new(ramp(2, 20_000));
    let (old, old_open) = counted(&data);
    let (copy, copy_open) = counted(&data);
    let d = DiskSource::with_window(PathBuf::from("Old/Vox_01.wav"), 2, 20_000, 48_000, 2_048, old);
    let m = map(1, 20_000);
    assert!(d.prime(&m, 0));
    // An offline read before the save holds a reader of the old file too.
    d.with_sync(|get| assert_eq!(get(1, 7), Some(data[1][7])));
    assert_eq!(old_open.load(Ordering::SeqCst), 2, "the streamer's and the synchronous reader's");
    let mut k = 0;
    while k + 64 <= 20_000 {
        if k == 6_400 {
            // The save: the copy holds the same bytes.
            d.retarget(PathBuf::from("New/Audio Files/Vox_01.wav"), Arc::clone(&copy));
            assert_eq!(old_open.load(Ordering::SeqCst), 0, "no reader of the old file is held after the save");
            assert_eq!(d.path(), PathBuf::from("New/Audio Files/Vox_01.wav"));
        }
        // The audio thread reads a block, then the streamer runs a pass.
        let v = d.stream(&m, k, 64).unwrap_or_else(|| panic!("the ring was short at clip time {k}"));
        for i in k..k + 64 {
            assert_eq!((v.get(0, i), v.get(1, i)), (data[0][i as usize], data[1][i as usize]), "clip time {i}");
        }
        drop(v);
        k += 64;
        d.pump();
    }
    assert_eq!(d.underruns(), 0, "no block went without audio across the save");
    assert_eq!((old_open.load(Ordering::SeqCst), copy_open.load(Ordering::SeqCst)), (0, 1), "the streamer reads the copy");
    d.with_sync(|get| assert_eq!(get(0, 19_999), Some(data[0][19_999])));
    assert_eq!(copy_open.load(Ordering::SeqCst), 2, "an offline render reads the copy");
    assert_eq!(old_open.load(Ordering::SeqCst), 0);
}

#[test]
fn a_take_whose_old_file_was_gone_plays_from_its_copy_at_the_next_prime_issue_629() {
    let data = Arc::new(ramp(1, 10_000));
    let gone: Opener = Arc::new(|| Err("Old/Vox_01.wav: the folder was deleted".to_string()));
    let d = DiskSource::with_window(PathBuf::from("Old/Vox_01.wav"), 1, 10_000, 48_000, 1_024, gone);
    let m = map(3, 10_000);
    assert!(!d.prime(&m, 0), "nothing to play while the file is gone");
    assert!(d.fault().is_some());
    let (copy, _) = counted(&data);
    d.retarget(PathBuf::from("New/Audio Files/Vox_01.wav"), copy);
    assert_eq!(d.fault(), None, "the gone file's fault went with it");
    // The ring filled while the file was gone held silence: the copy's audio is read in its place.
    assert!(d.prime(&m, 0));
    let v = d.stream(&m, 0, 64).expect("filled from the copy");
    assert_eq!(v.get(0, 63), data[0][63]);
    drop(v);
    assert_eq!(d.check(), Ok(()), "an offline render can read it");
}

#[test]
fn a_copy_that_is_not_the_same_audio_is_refused_and_the_source_reads_on_issue_629() {
    let dir = Dir::new("retarget-refused");
    let take = dir.0.join("Vox_01.wav");
    write_wav(&take, &[0.5; 3_000]);
    let shorter = dir.0.join("Vox_01_01.wav");
    write_wav(&shorter, &[0.5; 2_999]);
    let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let d = DiskSource::open(&take, len(&take)).unwrap_or_else(|e| panic!("{e}"));
    let err = d.retarget_file(&shorter, len(&shorter)).unwrap_err();
    assert!(err.contains("2999 frames"), "{err}");
    assert_eq!(d.path(), take, "the source reads the file it read");
}

#[test]
fn built_peaks_are_written_beside_the_copy_so_it_opens_without_a_rebuild_issue_629() {
    let dir = Dir::new("retarget-peaks");
    let (take, copy) = (dir.0.join("Vox_01.wav"), dir.0.join("Vox_01_01.wav"));
    let data: Vec<f32> = (0..5_000).map(|i| ((i % 900) as f32 / 450.0) - 1.0).collect();
    write_wav(&take, &data);
    std::fs::copy(&take, &copy).unwrap_or_else(|e| panic!("{e}"));
    let len = std::fs::metadata(&take).map(|m| m.len()).unwrap_or(0);
    let d = DiskSource::open(&take, len).unwrap_or_else(|e| panic!("{e}"));
    let (built, _) = load_or_build_peaks(&take, &d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    d.set_peaks(built.clone());
    d.retarget_file(&copy, len).unwrap_or_else(|e| panic!("{e}"));
    assert!(sidecar_path(&copy).is_file(), "the copy has its sidecar");
    let (read, origin) = load_or_build_peaks(&copy, &d.opener(), 1, d.frames()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((origin, read), (PeaksOrigin::Sidecar, built), "the copy's sidecar is current and holds the same peaks");
}

/// A peaks build reader of the old file: its first read waits until the old gate is closed and a
/// helper says go; `dropped` is set when it is closed.
struct Waiting {
    inner: Fake,
    first: bool,
    go: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
    entered: std::sync::mpsc::Sender<()>,
    dropped: Arc<AtomicBool>,
}

impl DiskFile for Waiting {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        if std::mem::take(&mut self.first) {
            let _ = self.entered.send(());
            let _ = self.go.lock().unwrap_or_else(PoisonError::into_inner).recv();
        }
        self.inner.read_at(at, frames, out)
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_peaks_build_of_the_old_file_is_stopped_and_let_go_before_the_save_returns_issue_629() {
    let dir = Dir::new("retarget-build");
    let (old_path, new_path) = (dir.0.join("Old.wav"), dir.0.join("New.wav"));
    write_wav(&old_path, &[0.25; 3_000]);
    write_wav(&new_path, &[0.25; 3_000]);
    let frames = 3 * peaks::READ_FRAMES + 1;
    let data = Arc::new(ramp(1, frames));
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (go, go_rx) = std::sync::mpsc::channel();
    let go_rx = Arc::new(Mutex::new(go_rx));
    let dropped = Arc::new(AtomicBool::new(false));
    let (inner_data, flag) = (Arc::clone(&data), Arc::clone(&dropped));
    let old: Opener = Arc::new(move || {
        let inner = Fake { data: Arc::clone(&inner_data) };
        if std::thread::current().name() == Some(stream::PEAKS_THREAD) {
            let w = Waiting { inner, first: true, go: Arc::clone(&go_rx), entered: entered_tx.clone(), dropped: Arc::clone(&flag) };
            Ok(Box::new(w) as Box<dyn DiskFile>)
        } else {
            Ok(Box::new(inner) as Box<dyn DiskFile>)
        }
    });
    // The new file's peaks build announces itself.
    let (built_tx, built_rx) = std::sync::mpsc::channel();
    let new_data = Arc::clone(&data);
    let new: Opener = Arc::new(move || {
        if std::thread::current().name() == Some(stream::PEAKS_THREAD) {
            let _ = built_tx.send(());
        }
        Ok(Box::new(Fake { data: Arc::clone(&new_data) }) as Box<dyn DiskFile>)
    });
    let source = DiskSource::with_window(old_path.clone(), 1, frames as u64, 48_000, 1_024, old);
    let owner = crate::SourceAudio::disk(Arc::clone(&source));
    register(&source);
    entered.recv().unwrap_or_else(|e| panic!("{e}"));
    let old_gate = Arc::clone(&source.peaks_gate.lock().unwrap_or_else(PoisonError::into_inner));
    // The build goes on once the save has closed its gate.
    let helper = std::thread::spawn(move || {
        while !old_gate.is_closed() {
            std::thread::yield_now();
        }
        let _ = go.send(());
    });
    source.retarget(new_path.clone(), new);
    assert!(dropped.load(Ordering::SeqCst), "the old file's peaks reader was still open when the save returned");
    helper.join().unwrap_or_else(|_| panic!("helper"));
    assert!(!sidecar_path(&old_path).exists(), "nothing was written beside the old file");
    built_rx.recv().unwrap_or_else(|e| panic!("the new file's peaks were not built: {e}"));
    drop(owner);
    drop(source);
}

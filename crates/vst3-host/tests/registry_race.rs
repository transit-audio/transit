//! Deterministic scan/publication races.
mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use transit_vst3_host as host;

/// Plug-ins in the VST3 fixture bundle (gain, synth and the F10 events fixture).
const FIXTURE_PLUGINS: usize = 3;

static SERIAL: Mutex<()> = Mutex::new(());

struct Test {
    _serial: MutexGuard<'static, ()>,
    dir: transit_test_support::TestDir,
    fixture: PathBuf,
}

impl Test {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = common::fixture_dir().to_path_buf();
        // A fresh folder per test, never one left by an earlier run (E51 recurrence, #347, as #346 did for CLAP). The
        // process id alone repeats across runs whose processes get the same ids (a VM that boots for each run). The
        // guard names the folder with the clock and a counter, creates it with `create_dir` (it refuses a folder that
        // exists) and removes it when the test ends, so no plug-in a scan loaded is ever rewritten in place.
        let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-registry");
        host::set_scan_extra_dirs_only(true);
        host::set_scan_hook(None);
        host::set_extra_dirs(Vec::new());
        host::set_scanner(None);
        host::set_cache_file(Some(dir.join("cache.json")));
        Self { _serial: serial, dir, fixture }
    }

    fn cache(&self) -> Vec<serde_json::Value> {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(self.dir.join("cache.json")).unwrap()).unwrap();
        json["files"].as_array().unwrap().clone()
    }

    fn assert_fixture(&self, list: &[host::Vst3Descriptor]) {
        let wanted = host::scan_paths(std::slice::from_ref(&self.fixture));
        assert_eq!(wanted.len(), FIXTURE_PLUGINS);
        for d in wanted {
            assert!(list.iter().any(|p| p.id == d.id), "missing {}", d.id);
        }
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        host::set_scan_hook(None);
        host::set_scan_extra_dirs_only(false);
        host::set_scanner(None);
        host::set_cache_file(None);
        host::set_extra_dirs(Vec::new());
    }
}

fn paused_scan(during: impl FnOnce()) -> Vec<host::Vst3Descriptor> {
    let (loaded_tx, loaded_rx) = channel();
    let (go_tx, go_rx) = channel();
    let go_rx = Mutex::new(go_rx);
    host::set_scan_hook(Some(Arc::new(move || {
        loaded_tx.send(()).unwrap();
        go_rx.lock().unwrap().recv_timeout(Duration::from_secs(60)).unwrap();
    })));
    // Release the worker even if an assertion in the caller fails.
    struct Resume(std::sync::mpsc::Sender<()>);
    impl Drop for Resume {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let resume = Resume(go_tx);
    let scanner = std::thread::spawn(host::rescan);
    loaded_rx.recv_timeout(Duration::from_secs(60)).expect("scan reached publication hook");
    host::set_scan_hook(None);
    during();
    drop(resume);
    scanner.join().unwrap()
}

#[test]
fn a_folder_added_during_a_scan_is_in_the_scan_that_publishes_after_it() {
    let t = Test::new();
    let published = paused_scan(|| {
        assert_eq!(host::add_search_dir(&t.fixture), FIXTURE_PLUGINS);
        assert!(host::cached().is_none(), "registry reads do not wait for publication");
    });
    t.assert_fixture(&published);
    t.assert_fixture(&host::scan());
}

#[test]
fn late_folder_uses_configured_child_scanner_and_records_failure() {
    let t = Test::new();
    host::set_scanner(Some(t.dir.join("unavailable-scanner")));
    let published = paused_scan(|| host::set_extra_dirs(vec![t.fixture.clone()]));
    assert!(published.is_empty(), "late files bypassed the configured child scanner");
    let problems = host::problems();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].1.contains("scan process did not start"), "{problems:?}");
    let files = t.cache();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["error"].as_str(), Some(problems[0].1.as_str()));
}

#[test]
fn skip_before_late_round_leaves_files_unloaded_and_uncached() {
    let t = Test::new();
    let published = paused_scan(|| {
        host::set_extra_dirs(vec![t.fixture.clone()]);
        host::skip_scan();
    });
    assert!(published.is_empty(), "Skip must apply to late files");
    assert!(t.cache().is_empty(), "skipped files must be retried");
    assert!(host::problems().is_empty(), "Skip is not a load failure");
    assert!(host::progress().is_none());
    t.assert_fixture(&host::rescan());
}

#[test]
fn late_plugins_join_the_main_scan_cache() {
    let t = Test::new();
    let initial = t.dir.join("initial.vst3");
    std::fs::write(&initial, b"not a plugin").unwrap();
    host::set_extra_dirs(vec![initial.clone()]);
    let published = paused_scan(|| host::set_extra_dirs(vec![initial.clone(), t.fixture.clone()]));
    t.assert_fixture(&published);
    let files = t.cache();
    assert_eq!(files.len(), 2, "both initial and late results are persisted");
    let ids: Vec<_> = files.iter().flat_map(|f| f["plugins"].as_array().unwrap()).map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), FIXTURE_PLUGINS);
    assert!(published.iter().all(|d| ids.contains(&d.id.as_str())));
    assert!(files.iter().any(|f| f["error"].is_string()), "initial failure is retained");
}

#[test]
fn late_malformed_files_join_problems_and_cache() {
    let t = Test::new();
    let bad = t.dir.join("bad.vst3");
    std::fs::write(&bad, b"not a plugin").unwrap();
    assert!(paused_scan(|| host::set_extra_dirs(vec![bad.clone()])).is_empty());
    let problems = host::problems();
    assert_eq!(problems.len(), 1, "late failures must be reported");
    assert_eq!(problems[0].0, bad);
    let files = t.cache();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["error"].as_str(), Some(problems[0].1.as_str()));
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn installing_into_an_already_snapshotted_folder_is_published() {
    let t = Test::new();
    let installed = t.dir.join("install");
    std::fs::create_dir_all(&installed).unwrap();
    host::set_extra_dirs(vec![installed.clone()]);
    let published = paused_scan(|| {
        copy_tree(&t.fixture, &installed);
        assert_eq!(host::add_search_dir(&installed), FIXTURE_PLUGINS);
    });
    t.assert_fixture(&published);
    t.assert_fixture(&host::cached().unwrap());
    assert_eq!(t.cache().iter().flat_map(|f| f["plugins"].as_array().unwrap()).count(), FIXTURE_PLUGINS);
}

#[test]
fn refreshing_an_overlapping_folder_preserves_the_first_id_winner() {
    let t = Test::new();
    let first = t.dir.join("a");
    let second = t.dir.join("b");
    copy_tree(&t.fixture, &first);
    copy_tree(&t.fixture, &second);
    host::set_extra_dirs(vec![first.clone(), second]);
    let published = paused_scan(|| {
        assert_eq!(host::add_search_dir(&first), FIXTURE_PLUGINS);
    });
    t.assert_fixture(&published);
    assert!(published.iter().all(|d| d.path.starts_with(&first)), "refresh reordered duplicate IDs");
    assert_eq!(t.cache().len(), 2, "one cache row per file");
}

#[test]
fn continuous_additions_publish_after_four_late_rounds_and_defer_the_rest() {
    let t = Test::new();
    let dirs: Vec<_> = (0..8)
        .map(|i| {
            let dir = t.dir.join(format!("late-{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("bad.vst3"), b"not a plugin").unwrap();
            dir
        })
        .collect();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let additions = dirs.clone();
    host::set_scan_hook(Some(Arc::new(move || {
        let i = count.fetch_add(1, Ordering::Relaxed);
        if let Some(dir) = additions.get(i) {
            let mut extra = host::search_dirs().1;
            extra.push(dir.clone());
            host::set_extra_dirs(extra);
        }
    })));
    assert!(host::rescan().is_empty());
    host::set_scan_hook(None);
    assert_eq!(calls.load(Ordering::Relaxed), 5, "one initial round plus four late rounds");
    assert_eq!(host::problems().len(), 4);
    assert_eq!(t.cache().len(), 4);
    assert!(host::search_dirs().1.contains(&dirs[4]), "deferred folder stays registered");
    assert!(host::rescan().is_empty());
    assert_eq!(host::problems().len(), 5, "the next rescan covers the deferred folder");
    assert_eq!(t.cache().len(), 5);
}

/// #659: plug-in files that are found but none of which loads are reported as such (counts and the
/// first loader error), not as "none found"; a file that loads keeps the report away.
#[test]
fn a_scan_whose_files_all_fail_reports_them_instead_of_none_found() {
    let t = Test::new();
    let (a, b) = (t.dir.join("a.vst3"), t.dir.join("b.vst3"));
    std::fs::write(&a, b"not a plugin").unwrap();
    std::fs::write(&b, b"not a plugin either").unwrap();
    host::set_extra_dirs(vec![a.clone(), b.clone()]);
    assert!(host::rescan().is_empty());
    let summary = host::scan_summary().expect("a published scan has a summary");
    assert_eq!((summary.found, summary.loaded, summary.failed), (2, 0, 2), "{summary:?}");
    let first = summary.first_error.clone().expect("the first loader error");
    assert_eq!(Some(first.as_str()), host::problems().first().map(|p| p.1.as_str()));
    let message = summary.none_loaded_message("VST3").expect("none loaded is said");
    assert!(message.starts_with("2 VST3 plug-in files found, but none could be loaded."), "{message}");
    assert!(message.ends_with(&first), "{message}");
    // The cache records the build that failed, so another build retries the files.
    assert!(t.cache().iter().all(|f| f["host"].as_str().is_some_and(|h| !h.is_empty())), "{:?}", t.cache());

    host::set_extra_dirs(vec![a, t.fixture.clone()]);
    t.assert_fixture(&host::rescan());
    let summary = host::scan_summary().expect("a published scan has a summary");
    assert_eq!((summary.found, summary.loaded, summary.failed), (2, 1, 1), "{summary:?}");
    assert_eq!(summary.none_loaded_message("VST3"), None);
}

/// Runs one background scan to its end and returns what it published. The scan thread is idle
/// before the request, so the request starts a scan: that is asserted at once (a request that
/// did not start fails here, with the reason, instead of timing out later). `wait_for_scans`
/// returns when the thread has ended, which is after publication; nothing here waits on a clock.
fn background_scan(clear_cache: bool) -> Vec<host::Vst3Descriptor> {
    host::wait_for_scans();
    let ran = Arc::new(AtomicUsize::new(0));
    let count = ran.clone();
    host::set_scan_hook(Some(Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    })));
    host::rescan_in_background(clear_cache);
    assert!(host::scanning(), "the request started no scan although none was running");
    host::wait_for_scans();
    host::set_scan_hook(None);
    assert_eq!(ran.load(Ordering::Relaxed), 1, "the scan loaded its files exactly once");
    host::cached().expect("the scan published")
}

/// Plants a wrong cached failure for the good file and a wrong cached success for a bad one, so
/// that reuse and a fresh read give different answers. Returns the bad file's real error.
fn plant_wrong_cache(t: &Test) -> serde_json::Value {
    let bad = t.dir.join("bad.vst3");
    std::fs::write(&bad, b"not a plugin").unwrap();
    host::set_extra_dirs(vec![t.fixture.clone(), bad]);
    t.assert_fixture(&host::rescan());
    let mut cache: serde_json::Value = serde_json::from_slice(&std::fs::read(t.dir.join("cache.json")).unwrap()).unwrap();
    let files = cache["files"].as_array_mut().unwrap();
    assert_eq!(files.len(), 2, "one good file and one bad file");
    let (good, broken) = if files[0]["error"].is_null() { (0, 1) } else { (1, 0) };
    assert!(files[broken]["error"].is_string(), "the bad file failed");
    let real_error = files[broken]["error"].clone();
    let good_plugins = files[good]["plugins"].clone();
    files[good]["error"] = serde_json::json!("planted failure");
    files[good]["plugins"] = serde_json::json!([]);
    files[broken]["error"] = serde_json::Value::Null;
    files[broken]["plugins"] = good_plugins;
    std::fs::write(t.dir.join("cache.json"), serde_json::to_vec(&cache).unwrap()).unwrap();
    real_error
}

/// #659: the start-up scan reuses what the cache holds for unchanged files (a remembered failure
/// and a remembered success alike), and an explicit rescan loads every file again and rewrites
/// the cache from what it found. The start-up scan here is `rescan_in_background(false)`, the same
/// `scan_now(true)` that `scan_in_background` runs.
#[test]
fn an_explicit_rescan_loads_every_file_again_but_the_start_up_scan_reuses_the_cache() {
    let t = Test::new();
    let real_error = plant_wrong_cache(&t);

    // The start-up scan: both planted entries are taken from the cache.
    background_scan(false);
    let problems = host::problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].1, "planted failure", "the cached failure is reused, not loaded again");

    // The explicit rescan: nothing comes from the cache, and the cache is rewritten.
    t.assert_fixture(&background_scan(true));
    let problems = host::problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(serde_json::json!(problems[0].1), real_error, "the cached success is loaded again and fails");
    let after = t.cache();
    let errors: Vec<_> = after.iter().filter_map(|f| f["error"].as_str()).collect();
    assert_eq!(errors.len(), 1, "{after:?}");
    assert!(!errors.contains(&"planted failure"), "the cache is rewritten from the results: {after:?}");
}

/// Holds a scan that reuses the cache after it loaded its files and before it publishes, runs
/// `during` meanwhile, lets the scan go (it panics when `fail_first`) and returns how many scans
/// reached their publication step. The held scan blocks on a channel. The only bound is the
/// `recv_timeout` on the hold, which can fire only if the scan thread never reaches the hook
/// (nothing before the hook can fail in these tests), so never in a correct run.
fn held_start_up_scan(fail_first: bool, during: impl FnOnce()) -> usize {
    host::wait_for_scans();
    let (held_tx, held_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let held_tx = Mutex::new(held_tx);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    host::set_scan_hook(Some(Arc::new(move || {
        if count.fetch_add(1, Ordering::Relaxed) == 0 {
            held_tx.lock().unwrap().send(()).unwrap();
            // Ends when the test sends or drops its end (`resume` below always does).
            let _ = go_rx.lock().unwrap().recv();
            assert!(!fail_first, "a planted scan failure");
        }
    })));
    let resume = go_tx;
    host::rescan_in_background(false);
    held_rx.recv_timeout(Duration::from_secs(60)).expect("the start-up scan reached its hold");
    assert!(host::scanning());
    during();
    assert_eq!(calls.load(Ordering::Relaxed), 1, "a request during a scan starts no second scan beside it");
    drop(resume);
    host::wait_for_scans();
    host::set_scan_hook(None);
    calls.load(Ordering::Relaxed)
}

/// #659: Scan Plug-ins clicked while the cache-reusing start-up scan runs is not dropped: after
/// that scan, every file is loaded again, including the cached failure.
#[test]
fn a_full_rescan_asked_for_during_a_scan_runs_after_it_and_rereads_every_file() {
    let t = Test::new();
    let real_error = plant_wrong_cache(&t);
    let scans = held_start_up_scan(false, || host::rescan_in_background(true));
    assert_eq!(scans, 2, "the start-up scan, then the queued rescan");
    t.assert_fixture(&host::cached().unwrap());
    let problems = host::problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(serde_json::json!(problems[0].1), real_error, "the planted entries were loaded again");
    assert!(!t.cache().iter().any(|f| f["error"].as_str() == Some("planted failure")));
}

/// Two full requests during one scan, and a cache-reusing request among them, make one rescan.
#[test]
fn repeated_requests_during_a_scan_queue_one_rescan() {
    let t = Test::new();
    let scans = held_start_up_scan(false, || {
        host::rescan_in_background(true);
        host::rescan_in_background(true);
        host::rescan_in_background(false);
    });
    assert_eq!(scans, 2, "the start-up scan and one rescan, however many requests came");
    assert!(!host::scanning());
    assert!(t.cache().is_empty());
    // A request after the thread ended starts a scan of its own, once.
    assert!(background_scan(true).is_empty());
}

/// A queued rescan is not lost when the running scan fails.
#[test]
fn a_queued_rescan_runs_even_when_the_scan_before_it_fails() {
    let _t = Test::new();
    let scans = held_start_up_scan(true, || host::rescan_in_background(true));
    assert_eq!(scans, 2, "the failed scan reached its hook, the queued rescan ran");
    assert!(host::cached().is_some(), "the rescan published");
    assert!(!host::scanning());
}

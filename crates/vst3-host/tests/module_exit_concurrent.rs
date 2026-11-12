//! #235: two plug-in main threads (host threads, ADR 0018) shut down at the same time, so one
//! thread's module exit can find the loaded-module lock held by the other. It must still exit the
//! module it entered, not skip it and leave the shutdown unclean. The hold is injected: the test
//! holds the lock (`while_module_cache_held`) until the first thread's exit has found it held
//! (`module_exits_contended`), a forced handshake rather than a timing window.
//!
//! The case runs in a child process (`harness = false`, see `module_exit.rs`): module exits are
//! process-wide and refuse later loads.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A plug-in main thread that enters the module at `dir` (its own copy of the fixture), reports
/// it, then runs its own shutdown when told to and reports that.
struct HostThread {
    go: mpsc::Sender<()>,
    done: mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

fn host_thread(dir: PathBuf, loaded: mpsc::Sender<usize>) -> HostThread {
    let (go, go_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        transit_vst3_host::adopt_main_thread();
        loaded.send(transit_vst3_host::scan_paths(&[dir]).len()).unwrap();
        go_rx.recv().unwrap();
        transit_vst3_host::shutdown();
        done_tx.send(()).unwrap();
    });
    HostThread { go, done, thread }
}

fn child(root: &Path) {
    let src = common::unregistered_fixture_dir().join("vendor/Fixture.vst3");
    let (a, b) = (root.join("a"), root.join("b"));
    copy_dir(&src, &a.join("Fixture.vst3"));
    copy_dir(&src, &b.join("Fixture.vst3"));
    let (loaded_tx, loaded) = mpsc::channel();
    let first = host_thread(a, loaded_tx.clone());
    let second = host_thread(b, loaded_tx);
    assert_eq!([loaded.recv().unwrap(), loaded.recv().unwrap()], [3, 3], "each host thread entered its own copy");
    assert_eq!(transit_vst3_host::modules_not_exited(), 2);
    // The other host thread holds the lock while the first one's shutdown starts.
    transit_vst3_host::while_module_cache_held(|| {
        first.go.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while transit_vst3_host::module_exits_contended() == 0 {
            assert!(Instant::now() < deadline, "the first host thread's module exit never reached the lock");
            std::thread::yield_now();
        }
    });
    first.done.recv().unwrap();
    assert_eq!(transit_vst3_host::modules_not_exited(), 1, "the first host thread skipped the module it entered");
    second.go.send(()).unwrap();
    second.done.recv().unwrap();
    first.thread.join().unwrap();
    second.thread.join().unwrap();
    assert_eq!(transit_vst3_host::modules_not_exited(), 0, "every entered module had its exit");
}

fn main() {
    if let Some(root) = std::env::args().nth(2).filter(|_| std::env::args().nth(1).as_deref() == Some("child")) {
        return child(Path::new(&root));
    }
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vst3-exit-concurrent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::new(std::env::current_exe().expect("test binary")).arg("child").arg(&root).output().expect("run the child");
    // The fixture copies belong to this run only, and the child has ended.
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "concurrent host-thread shutdown: {:?}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    println!("module_exit_concurrent: ok");
}

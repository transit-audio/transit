//! ADR 0064 slice HM0: the scan lists every file's classes, publishes the list, and then probes
//! each class in a child process of its own. `harness = false`: this test program is also the
//! scan program its scans start (`--scan-vst3 <file>`, `--probe-vst3 <file> <class id>`), as the
//! app is in production; a probe child logs its process id to `probe-children.log` beside the
//! file, so the test counts the children, and runs in the file's folder, where the fixture's
//! blocking class leaves `blocking.mark` once it is inside its creation.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_vst3_host as host;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, name] = args.as_slice()
        && flag == "--hold-output"
    {
        hold_output(name);
        return;
    }
    if let [flag, name] = args.as_slice()
        && flag == "--chatter"
    {
        chatter(name);
        return;
    }
    if let [flag, dir] = args.as_slice()
        && flag == "--be-parent"
    {
        be_parent(Path::new(dir));
    }
    if let Some(reply) = child(&args) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "{reply}");
        let _ = std::io::stdout().flush();
        host::shutdown();
        return;
    }
    listing_then_probing();
    probe_failures();
    probing_runs_in_the_background();
    edge_declarations_and_helpers();
    scan_children_end_with_their_parent();
    containment_failure_stops_the_child();
    println!("probe_scan: ok");
}

/// The scan program's part, as the app's `scan_child`.
fn child(args: &[String]) -> Option<String> {
    match args {
        [flag, file] if flag == host::SCAN_FLAG => Some(host::scan_reply(Path::new(file))),
        [flag, file, id] if flag == host::PROBE_FLAG => {
            let file = Path::new(file);
            let folder = file.parent().unwrap_or(Path::new("."));
            let line = format!("{} {} {id}\n", std::process::id(), file.file_name().unwrap_or_default().to_string_lossy());
            use std::io::Write as _;
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(folder.join("probe-children.log")).expect("child log");
            log.write_all(line.as_bytes()).expect("child log");
            std::env::set_current_dir(folder).expect("the file's folder");
            Some(host::probe_reply(file, id))
        }
        _ => None,
    }
}

/// The probe children of one file: (process id, class id) per child, in order.
fn children(folder: &Path, file: &str) -> Vec<(String, String)> {
    std::fs::read_to_string(folder.join("probe-children.log"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split(' ');
            let (pid, f, id) = (w.next()?, w.next()?, w.next()?);
            (f == file).then(|| (pid.to_string(), id.to_string()))
        })
        .collect()
}

/// What each probe outcome should be, from the classes' declared buses (ADR 0064, decision 2.4).
/// (name, note_output, note_input, audio_outputs).
const EXPECTED: [(&str, bool, bool, u32); 8] = [
    ("Fixture Gain", false, false, 1),
    ("Fixture Synth", false, true, 1),
    ("Fixture Events", false, true, 1),
    ("Fixture Notes", true, true, 1),
    ("Fixture Notes Silent", false, true, 1),
    ("Fixture Note Effect A", true, true, 0),
    ("Fixture Note Effect B", true, true, 0),
    ("Fixture Note Effect C", true, true, 0),
];

fn find<'a>(list: &'a [host::Vst3Descriptor], name: &str) -> &'a host::Vst3Descriptor {
    list.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name} is listed: {list:?}"))
}

/// The descriptor without its probe outcome: what the listing alone says.
fn listed(d: &host::Vst3Descriptor) -> host::Vst3Descriptor {
    host::Vst3Descriptor { probe: host::ProbeOutcome::default(), ..d.clone() }
}

/// Every class keeps exactly what its file's listing says (id, name, vendor, sub-categories,
/// category, instrument flag, file): the probe only adds its outcome.
fn assert_listing_unchanged(list: &[host::Vst3Descriptor], bundles: &[PathBuf]) {
    let mut wanted: Vec<host::Vst3Descriptor> = bundles.iter().flat_map(|b| host::load_bundle(b).expect("listing")).collect();
    wanted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut got: Vec<host::Vst3Descriptor> = list.iter().map(listed).collect();
    got.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(got, wanted);
}

fn reuse_scan() -> Vec<host::Vst3Descriptor> {
    host::rescan_in_background(false);
    host::wait_for_scans();
    host::cached().expect("published")
}

struct Probes {
    /// Class ids, in the order their probe children started.
    started: Arc<Mutex<Vec<String>>>,
    /// The published list at the first probe, before any probe answered.
    first: Arc<Mutex<Option<Vec<host::Vst3Descriptor>>>>,
    /// (`scanning()`, `probing()`) when the first probe child was about to start.
    state_at_first: Arc<Mutex<Option<(bool, bool)>>>,
    /// When each probe child was about to start: how many classes the cache file on disk held
    /// with a probe answer (`watch_cache`).
    durable: Arc<Mutex<Vec<usize>>>,
}

impl Probes {
    fn watch() -> Self {
        Probes::watch_cache(None)
    }

    fn watch_cache(cache: Option<PathBuf>) -> Self {
        let durable = Arc::new(Mutex::new(Vec::new()));
        let d = Arc::clone(&durable);
        let started = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::new(Mutex::new(None));
        let state_at_first = Arc::new(Mutex::new(None));
        let (s, f, g) = (Arc::clone(&started), Arc::clone(&first), Arc::clone(&state_at_first));
        host::set_probe_hook(Some(Arc::new(move |id: &str| {
            let mut started = s.lock().unwrap();
            if started.is_empty() {
                *f.lock().unwrap() = host::cached();
                *g.lock().unwrap() = Some((host::scanning(), host::probing()));
            }
            if let Some(cache) = &cache {
                let text = std::fs::read_to_string(cache).unwrap_or_default();
                d.lock().unwrap().push(text.matches("\"note_output\": true").count() + text.matches("\"note_output\": false").count());
            }
            started.push(id.to_string());
        })));
        Probes { started, first, state_at_first, durable }
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.started.lock().unwrap())
    }
}

fn set_up(dir: &Path) {
    host::set_scan_extra_dirs_only(true);
    host::set_extra_dirs(vec![dir.to_path_buf()]);
    host::set_scanner(Some(std::env::current_exe().expect("this program")));
    host::set_cache_file(Some(dir.join("cache.json")));
}

fn tear_down() {
    host::set_probe_hook(None);
    host::set_probe_answered_hook(None);
    host::set_probe_deadline(None);
    host::set_contain_fails(None);
    host::set_watch_fails(None);
    host::set_scanner(None);
    host::set_cache_file(None);
    host::set_extra_dirs(Vec::new());
}

fn listing_then_probing() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-scan");
    let bundles = [
        common::install_named(&[], "vst3-fixture-target", &dir, "Fixture"),
        common::install_named(&["--features", "notes"], "vst3-notes-fixture-target", &dir, "Notes"),
        common::install_named(&["--features", "note-effects"], "vst3-note-effects-fixture-target", &dir, "NoteEffects"),
    ];
    let vendor = dir.join("vendor");
    set_up(&dir);
    // A cache file of the version before probe outcomes, current for the Notes file (its binary's
    // size and time) but naming a class that file does not hold: read, it would be listed.
    let notes = &bundles[1];
    let binary = host::scan::bundle_binary(notes).expect("the Notes binary");
    let meta = std::fs::metadata(&binary).unwrap();
    let modified_ns = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    let fake = host::Vst3Descriptor { name: "Stale Notes".into(), ..host::load_bundle(notes).expect("listing")[0].clone() };
    let stale = serde_json::json!({ "version": 1, "files": [{
        "path": notes, "size": meta.len(), "modified_ns": modified_ns, "plugins": [fake], "error": null, "host": ""
    }] });
    std::fs::write(dir.join("cache.json"), stale.to_string()).unwrap();
    let probes = Probes::watch_cache(Some(dir.join("cache.json")));

    let list = reuse_scan();

    // The listing was published before the first probe child started: every class, none probed.
    let first = probes.first.lock().unwrap().clone().expect("a listing was published before the first probe");
    assert_eq!(first.len(), EXPECTED.len(), "{first:?}");
    for (name, ..) in EXPECTED {
        assert_eq!(find(&first, name).probe, host::ProbeOutcome::default(), "{name} was not probed yet when the listing was published");
    }
    // The listing scan was over (what the splash and the menus wait for) and probing had begun.
    assert_eq!(*probes.state_at_first.lock().unwrap(), Some((false, true)), "(scanning, probing) at the first probe child");
    // Every outcome was on disk before the next child started: quitting during a pass keeps what
    // it found (the cache file names no answer for a class not probed yet).
    assert_eq!(
        std::mem::take(&mut *probes.durable.lock().unwrap()),
        (0..EXPECTED.len()).collect::<Vec<_>>(),
        "answers on disk at each child's start"
    );
    // One probe child per class.
    let started = probes.take();
    assert_eq!(started.len(), EXPECTED.len(), "{started:?}");
    for (name, note_output, note_input, audio_outputs) in EXPECTED {
        let d = find(&list, name);
        let p = &d.probe;
        assert_eq!((p.note_output, p.note_input, p.audio_outputs), (Some(note_output), Some(note_input), Some(audio_outputs)), "{name}");
        assert_eq!(p.note_output_index, note_output.then_some(0), "{name}: VST3 reads event output bus 0");
        assert_eq!((p.probe_error.as_deref(), p.probe_host.as_deref()), (None, None), "{name}");
        assert_eq!(started.iter().filter(|id| **id == d.class_id).count(), 1, "{name}: one probe");
    }
    // The categories the record names stay as the listing gives them.
    for name in ["Fixture Notes", "Fixture Notes Silent", "Fixture Note Effect A", "Fixture Note Effect B", "Fixture Note Effect C", "Fixture Synth"]
    {
        let d = find(&list, name);
        assert_eq!((d.category, d.is_instrument), (transit_dsp::Category::Instrument, true), "{name}");
    }
    assert_eq!((find(&list, "Fixture Gain").category, find(&list, "Fixture Gain").is_instrument), (transit_dsp::Category::Other, false));
    assert_listing_unchanged(&list, &bundles);
    // The zero-audio bundle: three classes, three children, each class once.
    let effects = children(&vendor, "NoteEffects.vst3");
    assert_eq!(effects.len(), 3, "{effects:?}");
    let mut pids: Vec<&String> = effects.iter().map(|(p, _)| p).collect();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), 3, "a child process of its own per class: {effects:?}");
    let mut ids: Vec<&String> = effects.iter().map(|(_, id)| id).collect();
    ids.sort();
    let mut wanted: Vec<String> = ["A", "B", "C"].iter().map(|x| find(&list, &format!("Fixture Note Effect {x}")).class_id.clone()).collect();
    wanted.sort();
    assert_eq!(ids, wanted.iter().collect::<Vec<_>>());
    // The version 1 file was ignored and rewritten as version 2, with the outcomes.
    assert!(list.iter().all(|d| d.name != "Stale Notes"), "the version 1 file was not read: {list:?}");
    let cache: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("cache.json")).unwrap()).unwrap();
    assert_eq!(cache["version"], 2);
    let text = cache.to_string();
    assert!(text.contains("\"note_output\":true") && text.contains("\"audio_outputs\":0"), "{text}");

    // A later scan reuses every answered probe; the explicit rescan probes every class again.
    let again = reuse_scan();
    assert_eq!(probes.take(), Vec::<String>::new(), "answered probes are reused");
    assert_eq!(
        again.iter().map(|d| (d.id.clone(), d.probe.clone())).collect::<Vec<_>>(),
        list.iter().map(|d| (d.id.clone(), d.probe.clone())).collect::<Vec<_>>()
    );
    let listed_again = host::rescan();
    assert!(
        listed_again.iter().all(|d| d.probe == host::ProbeOutcome::default()),
        "the explicit rescan returns its listing without waiting for probes"
    );
    host::wait_for_scans();
    let rescanned = host::cached().expect("published");
    assert_eq!(probes.take().len(), EXPECTED.len(), "the explicit rescan probes every class");
    assert_eq!(find(&rescanned, "Fixture Notes").probe.note_output, Some(true));
    tear_down();
}

/// The probe deadline of the failure file. It passes once the blocking class has written its mark
/// (its probe child is inside the class's creation), never before, so no probe is cut short by the
/// clock; its budget, as the reason gives it, is the shortened 1 s. With `hold` set, the first
/// blocking child it sees is held instead (its deadline passes only after `release`) and `held` is
/// told.
struct Held {
    mark: PathBuf,
    seen: bool,
    holding: bool,
    gate: Arc<Gate>,
}

#[derive(Default)]
struct Gate {
    /// Deadlines made (one per probe child).
    made: AtomicUsize,
    /// Blocking marks seen.
    marks: AtomicUsize,
    hold: AtomicBool,
    release: AtomicBool,
    held: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl Gate {
    fn install(mark: PathBuf) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        let g = Arc::clone(&gate);
        host::set_probe_deadline(Some(Arc::new(move |_: &str| {
            g.made.fetch_add(1, Ordering::SeqCst);
            Box::new(Held { mark: mark.clone(), seen: false, holding: false, gate: Arc::clone(&g) }) as Box<dyn host::Deadline>
        })));
        gate
    }
}

impl host::Deadline for Held {
    fn passed(&mut self) -> bool {
        if !self.seen && std::fs::remove_file(&self.mark).is_ok() {
            self.seen = true;
            self.gate.marks.fetch_add(1, Ordering::SeqCst);
            self.holding = self.gate.hold.swap(false, Ordering::SeqCst);
            if self.holding
                && let Some(tx) = self.gate.held.lock().unwrap().take()
            {
                let _ = tx.send(());
            }
        }
        self.seen && (!self.holding || self.gate.release.load(Ordering::SeqCst))
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(1)
    }
}

const FAILING: [(&str, &str); 3] = [
    ("Fixture Probe Error", "factory cannot create the component"),
    ("Fixture Probe Crash", "ended the scan process"),
    ("Fixture Probe Block", "still loading after 1 s; stopped"),
];

fn probe_failures() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-failures");
    let bundle = common::install_named(&["--features", "probe-failures"], "vst3-probe-failures-fixture-target", &dir, "Probes");
    let vendor = dir.join("vendor");
    set_up(&dir);
    let gate = Gate::install(vendor.join("blocking.mark"));
    let probes = Probes::watch();

    let list = reuse_scan();

    assert_eq!(probes.take().len(), 5, "one probe per class");
    assert_eq!(gate.marks.load(Ordering::SeqCst), 1, "the blocking class's child was inside its creation when its deadline passed");
    assert_eq!(list.len(), 5, "every class keeps its listing: {list:?}");
    assert_listing_unchanged(&list, std::slice::from_ref(&bundle));
    for (name, reason) in FAILING {
        let p = &find(&list, name).probe;
        assert_eq!((p.note_output, p.note_input, p.audio_outputs, p.note_output_index), (None, None, None, None), "{name}");
        let error = p.probe_error.clone().unwrap_or_default();
        assert!(error.contains(reason), "{name}: {error}");
        assert!(p.probe_host.as_deref().is_some_and(|h| h.starts_with(env!("CARGO_PKG_VERSION"))), "{name}: the build that tried it: {p:?}");
    }
    assert_eq!(find(&list, "Fixture Notes").probe.note_output, Some(true), "the class before the failures has its own outcome");
    assert_eq!(find(&list, "Fixture Notes Silent").probe.note_output, Some(false), "the class after the failures has its own outcome");
    assert_eq!(find(&list, "Fixture Probe Crash").category, transit_dsp::Category::Instrument, "a failed probe keeps the category");
    // The file itself loaded: no file error, nothing in the file problems.
    assert_eq!(host::problems(), Vec::new());
    assert_eq!(host::scan_summary().map(|s| (s.loaded, s.failed)), Some((1, 0)));
    // The diagnostics report's data: the three failed probes with their reasons.
    let problems = host::probe_problems();
    let mut named: Vec<&str> = problems.iter().map(|p| p.name.as_str()).collect();
    named.sort();
    assert_eq!(named, ["Fixture Probe Block", "Fixture Probe Crash", "Fixture Probe Error"]);
    assert!(problems.iter().all(|p| p.path == bundle && !p.reason.is_empty() && !p.host.is_empty()), "{problems:?}");
    assert_eq!(children(&vendor, "Probes.vst3").len(), 5);

    // The same build reuses the failures as well as the answers.
    let same = reuse_scan();
    assert_eq!(probes.take(), Vec::<String>::new(), "the same build does not probe again");
    assert!(find(&same, "Fixture Probe Error").probe.probe_error.is_some());
    // Another build (the cache names another build for the failures): only the failures are probed.
    let cache = std::fs::read_to_string(dir.join("cache.json")).unwrap();
    let host_field = format!("\"probe_host\": \"{}\"", find(&same, "Fixture Probe Error").probe.probe_host.clone().unwrap_or_default());
    assert_eq!(cache.matches(&host_field).count(), 3, "{cache}");
    std::fs::write(dir.join("cache.json"), cache.replace(&host_field, "\"probe_host\": \"another build\"")).unwrap();
    reuse_scan();
    let mut retried: Vec<String> = probes.take();
    retried.sort();
    let mut failing: Vec<String> = FAILING.iter().map(|(n, _)| find(&list, n).class_id.clone()).collect();
    failing.sort();
    assert_eq!(retried, failing, "another build probes the failed classes again, never the answered ones");
    // The explicit rescan probes every class again.
    host::rescan();
    host::wait_for_scans();
    assert_eq!(probes.take().len(), 5);
    assert_eq!(gate.made.load(Ordering::SeqCst), 13, "each probe child had its own deadline");
    tear_down();
}

/// Probing is background work (ADR 0064, decision 2.4). While a probe child is held inside the
/// blocking class's creation: the listing is published, `scanning()` is over, the scan lock is free
/// and a scan returns at once. An explicit rescan request then stops the pass, and probing starts
/// again on the new listing. A listing published while a child runs keeps that child's outcome out
/// of the new list: the new listing's pass probes the class itself.
fn probing_runs_in_the_background() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-background");
    let bundle = common::install_named(&["--features", "probe-failures"], "vst3-probe-failures-fixture-target", &dir, "Probes");
    let vendor = dir.join("vendor");
    set_up(&dir);
    let gate = Gate::install(vendor.join("blocking.mark"));
    let (tx, rx) = std::sync::mpsc::channel();
    *gate.held.lock().unwrap() = Some(tx);
    gate.hold.store(true, Ordering::SeqCst);
    let probes = Probes::watch();
    // In file order: Notes, Error, Crash, Block, Notes Silent.
    let ids: Vec<String> = host::load_bundle(&bundle).expect("listing").iter().map(|d| d.class_id.clone()).collect();
    let block = ids[3].clone();
    let up_to_block: Vec<String> = ids[..4].to_vec();

    host::rescan_in_background(false);
    rx.recv().expect("the blocking class's probe child is held inside its creation");
    assert!(!host::scanning(), "the scan ended with the listing: the splash and \"Scanning...\" do not wait for probes");
    assert!(host::probing());
    assert!(host::scan_lock_is_free(), "a probe pass never holds the scan lock");
    assert_eq!(host::scan().len(), 5, "a scan (as a plug-in creation does for an unknown id) returns the published listing at once");
    let now = host::cached().expect("published");
    assert_eq!(find(&now, "Fixture Notes").probe.note_output, Some(true), "outcomes are published while probing goes on");
    assert_eq!(find(&now, "Fixture Probe Block").probe, host::ProbeOutcome::default(), "the held class has no outcome yet");

    // An explicit rescan while probing: the pass stops at the held child, and the new listing is
    // probed whole.
    host::rescan_in_background(true);
    gate.release.store(true, Ordering::SeqCst);
    host::wait_for_scans();
    assert_eq!(
        probes.take(),
        [up_to_block.clone(), ids.clone()].concat(),
        "the stopped pass ended at the held class; the new listing's pass probed every class"
    );
    let after = host::cached().expect("published");
    assert_eq!(find(&after, "Fixture Notes").probe.note_output, Some(true));
    assert_eq!(find(&after, "Fixture Notes Silent").probe.note_output, Some(false));
    assert!(find(&after, "Fixture Probe Block").probe.probe_error.as_deref().is_some_and(|e| e.contains("still loading")));

    // A listing published (a rescan) right after the blocking class's child answered, before its
    // outcome is published: that outcome stays out of the new list, the old pass stops, and the
    // new listing's pass probes the blocking class again.
    let once = Arc::new(AtomicBool::new(false));
    {
        let (once, block) = (Arc::clone(&once), block.clone());
        host::set_probe_answered_hook(Some(Arc::new(move |id: &str| {
            if id == block && !once.swap(true, Ordering::SeqCst) {
                host::rescan();
            }
        })));
    }
    host::rescan_in_background(true);
    host::wait_for_scans();
    host::set_probe_answered_hook(None);
    assert!(once.load(Ordering::SeqCst));
    assert_eq!(probes.take(), [up_to_block, ids].concat(), "the old pass stopped after the blocking class; the new listing's pass probed it again");
    let last = host::cached().expect("published");
    assert_eq!(find(&last, "Fixture Notes Silent").probe.note_output, Some(false));
    assert!(find(&last, "Fixture Probe Block").probe.probe_error.is_some());
    tear_down();
}

/// `--hold-output <name>`: the helper a fixture class starts. Locks `<name>.lock` (in the working
/// folder) for as long as it lives, writes its process id to `<name>.ready` once it holds the lock,
/// and sleeps; it ends on its own after 300 s, writing `<name>.done`, so a broken containment fails
/// a test (instead of passing it late) and cannot leave it for good.
fn hold_output(name: &str) {
    let lock = std::fs::File::create(format!("{name}.lock")).and_then(|f| f.lock().map(|()| f)).expect("helper lock");
    std::fs::write(format!("{name}.ready"), std::process::id().to_string()).expect("helper ready");
    std::thread::sleep(Duration::from_secs(300));
    let _ = std::fs::write(format!("{name}.done"), b"ended by itself");
    drop(lock);
}

/// `--be-parent <dir>`: a scan host of its own over `<dir>` (the probe failures file). It says
/// `BLOCKED` on its standard output once its probe child is held inside the blocking class's
/// creation, and then waits to be killed.
fn be_parent(dir: &Path) -> ! {
    set_up(dir);
    let gate = Gate::install(dir.join("vendor").join("blocking.mark"));
    let (tx, rx) = std::sync::mpsc::channel();
    *gate.held.lock().unwrap() = Some(tx);
    gate.hold.store(true, Ordering::SeqCst);
    host::rescan_in_background(false);
    rx.recv().expect("held");
    use std::io::Write as _;
    println!("BLOCKED");
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Waits until no running process holds the lock of `file` (the lock goes with its holder).
fn wait_until_unlocked(file: &Path) {
    let f = std::fs::File::open(file).expect("lock file");
    f.lock().expect("lock");
}

/// Whether a running process holds the lock of `file`.
fn locked(file: &Path) -> bool {
    let f = std::fs::File::open(file).expect("lock file");
    match f.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(e) => panic!("{e}"),
    }
}

/// The listing scan keeps working past declarations at the edges, and a probe child's helper
/// processes neither outlive the probe nor hold the pass up (findings 1 to 4 of the review).
fn edge_declarations_and_helpers() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-edges");
    common::install_named(&["--features", "edges"], "vst3-edges-fixture-target", &dir, "Edges");
    let vendor = dir.join("vendor");
    set_up(&dir);
    let gate = Gate::install(vendor.join("blocking.mark"));
    let probes = Probes::watch();
    let list = reuse_scan();
    let started = probes.take();
    assert_eq!(started.len(), list.len(), "one probe per class: {started:?}");
    let p = |name: &str| find(&list, name).probe.clone();
    let error = |name: &str| p(name).probe_error.unwrap_or_default();
    let b17 = p("Fixture Buses 17");
    assert_eq!((b17.note_output, b17.audio_outputs), (Some(true), Some(17)), "a count past the host's buffers is recorded as declared");
    assert!(error("Fixture Buses Negative").contains("declares -1 event output buses"), "{}", error("Fixture Buses Negative"));
    assert_eq!(p("Fixture Buses Negative").note_output, None, "a negative count is no answer");
    // The helper held the probe child's output pipe: the child's deadline still ended the probe,
    // the helper ended with its tree, and the pass went on.
    assert!(vendor.join("helper.ready").exists(), "the helper ran");
    assert!(error("Fixture Probe Helper").contains("still loading"), "{}", error("Fixture Probe Helper"));
    wait_until_unlocked(&vendor.join("helper.lock"));
    assert!(!vendor.join("helper.done").exists(), "the helper was killed with the probe child's tree; it did not end by itself");
    assert_eq!(p("Fixture Notes").note_output, Some(true), "the class after the helpers was probed");
    #[cfg(unix)]
    {
        // A helper that left the group on purpose is out of reach (a stated limit), but it no
        // longer holds the pass: collection ended at the deadline.
        assert!(error("Fixture Probe Escape").contains("still loading"), "{}", error("Fixture Probe Escape"));
        let escape = vendor.join("escape.lock");
        let alive = locked(&escape);
        let pid = std::fs::read_to_string(vendor.join("escape.ready")).unwrap_or_default();
        let _ = std::process::Command::new("kill").args(["-KILL", pid.trim()]).status();
        wait_until_unlocked(&escape);
        assert!(alive, "the helper that left the group outlived its probe (the stated limit)");
    }
    // Review round 2, finding A: a helper of a child that ends by itself is killed with the
    // child's tree, before the child is reaped (on Linux and macOS a kill after the reap signals
    // nothing, and the helper would end only by itself, writing `reaped.done`).
    assert!(error("Fixture Probe Helper Crash").contains("ended the scan process"), "{}", error("Fixture Probe Helper Crash"));
    wait_until_unlocked(&vendor.join("reaped.lock"));
    assert!(!vendor.join("reaped.done").exists(), "the crashed child's helper was killed with its tree; it did not end by itself");
    // Finding C: a helper that keeps writing to the probe child's output pipe (in a group of its
    // own on Linux and macOS, so only the closing of the pipe can end it there) ends once the
    // probe is over: nothing in this process reads the pipe or holds it open any more.
    assert!(error("Fixture Probe Chatter").contains("still loading"), "{}", error("Fixture Probe Chatter"));
    wait_until_unlocked(&vendor.join("chatter.lock"));
    assert!(!vendor.join("chatter.done").exists(), "the chattering helper's writes failed once the pipe was closed; it did not end by itself");
    // A 5 MiB line is an error at the bound, not 5 MiB kept.
    assert!(error("Fixture Probe Long Line").contains("longer than 4194304 bytes"), "{}", error("Fixture Probe Long Line"));
    assert_eq!(gate.marks.load(Ordering::SeqCst), if cfg!(unix) { 3 } else { 2 });
    tear_down();
}

/// A scan child ends with its parent, however the parent ends: here it is killed outright while
/// its probe child is held inside the blocking class's creation (finding 5). The child holds
/// `blocking.lock` for as long as it lives.
fn scan_children_end_with_their_parent() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-parent");
    common::install_named(&["--features", "probe-failures"], "vst3-probe-failures-fixture-target", &dir, "Probes");
    let mut parent = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--be-parent")
        .arg(&*dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(parent.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        use std::io::BufRead as _;
        assert!(out.read_line(&mut line).unwrap() > 0, "the parent ended before its child was held");
        if line.contains("BLOCKED") {
            break;
        }
    }
    let lock = dir.join("vendor").join("blocking.lock");
    assert!(locked(&lock), "the probe child holds its lock while it is held");
    parent.kill().unwrap();
    parent.wait().unwrap();
    wait_until_unlocked(&lock);
    assert!(!dir.join("vendor").join("blocking.done").exists(), "the probe child was ended with its parent; it did not end by itself");
}

/// `--chatter <name>`: as `--hold-output`, and it writes a line to its standard output every
/// 10 ms until a write fails, which ends it at once (without `<name>.done`).
fn chatter(name: &str) {
    let lock = std::fs::File::create(format!("{name}.lock")).and_then(|f| f.lock().map(|()| f)).expect("chatter lock");
    std::fs::write(format!("{name}.ready"), std::process::id().to_string()).expect("chatter ready");
    std::thread::spawn(|| {
        use std::io::Write as _;
        loop {
            let mut out = std::io::stdout();
            if writeln!(out, "chatter").and_then(|()| out.flush()).is_err() {
                std::process::exit(0);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    std::thread::sleep(Duration::from_secs(300));
    let _ = std::fs::write(format!("{name}.done"), b"ended by itself");
    drop(lock);
}

/// Review round 2, finding B: a scan child that cannot be contained, or whose parent watch cannot
/// start, loads nothing. A listing child's failure is that file's scan error; a probe child's
/// keeps the listing and records the reason as the probe's outcome. Each class creation is logged
/// to `created.log` by the fixture: no line means nothing was loaded.
fn containment_failure_stops_the_child() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "vst3-probe-uncontained");
    let bundle = common::install_named(&["--features", "notes"], "vst3-notes-fixture-target", &dir, "Notes");
    let vendor = dir.join("vendor");
    let created = vendor.join("created.log");
    set_up(&dir);
    let listing_fails = |what: &str, reason: &str| {
        host::rescan();
        host::wait_for_scans();
        let problems = host::problems();
        assert_eq!(problems.len(), 1, "{what}: {problems:?}");
        assert!(problems[0].0 == bundle && problems[0].1.contains(reason), "{what}: {problems:?}");
        assert!(host::cached().unwrap_or_default().is_empty(), "{what}: the file lists nothing");
    };
    let probes_fail = |what: &str, reason: &str| {
        host::rescan();
        host::wait_for_scans();
        let list = host::cached().expect("published");
        assert_eq!(list.len(), 2, "{what}: the listing stands: {list:?}");
        for d in &list {
            let p = &d.probe;
            assert_eq!(p.note_output, None, "{what}: {}", d.name);
            assert!(p.probe_error.as_deref().is_some_and(|e| e.contains(reason)), "{what}: {}: {p:?}", d.name);
        }
    };
    host::set_contain_fails(Some(host::SCAN_FLAG));
    listing_fails("a listing child that cannot be contained", "could not be contained");
    host::set_contain_fails(Some(host::PROBE_FLAG));
    probes_fail("a probe child that cannot be contained", "could not be contained");
    host::set_contain_fails(None);
    host::set_watch_fails(Some(host::SCAN_FLAG));
    listing_fails("a listing child whose parent watch cannot start", "cannot watch its parent");
    host::set_watch_fails(Some(host::PROBE_FLAG));
    probes_fail("a probe child whose parent watch cannot start", "cannot watch its parent");
    host::set_watch_fails(None);
    assert!(!created.exists(), "no class was created: {}", std::fs::read_to_string(&created).unwrap_or_default());
    // And once containment works again, the same scan creates and probes them.
    host::rescan();
    host::wait_for_scans();
    assert_eq!(find(&host::cached().expect("published"), "Fixture Notes").probe.note_output, Some(true));
    assert!(created.exists(), "the classs were created this time");
    tear_down();
}

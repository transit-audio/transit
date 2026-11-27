//! ADR 0064 slice HM0: the scan lists every file's plug-ins, publishes the list, and then probes
//! each plug-in in a child process of its own. `harness = false`: this test program is also the
//! scan program its scans start (`--scan-clap <file>`, `--probe-clap <file> <plugin id>`), as the
//! app is in production; a probe child logs its process id to `probe-children.log` beside the
//! file, so the test counts the children, and runs in the file's folder, where the fixture's
//! blocking plug-in leaves `blocking.mark` once it is inside its creation.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_clap_host as host;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(reply) = child(&args) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "{reply}");
        let _ = std::io::stdout().flush();
        return;
    }
    listing_then_probing();
    probe_failures();
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

/// The probe children of one file: (process id, plug-in id) per child, in order.
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

/// What each probe outcome should be, from the plug-ins' declared ports (ADR 0064, decisions 2.1
/// and 2.4): (name, note_output, first readable output port, note_input, audio_outputs).
const EXPECTED: [(&str, bool, Option<u32>, bool, u32); 7] = [
    ("Fixture Gain", false, None, false, 1),
    ("Fixture Synth", false, None, true, 1),
    // Output port 0 speaks MIDI 2 only; port 1 CLAP and MIDI is the first readable one.
    ("Fixture Notes", true, Some(1), true, 1),
    // Its only output port speaks MIDI 2 only.
    ("Fixture Notes Silent", false, None, true, 1),
    ("Fixture Note Effect A", true, Some(0), true, 0),
    ("Fixture Note Effect B", true, Some(0), true, 0),
    ("Fixture Note Effect C", true, Some(0), true, 0),
];

fn find<'a>(list: &'a [host::ClapDescriptor], name: &str) -> &'a host::ClapDescriptor {
    list.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name} is listed: {list:?}"))
}

/// The descriptor without its probe outcome: what the listing alone says.
fn listed(d: &host::ClapDescriptor) -> host::ClapDescriptor {
    host::ClapDescriptor { probe: host::ProbeOutcome::default(), ..d.clone() }
}

/// Every plug-in keeps exactly what its file's listing says (id, name, vendor, features,
/// category, instrument flag, file): the probe only adds its outcome.
fn assert_listing_unchanged(list: &[host::ClapDescriptor], bundles: &[PathBuf]) {
    let mut wanted: Vec<host::ClapDescriptor> = bundles.iter().flat_map(|b| host::load_bundle(b).expect("listing")).collect();
    wanted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut got: Vec<host::ClapDescriptor> = list.iter().map(listed).collect();
    got.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(got, wanted);
}

fn reuse_scan() -> Vec<host::ClapDescriptor> {
    host::rescan_in_background(false);
    host::wait_for_scans();
    host::cached().expect("published")
}

struct Probes {
    /// Plug-in ids, in the order their probe children started.
    started: Arc<Mutex<Vec<String>>>,
    /// The published list at the first probe, before any probe answered.
    first: Arc<Mutex<Option<Vec<host::ClapDescriptor>>>>,
}

impl Probes {
    fn watch() -> Self {
        let started = Arc::new(Mutex::new(Vec::new()));
        let first = Arc::new(Mutex::new(None));
        let (s, f) = (Arc::clone(&started), Arc::clone(&first));
        host::set_probe_hook(Some(Arc::new(move |id: &str| {
            let mut started = s.lock().unwrap();
            if started.is_empty() {
                *f.lock().unwrap() = host::cached();
            }
            started.push(id.to_string());
        })));
        Probes { started, first }
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
    host::set_probe_deadline(None);
    host::set_scanner(None);
    host::set_cache_file(None);
    host::set_extra_dirs(Vec::new());
}

fn listing_then_probing() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "clap-probe-scan");
    let bundles = [
        common::install_named(&[], "clap-fixture-target", &dir, "Fixture"),
        common::install_named(&["--features", "notes"], "clap-notes-fixture-target", &dir, "Notes"),
        common::install_named(&["--features", "note-effects"], "clap-note-effects-fixture-target", &dir, "NoteEffects"),
    ];
    let vendor = dir.join("vendor");
    set_up(&dir);
    // A cache file of the version before probe outcomes, current for the Notes file (its
    // executable's size and time) but naming a plug-in that file does not hold: read, it would be
    // listed.
    let notes = &bundles[1];
    let exe = host::scan::bundle_executable(notes).expect("the Notes executable");
    let meta = std::fs::metadata(&exe).unwrap();
    let modified_ns = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    let fake = host::ClapDescriptor { name: "Stale Notes".into(), ..host::load_bundle(notes).expect("listing")[0].clone() };
    let stale = serde_json::json!({ "version": 1, "files": [{
        "path": notes, "size": meta.len(), "modified_ns": modified_ns, "plugins": [fake], "error": null, "host": ""
    }] });
    std::fs::write(dir.join("cache.json"), stale.to_string()).unwrap();
    let probes = Probes::watch();

    let list = reuse_scan();

    // The listing was published before the first probe child started: every plug-in, none probed.
    let first = probes.first.lock().unwrap().clone().expect("a listing was published before the first probe");
    assert_eq!(first.len(), EXPECTED.len(), "{first:?}");
    for (name, ..) in EXPECTED {
        assert_eq!(find(&first, name).probe, host::ProbeOutcome::default(), "{name} was not probed yet when the listing was published");
    }
    // One probe child per plug-in.
    let started = probes.take();
    assert_eq!(started.len(), EXPECTED.len(), "{started:?}");
    for (name, note_output, index, note_input, audio_outputs) in EXPECTED {
        let d = find(&list, name);
        let p = &d.probe;
        assert_eq!(
            (p.note_output, p.note_output_index, p.note_input, p.audio_outputs),
            (Some(note_output), index, Some(note_input), Some(audio_outputs)),
            "{name}"
        );
        assert_eq!((p.probe_error.as_deref(), p.probe_host.as_deref()), (None, None), "{name}");
        assert_eq!(started.iter().filter(|id| **id == d.plugin_id).count(), 1, "{name}: one probe");
    }
    // The categories the features give stay as the listing gives them.
    for name in ["Fixture Notes", "Fixture Notes Silent", "Fixture Synth"] {
        let d = find(&list, name);
        assert_eq!((d.category, d.is_instrument), (transit_dsp::Category::Instrument, true), "{name}");
    }
    for name in ["Fixture Note Effect A", "Fixture Note Effect B", "Fixture Note Effect C"] {
        let d = find(&list, name);
        assert_eq!((d.category, d.is_instrument), (transit_dsp::Category::Other, false), "{name}: `note-effect` is no instrument");
    }
    assert_listing_unchanged(&list, &bundles);
    // The zero-audio file: three plug-ins, three children, each plug-in once.
    let effects = children(&vendor, "NoteEffects.clap");
    assert_eq!(effects.len(), 3, "{effects:?}");
    let mut pids: Vec<&String> = effects.iter().map(|(p, _)| p).collect();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), 3, "a child process of its own per plug-in: {effects:?}");
    let mut ids: Vec<&String> = effects.iter().map(|(_, id)| id).collect();
    ids.sort();
    let mut wanted: Vec<String> = ["A", "B", "C"].iter().map(|x| find(&list, &format!("Fixture Note Effect {x}")).plugin_id.clone()).collect();
    wanted.sort();
    assert_eq!(ids, wanted.iter().collect::<Vec<_>>());
    // The version 1 file was ignored and rewritten as version 2, with the outcomes.
    assert!(list.iter().all(|d| d.name != "Stale Notes"), "the version 1 file was not read: {list:?}");
    let cache: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("cache.json")).unwrap()).unwrap();
    assert_eq!(cache["version"], 2);
    let text = cache.to_string();
    assert!(text.contains("\"note_output\":true") && text.contains("\"audio_outputs\":0"), "{text}");

    // A later scan reuses every answered probe; the explicit rescan probes every plug-in again.
    let again = reuse_scan();
    assert_eq!(probes.take(), Vec::<String>::new(), "answered probes are reused");
    assert_eq!(
        again.iter().map(|d| (d.id.clone(), d.probe.clone())).collect::<Vec<_>>(),
        list.iter().map(|d| (d.id.clone(), d.probe.clone())).collect::<Vec<_>>()
    );
    let rescanned = host::rescan();
    assert_eq!(probes.take().len(), EXPECTED.len(), "the explicit rescan probes every plug-in");
    assert_eq!(find(&rescanned, "Fixture Notes").probe.note_output, Some(true));
    tear_down();
}

/// The probe deadline of the failure file: it passes once the blocking plug-in has written its
/// mark (its probe child is inside the plug-in's creation), never before, so no plug-in's probe is
/// cut short by the clock. Its budget, as the reason gives it, is the shortened 1 s.
struct UntilBlocked {
    mark: PathBuf,
    seen: Arc<AtomicBool>,
}

impl host::Deadline for UntilBlocked {
    fn passed(&mut self) -> bool {
        if std::fs::remove_file(&self.mark).is_ok() {
            self.seen.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(1)
    }
}

const FAILING: [(&str, &str); 3] = [
    ("Fixture Probe Error", "factory returned no instance"),
    ("Fixture Probe Crash", "ended the scan process"),
    ("Fixture Probe Block", "still loading after 1 s; stopped"),
];

fn probe_failures() {
    let dir = transit_test_support::TestDir::created_in(Path::new(env!("CARGO_TARGET_TMPDIR")), "clap-probe-failures");
    let bundle = common::install_named(&["--features", "probe-failures"], "clap-probe-failures-fixture-target", &dir, "Probes");
    let vendor = dir.join("vendor");
    set_up(&dir);
    let seen = Arc::new(AtomicBool::new(false));
    let deadlines = Arc::new(AtomicUsize::new(0));
    {
        let (seen, deadlines, mark) = (Arc::clone(&seen), Arc::clone(&deadlines), vendor.join("blocking.mark"));
        host::set_probe_deadline(Some(Arc::new(move |_: &str| {
            deadlines.fetch_add(1, Ordering::SeqCst);
            Box::new(UntilBlocked { mark: mark.clone(), seen: Arc::clone(&seen) }) as Box<dyn host::Deadline>
        })));
    }
    let probes = Probes::watch();

    let list = reuse_scan();

    assert_eq!(probes.take().len(), 5, "one probe per plug-in");
    assert!(seen.load(Ordering::SeqCst), "the blocking plug-in's child was inside its creation when its deadline passed");
    assert_eq!(list.len(), 5, "every plug-in keeps its listing: {list:?}");
    assert_listing_unchanged(&list, std::slice::from_ref(&bundle));
    for (name, reason) in FAILING {
        let p = &find(&list, name).probe;
        assert_eq!((p.note_output, p.note_input, p.audio_outputs, p.note_output_index), (None, None, None, None), "{name}");
        let error = p.probe_error.clone().unwrap_or_default();
        assert!(error.contains(reason), "{name}: {error}");
        assert!(p.probe_host.as_deref().is_some_and(|h| h.starts_with(env!("CARGO_PKG_VERSION"))), "{name}: the build that tried it: {p:?}");
    }
    assert_eq!(find(&list, "Fixture Notes").probe.note_output, Some(true), "the plug-in before the failures has its own outcome");
    assert_eq!(find(&list, "Fixture Notes Silent").probe.note_output, Some(false), "the plug-in after the failures has its own outcome");
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
    assert_eq!(children(&vendor, "Probes.clap").len(), 5);

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
    let mut failing: Vec<String> = FAILING.iter().map(|(n, _)| find(&list, n).plugin_id.clone()).collect();
    failing.sort();
    assert_eq!(retried, failing, "another build probes the failed plug-ins again, never the answered ones");
    // The explicit rescan probes every plug-in again.
    host::rescan();
    assert_eq!(probes.take().len(), 5);
    assert!(deadlines.load(Ordering::SeqCst) >= 13, "each probe child had its own deadline");
    tear_down();
}

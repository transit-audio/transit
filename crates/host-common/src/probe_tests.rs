use super::*;
use crate::scan_cache::{Deadline, entry_for_test, probe_in_child};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
struct Class {
    key: String,
    category: &'static str,
    probe: ProbeOutcome,
}

impl Probed for Class {
    fn outcome(&self) -> &ProbeOutcome {
        &self.probe
    }
    fn outcome_mut(&mut self) -> &mut ProbeOutcome {
        &mut self.probe
    }
}

fn class(key: &str) -> Class {
    Class { key: key.into(), category: "Instrument", probe: ProbeOutcome::default() }
}

const NOTES: Capability = Capability { note_output: true, note_input: true, audio_outputs: 0, first_note_output: Some(0) };

/// The file holds a good class, one whose probe fails and another good one; a file that did not
/// load has none to probe.
fn files() -> Vec<Entry<Class>> {
    vec![
        entry_for_test(PathBuf::from("a.plug"), vec![class("good"), class("bad"), class("other")], None),
        entry_for_test(PathBuf::from("broken.plug"), Vec::new(), Some("refused".into())),
    ]
}

fn probe_with(asked: &std::cell::RefCell<Vec<String>>) -> impl FnMut(&Path, &Class) -> Option<Result<Capability, String>> + '_ {
    |_, c| {
        asked.borrow_mut().push(c.key.clone());
        Some(if c.key == "bad" { Err("bad: the plug-in ended the scan process".into()) } else { Ok(NOTES) })
    }
}

#[test]
fn a_failed_probe_marks_only_its_class_and_keeps_every_listing() {
    let mut files = files();
    let asked = std::cell::RefCell::new(Vec::new());
    let mut published = Vec::new();
    let run = probe_files(&mut files, "build 1", probe_with(&asked), |p, c| published.push((p.to_path_buf(), c.key.clone())));
    assert_eq!(run, 3);
    assert_eq!(*asked.borrow(), ["good", "bad", "other"], "one probe per class, in file order");
    assert_eq!(published.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>(), ["good", "bad", "other"], "each outcome published as it arrives");
    let a = &files[0];
    assert_eq!(a.error, None, "the file entry has no error");
    assert_eq!(
        a.plugins.iter().map(|c| (c.key.as_str(), c.category)).collect::<Vec<_>>(),
        [("good", "Instrument"), ("bad", "Instrument"), ("other", "Instrument")]
    );
    assert_eq!(a.plugins[0].probe.note_output, Some(true));
    assert_eq!(a.plugins[0].probe.audio_outputs, Some(0));
    assert_eq!(a.plugins[2].probe.note_output, Some(true), "the class after the failure has its own outcome");
    let bad = &a.plugins[1].probe;
    assert_eq!((bad.note_output, bad.note_input, bad.audio_outputs), (None, None, None));
    assert_eq!(bad.probe_error.as_deref(), Some("bad: the plug-in ended the scan process"));
    assert_eq!(bad.probe_host.as_deref(), Some("build 1"));
    let problems = problems(&a.plugins, |c| (PathBuf::from("a.plug"), c.key.clone(), c.key.clone()));
    assert_eq!(problems.len(), 1);
    assert_eq!((problems[0].id.as_str(), problems[0].host.as_str()), ("bad", "build 1"));
}

#[test]
fn a_failure_is_retried_by_another_build_and_a_success_is_reused() {
    let mut files = files();
    let asked = std::cell::RefCell::new(Vec::new());
    probe_files(&mut files, "build 1", probe_with(&asked), |_, _| {});
    asked.borrow_mut().clear();
    // The same build reuses both outcomes.
    assert_eq!(probe_files(&mut files, "build 1", probe_with(&asked), |_, _| {}), 0);
    assert!(asked.borrow().is_empty());
    // Another build probes the failed class again, never the ones that answered.
    assert_eq!(probe_files(&mut files, "build 2", probe_with(&asked), |_, _| {}), 1);
    assert_eq!(*asked.borrow(), ["bad"]);
    assert_eq!(files[0].plugins[1].probe.probe_host.as_deref(), Some("build 2"));
}

#[test]
fn a_stopped_probe_leaves_its_class_and_the_rest_unprobed() {
    let mut files = files();
    let mut asked = 0;
    let run = probe_files(
        &mut files,
        "build 1",
        |_, c| {
            asked += 1;
            (c.key != "bad").then_some(Ok(NOTES))
        },
        |_, _| {},
    );
    assert_eq!((run, asked), (1, 2));
    let p: Vec<&ProbeOutcome> = files[0].plugins.iter().map(|c| &c.probe).collect();
    assert_eq!(p[0].note_output, Some(true));
    assert_eq!(*p[1], ProbeOutcome::default(), "stopped: not a failure");
    assert_eq!(*p[2], ProbeOutcome::default());
    assert!(p[1].needs_probe("build 1") && p[2].needs_probe("build 1"), "the next scan probes them");
}

#[test]
fn a_probe_reply_reads_back() {
    let ok = reply::<String>(Ok(NOTES));
    let json = ok.strip_prefix(REPLY).unwrap_or_default();
    assert_eq!(parse_reply(json, |m| m), Ok(NOTES));
    let err = reply::<String>(Err("cannot create".into()));
    assert_eq!(parse_reply(err.strip_prefix(REPLY).unwrap_or_default(), |m| m), Err("cannot create".to_string()));
}

/// A deadline the test passes itself, at the first poll: no wall-clock wait.
struct Now;

impl Deadline for Now {
    fn passed(&mut self) -> bool {
        true
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(7)
    }
}

/// A deadline that never passes: the child's own exit or answer ends the wait.
struct Never;

impl Deadline for Never {
    fn passed(&mut self) -> bool {
        false
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(120)
    }
}

/// `(program, flag)` that runs `script` in a shell, with the script as the "file" argument and the
/// class id after it: `sh` takes the id as `$0`; PowerShell appends it to the script, where it is a
/// command not found after the script's own statements ran (so the id must not be a PowerShell
/// keyword such as `class`, which would fail the whole script before it runs).
fn shell() -> (&'static str, &'static str) {
    if cfg!(windows) { ("powershell", "-Command") } else { ("sh", "-c") }
}

#[test]
fn a_probe_child_answers_its_capability() {
    let (program, flag) = shell();
    let line = reply::<String>(Ok(NOTES));
    let script = if cfg!(windows) { format!("Write-Output '{line}';") } else { format!("echo '{line}'") };
    let r = probe_in_child(Path::new(program), flag, Path::new(&script), "the-class", || false, &mut Never);
    assert_eq!(r, Some(Ok(NOTES)));
}

#[test]
fn a_probe_child_that_ends_without_an_answer_is_a_failure() {
    let (program, flag) = shell();
    let script = if cfg!(windows) { "exit 3;" } else { "exit 3" };
    let r = probe_in_child(Path::new(program), flag, Path::new(script), "the-class", || false, &mut Never);
    let e = r.and_then(Result::err).unwrap_or_default();
    assert!(e.contains("ended the scan process") && e.contains("(the-class)"), "{e}");
}

#[test]
fn a_probe_child_past_its_deadline_is_stopped_and_a_failure() {
    let (program, flag) = shell();
    let script = if cfg!(windows) { "Start-Sleep 60;" } else { "exec sleep 60" };
    let r = probe_in_child(Path::new(program), flag, Path::new(script), "the-class", || false, &mut Now);
    let e = r.and_then(Result::err).unwrap_or_default();
    assert!(e.contains("still loading after 7 s; stopped") && e.contains("(the-class)"), "{e}");
}

#[test]
fn a_probe_child_stopped_by_the_scan_is_not_an_outcome() {
    let (program, flag) = shell();
    let script = if cfg!(windows) { "Start-Sleep 60;" } else { "exec sleep 60" };
    assert_eq!(probe_in_child(Path::new(program), flag, Path::new(script), "the-class", || true, &mut Never), None);
}

/// A deadline that, when first asked, lets the child go on (it creates `release`) and then waits
/// until the child has written its whole answer and said so (`mark`): the answer is in the pipe,
/// unread, when the deadline passes.
struct AnswerInThePipe {
    release: PathBuf,
    mark: PathBuf,
}

impl Deadline for AnswerInThePipe {
    fn passed(&mut self) -> bool {
        let _ = std::fs::write(&self.release, b"go on");
        while !self.mark.exists() {
            std::thread::yield_now();
        }
        true
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(7)
    }
}

/// Review round 2, finding E: a complete answer already in the pipe when the deadline passes still
/// counts (what is readable is read once more before the run rules "deadline").
#[test]
fn an_answer_in_the_pipe_at_the_deadline_counts() {
    let d = transit_test_support::TestDir::new("probe-answer-at-deadline");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (release, mark) = (d.join("release"), d.join("mark"));
    let (program, flag) = shell();
    let line = reply::<String>(Ok(NOTES));
    let script = if cfg!(windows) {
        format!(
            "while (!(Test-Path '{}')) {{ Start-Sleep -Milliseconds 10 }}; Write-Output '{line}'; New-Item '{}' | Out-Null; Start-Sleep 60;",
            release.display(),
            mark.display()
        )
    } else {
        format!("while [ ! -f '{}' ]; do sleep 0.01; done; echo '{line}'; touch '{}'; exec sleep 60", release.display(), mark.display())
    };
    let r = probe_in_child(Path::new(program), flag, Path::new(&script), "the-class", || false, &mut AnswerInThePipe { release, mark });
    assert_eq!(r, Some(Ok(NOTES)));
    let _ = std::fs::remove_dir_all(&d);
}

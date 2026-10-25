//! `transit --report` (#114) end to end: the real binary, no display, no audio stream.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-report-cli-{}-{}-{tag}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn transit(config: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_transit"))
        .args(args)
        .current_dir(cwd)
        .env("TRANSIT_CONFIG_DIR", config)
        .env("TRANSIT_NO_UPDATE_CHECK", "1")
        .env("USERNAME", "report_tester")
        .env("USER", "report_tester")
        .output()
        .expect("run transit")
}

const HEADERS: [&str; 8] = [
    "== transit ==",
    "== System ==",
    "== Audio stack ==",
    "== Audio devices ==",
    "== Settings ==",
    "== Open devices ==",
    "== Callback telemetry ==",
    "== Log ==",
];

#[test]
fn report_writes_a_file_with_every_section_and_prints_its_path() {
    let (config, cwd) = (temp_dir("config"), temp_dir("cwd"));
    let logs = config.join("logs");
    std::fs::create_dir_all(&logs).expect("logs");
    std::fs::write(logs.join("transit.log"), "first line\nINFO user report_tester opened a file\nlast line\n").expect("log");
    let out = transit(&config, &cwd, &["--report"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let path = PathBuf::from(&printed);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    assert!(name.starts_with("transit-report-") && name.ends_with(".txt"), "{name}");
    let text = std::fs::read_to_string(&path).expect("the printed path exists");
    for header in HEADERS {
        assert!(text.contains(header), "missing {header}\n{text}");
    }
    assert!(text.contains("Version: "), "{text}");
    assert!(text.contains("last line"), "the log's lines are in: {text}");
    assert!(text.contains("INFO user <user> opened a file"), "and the user name is not: {text}");
    assert!(!text.contains("report_tester"), "{text}");
    assert_eq!(
        std::fs::read_to_string(logs.join("transit.log")).expect("log"),
        "first line\nINFO user report_tester opened a file\nlast line\n",
        "the log is read, not rotated"
    );
    let _ = std::fs::remove_dir_all(&config);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn report_with_a_path_writes_there() {
    let (config, cwd) = (temp_dir("config2"), temp_dir("cwd2"));
    let target = cwd.join("mine.txt");
    let out = transit(&config, &cwd, &["--report", target.to_str().expect("utf-8 path")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("mine.txt"));
    let text = std::fs::read_to_string(&target).expect("file at the given path");
    assert!(HEADERS.iter().all(|h| text.contains(h)));
    assert!(text.contains("(no log lines)"), "no log yet is still a report");
    let _ = std::fs::remove_dir_all(&config);
    let _ = std::fs::remove_dir_all(&cwd);
}

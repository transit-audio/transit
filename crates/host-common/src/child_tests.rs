use super::*;

fn reply_line(json: &str) -> Vec<u8> {
    format!("{REPLY}{json}\n").into_bytes()
}

#[test]
fn the_first_complete_answer_line_is_kept_and_other_lines_are_dropped() {
    let mut c = Collector::new(64, 1024);
    c.push(b"plug-in chatter\n").unwrap();
    assert!(c.line.is_empty(), "a finished line that is not an answer is not kept");
    c.push(&reply_line("{\"a\":1}")[..10]).unwrap();
    assert_eq!(c.reply(), None, "half a line is no answer");
    c.push(&reply_line("{\"a\":1}")[10..]).unwrap();
    assert_eq!(c.reply(), Some("{\"a\":1}"));
    c.push(&reply_line("{\"b\":2}")).unwrap();
    assert_eq!(c.reply(), Some("{\"a\":1}"), "the first answer stands");
    let mut d = Collector::new(64, 1024);
    d.push(format!("{REPLY}{{}}\r\n").as_bytes()).unwrap();
    assert_eq!(d.reply(), Some("{}"), "a carriage return is not part of the answer");
    let mut e = Collector::new(64, 1024);
    e.push(format!("{REPLY}{{\"last\":true}}").as_bytes()).unwrap();
    assert_eq!(e.reply(), None);
    e.finish();
    assert_eq!(e.reply(), Some("{\"last\":true}"), "an unterminated last line counts at the end of the output");
}

#[test]
fn a_line_past_its_bound_is_an_error_not_more_memory() {
    let mut c = Collector::new(16, 1024);
    c.push(&[b'x'; 10]).unwrap();
    let e = c.push(&[b'x'; 10]).unwrap_err();
    assert!(e.contains("longer than 16 bytes"), "{e}");
    assert!(c.line.len() <= 16, "{}", c.line.len());
    // Lines that end within the bound are fine, however many.
    let mut d = Collector::new(16, 1 << 20);
    for _ in 0..1000 {
        d.push(b"0123456789\n").unwrap();
    }
    assert!(d.line.is_empty());
}

#[test]
fn output_past_its_bound_is_an_error() {
    let mut c = Collector::new(1024, 100);
    for _ in 0..9 {
        c.push(b"012345678\n").unwrap();
    }
    let e = c.push(b"0123456789\n").unwrap_err();
    assert!(e.contains("passed 100 bytes"), "{e}");
}

/// Review round 3, item 3: once an answer is held, later output is not judged: neither an
/// over-long line nor too much output turns the run into an error.
#[test]
fn an_answer_held_is_not_undone_by_later_output() {
    let mut c = Collector::new(64, 200);
    let mut chunk = reply_line("{\"a\":1}");
    chunk.extend_from_slice(&[b'x'; 100]);
    c.push(&chunk).unwrap();
    c.push(&[b'y'; 1000]).unwrap();
    assert_eq!(c.reply(), Some("{\"a\":1}"));
}

/// Review round 3, item 6: the parent watch failure seam acts only with both its environment
/// variable and the armed handshake; the variable alone (inherited by a test-hooks build of the
/// app) does nothing.
#[test]
fn the_watch_seam_needs_its_variable_and_the_armed_handshake() {
    let armed = std::str::from_utf8(ARMED_GO).unwrap();
    let go = std::str::from_utf8(GO).unwrap();
    let env = Some(OsStr::new("1"));
    assert!(watch_armed(env, armed));
    assert!(!watch_armed(env, go), "the variable alone");
    assert!(!watch_armed(None, armed), "the handshake alone");
    assert!(accepted(go));
    assert!(!accepted("go on\n") && !accepted(""), "only the handshake lets a child go on");
}

/// A deadline that panics once the child has written its process id to `pid`.
#[cfg(unix)]
struct PanicsOnceStarted {
    pid: std::path::PathBuf,
}

#[cfg(unix)]
impl Deadline for PanicsOnceStarted {
    fn passed(&mut self) -> bool {
        if std::fs::read_to_string(&self.pid).is_ok_and(|p| p.trim().len() > 1) {
            panic!("a deadline callback panics");
        }
        false
    }
    fn limit(&self) -> Duration {
        Duration::from_secs(7)
    }
}

/// Review round 3, item 4: a panic in a callback while the child runs leaves no child behind, not
/// even a zombie (the run's guard kills the tree, then kills and reaps the child, as it unwinds).
#[cfg(unix)]
#[test]
fn a_panic_in_a_callback_leaves_no_child_behind() {
    let d = transit_test_support::TestDir::new("child-panic-guard");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let pid_file = d.join("pid");
    let script = format!("echo $$ > '{}'; exec sleep 60", pid_file.display());
    let args = [OsStr::new("-c"), OsStr::new(&script)];
    let mut deadline = PanicsOnceStarted { pid: pid_file.clone() };
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(Path::new("sh"), &args, Path::new("x"), &mut deadline, || false)));
    assert!(unwound.is_err(), "the callback's panic came through");
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    let state = std::process::Command::new("ps").args(["-o", "stat=", "-p", &pid]).output().unwrap();
    let state = String::from_utf8_lossy(&state.stdout).trim().to_string();
    assert_eq!(state, "", "the child {pid} is gone, reaped (a zombie shows Z)");
    let _ = std::fs::remove_dir_all(&d);
}

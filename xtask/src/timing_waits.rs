//! Rule (f) of `cargo xtask timing` (E49 recurrence 2026-11-26): a wall-clock wait in the test code of
//! the crates whose tests drive real threads (`crates/{playback,plugin-host,midi-io,ui-studio}`) is a
//! listed exception. Twice in one day a test failed on a starved machine because a background thread
//! (a plug-in host thread, the native MIDI worker) had not reached a state when a wall-clock bound or a
//! fixed frame count ran out. A test drives that work itself where it can (an inline plug-in host, a
//! stepped clock, a handshake it controls); where a real thread is what the test proves, its wait says
//! so.
//!
//! A wait helper is a fn of test code that is not itself a test (no `#[test]`, not `#[ignore]`) whose
//! own lines hold a loop (`while`, `loop`, `for`) and read the wall clock (`Instant::now()` or
//! `.elapsed()`): the `until`/`frames_until` family many tests share. It needs
//! `// timing: stall-bound <reason>` on its header, in the attribute and comment lines directly above
//! it, or on a line of its body; the reason names what is waited for and why it is a real thread. A
//! test's own inline loop stays with rules (b) and (d).
//!
//! The helpers that existed when the rule came in are listed in [`EXCEPTIONS`] instead, each with
//! what it waits for. A wait for a named state cannot tell a starved thread from a lost wake or a
//! lost message: both run out the bound with the same text (E49 recurrence 2026-11-26, where a
//! starved-looking MIDI handoff wait was a product race). Shrink the list by driving the work from the
//! test; never grow it without a reason a reviewer can check.

use super::{Hit, parse};

/// The wait helpers kept on real threads: (path, fn, what it waits for).
pub(crate) const EXCEPTIONS: &[(&str, &str, &str)] = &[
    (
        "crates/playback/src/player/device/external_measure_tests.rs",
        "until",
        "a fake-device Player's MIDI output thread and ingress worker reach a named state; 10 s guard",
    ),
    (
        "crates/playback/src/player/device/external_tests.rs",
        "until",
        "a fake-device Player's MIDI output thread and ingress worker reach a named state; 10 s guard",
    ),
    ("crates/playback/src/player/device/tests/external_converted.rs", "measure", "10 s guard for the MIDI output thread to open the destination"),
    ("crates/playback/src/player/device/tests.rs", "hardware_ping_on_native_asio", "hardware helper, called only from #[ignore] loopback tests"),
    ("crates/playback/src/player/midi_out/tests.rs", "until", "the MIDI output thread (port polling, opens, sends) is the subject; 10 s guard"),
    ("crates/playback/src/player/native/tests.rs", "until", "the native MIDI device thread and ingress worker reach a named state; 10 s guard"),
    ("crates/playback/src/player/runtime/late_tests.rs", "frames_until", "instances made on a non-inline plug-in host thread"),
    (
        "crates/playback/src/player/runtime/tests.rs",
        "frames_until_loaded",
        "the subject: frames never block while a host thread makes an instance; 20 s guard",
    ),
    (
        "crates/playback/src/player/runtime/tests.rs",
        "render_while_an_instance_is_made",
        "the subject: a stand-in audio thread renders while a host thread sits in a slow factory",
    ),
    ("crates/playback/src/player/runtime/tests.rs", "slow_readers", "instances made on a non-inline plug-in host thread; 20 s guard"),
    (
        "crates/playback/src/system_audio_tests.rs",
        "simulate_with",
        "no wait: the Instant is the origin of simulated stamps and the loop steps simulated time",
    ),
    ("crates/plugin-host/tests/host_thread.rs", "frames", "the host thread is the subject: frames advance while it works; returns at its limit"),
    ("crates/plugin-host/tests/host_thread.rs", "finish", "a state job on the host thread under test; 5 s guard"),
    ("crates/plugin-host/tests/presets.rs", "finish", "a state job on the host thread; 10 s guard"),
    (
        "crates/ui-studio/src/audio_devices_tests.rs",
        "frames_until",
        "the ASIO listing thread finishes (it must run off the UI thread); 5 s, the caller asserts",
    ),
    ("crates/ui-studio/src/browser_tests.rs", "settle", "the browser's background readers answer; gives up after 5 s without asserting"),
    ("crates/ui-studio/src/browser_tests.rs", "settle_imports", "the import threads commit or drop; 10 s, asserts"),
    ("crates/ui-studio/src/emergency_tests.rs", "child", "a child process loads the VST3 fixture on a real host thread before its emergency exit"),
    ("crates/ui-studio/src/emergency_tests.rs", "run_child", "a child process's exit is the subject"),
    ("crates/ui-studio/src/host/reconnect_tests.rs", "settle", "the VST3 fixture's instances are made on the plug-in host thread; 20 s guard"),
    ("crates/ui-studio/src/host_hang_tests.rs", "frames_until", "the host thread's wall-clock watchdog is the subject (a call held past its budget)"),
    ("crates/ui-studio/src/plugin_header_tests.rs", "frames_until", "the VST3 fixture on the plug-in host thread; 20 s guard"),
    ("crates/ui-studio/src/plugin_header_tests.rs", "finish", "a state job on the plug-in host thread; 20 s guard"),
    ("crates/ui-studio/src/plugin_header_tests.rs", "finish_without_frames", "a state job on the plug-in host thread with no frames; 20 s guard"),
    ("crates/ui-studio/src/plugin_header_tests.rs", "live_becomes", "session parameters reach the plug-in on the host thread; 10 s guard"),
    (
        "crates/ui-studio/src/plugin_retry_tests.rs",
        "frames_until",
        "creations held by a gate on a real host thread; the display runs on a manual clock",
    ),
    ("crates/ui-studio/src/unresponsive_tests.rs", "frames_until", "the stuck host-thread test: the host thread's watchdog is the subject"),
];

/// The crates whose tests rule (f) scans.
const CRATES: &[&str] = &["playback", "plugin-host", "midi-io", "ui-studio"];

fn in_scope(path: &str) -> bool {
    path.strip_prefix("crates/").and_then(|rest| rest.split('/').next()).is_some_and(|name| CRATES.contains(&name))
}

fn is_loop(code: &str) -> bool {
    let t = code.trim_start();
    t.starts_with("while ") || t.starts_with("for ") || t.starts_with("loop {") || code.contains(" loop {") || code.contains("= loop {")
}

fn reads_wall_clock(code: &str) -> bool {
    code.contains("Instant::now()") || code.contains(".elapsed()")
}

fn stall_bound(comment: &str) -> bool {
    comment.find("// timing:").is_some_and(|at| {
        let mut words = comment[at + "// timing:".len()..].split_whitespace();
        words.next() == Some("stall-bound") && words.next().is_some()
    })
}

pub fn scan(path: &str, src: &str, test_file: bool) -> Vec<Hit> {
    if !in_scope(path) {
        return Vec::new();
    }
    let p = parse(src, test_file);
    let lines: Vec<&str> = src.lines().collect();
    let mut hits = Vec::new();
    for f in p.funcs.iter().filter(|f| p.in_test[f.start] && !f.ignored) {
        let own = &p.code[f.start..=f.end];
        if !own.iter().any(|l| is_loop(l)) || !own.iter().any(|l| reads_wall_clock(l)) {
            continue;
        }
        let mut marked = p.comment[f.start..=f.end].iter().any(|c| stall_bound(c));
        let mut test = false;
        let mut i = f.start;
        while i > 0 {
            i -= 1;
            let code = p.code[i].trim();
            if !(code.is_empty() || code.starts_with("#[")) || (code.is_empty() && p.comment[i].is_empty()) {
                break;
            }
            test |= code.starts_with("#[test]");
            marked |= stall_bound(&p.comment[i]);
        }
        let name = super::pool::fn_name(&p.code[f.start]);
        let listed = EXCEPTIONS.iter().any(|(at, n, _)| *at == path && Some(*n) == name.as_deref());
        if !test && !marked && !listed {
            let text = format!("wall-clock wait without a stall-bound reason: {}", lines.get(f.start).copied().unwrap_or("").trim());
            hits.push(Hit { path: path.into(), line: f.start + 1, rule: 'f', text });
        }
    }
    hits
}

#[cfg(test)]
#[path = "timing_waits_tests.rs"]
mod tests;

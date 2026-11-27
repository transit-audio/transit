#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The test binary re-runs itself in a role (`role_entry`, chosen by `ROLE`): `holder` starts a
//! `leaf` that inherits its standard output, says the leaf's id and sleeps; `middle` starts a
//! contained `watched` leaf (which calls [`exit_with_parent`]) that inherits its standard output,
//! says so and sleeps. Every wait in these tests is for an end of file on a pipe only the processes
//! under test hold open, so a test returns exactly when they are gone; nothing reads a clock.

use super::*;
use std::io::{BufRead as _, Read as _};
use std::process::Stdio;

const ROLE: &str = "TRANSIT_RT_CHILD_TREE_ROLE";
const ENTRY: &str = "child_tree::tests::role_entry";

/// A role's command: this test binary, running only `role_entry`.
fn role(name: &str) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", ENTRY, "--nocapture", "--test-threads=1"]).env(ROLE, name);
    c
}

/// Sleeps long past any test (a process left uncontained holds its pipe open), then says on its
/// standard output that it ended by itself: a test that finds the line fails instead of passing
/// late.
fn sleep_long() -> ! {
    std::thread::sleep(std::time::Duration::from_secs(300));
    println!("{ENDED}");
    std::process::exit(0)
}

const ENDED: &str = "ENDED-BY-ITSELF";

#[test]
fn role_entry() {
    let Ok(name) = std::env::var(ROLE) else { return };
    match name.as_str() {
        "leaf" => sleep_long(),
        "holder" => {
            // Held back until the parent has contained this process (the scan children's rule).
            let mut go = String::new();
            std::io::stdin().read_line(&mut go).unwrap();
            // Never waited for: the leaf lives until the tree is killed, which is the test.
            #[allow(clippy::zombie_processes)]
            let leaf = role("leaf").stdin(Stdio::null()).spawn().unwrap();
            println!("LEAF {}", leaf.id());
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            sleep_long()
        }
        "middle" => {
            let mut c = role("watched");
            c.stdin(Stdio::null()).env("WATCH_PARENT", std::process::id().to_string());
            prepare(&mut c);
            // Never waited for: `watched` lives until this process is killed, which is the test.
            #[allow(clippy::zombie_processes)]
            let watched = c.spawn().unwrap();
            let tree = Tree::contain(&watched).unwrap();
            println!("WATCHED {}", watched.id());
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            // Keep the tree (the job handle) for as long as this process lives.
            std::mem::forget(tree);
            sleep_long()
        }
        "quick" => {
            let mut go = String::new();
            std::io::stdin().read_line(&mut go).unwrap();
            println!("QUICK");
            std::process::exit(0)
        }
        "talker" => {
            let mut go = String::new();
            std::io::stdin().read_line(&mut go).unwrap();
            println!("HELLO");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            sleep_long()
        }
        "watched" => {
            exit_with_parent(std::env::var("WATCH_PARENT").unwrap().parse().unwrap()).unwrap();
            sleep_long()
        }
        other => panic!("unknown role {other}"),
    }
}

/// Reads lines until one starts with `tag`; returns the rest of it.
fn line_with(out: &mut std::io::BufReader<std::process::ChildStdout>, tag: &str) -> String {
    let mut line = String::new();
    loop {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the role ended before saying {tag}");
        // The harness writes its own "test ... " before the role's output on the same line.
        if let Some(at) = line.find(tag) {
            return line[at + tag.len()..].trim().to_string();
        }
    }
}

#[test]
fn killing_a_tree_ends_the_child_and_what_it_started() {
    let mut c = role("holder");
    c.stdin(Stdio::piped()).stdout(Stdio::piped());
    prepare(&mut c);
    let mut holder = c.spawn().unwrap();
    let tree = Tree::contain(&holder).unwrap();
    std::io::Write::write_all(&mut holder.stdin.take().unwrap(), b"go\n").unwrap();
    let mut out = std::io::BufReader::new(holder.stdout.take().unwrap());
    let leaf = line_with(&mut out, "LEAF");
    assert_ne!(leaf, "");
    tree.kill();
    holder.wait().unwrap();
    // The leaf holds the pipe too: the end of file comes only once it is gone as well.
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    assert!(!rest.contains(ENDED), "a process of the tree was not ended: {rest}");
}

#[test]
fn a_contained_child_ends_when_its_parent_is_killed() {
    // The parent (`middle`) is killed outright, as a crash would end it: it runs no cleanup.
    let mut middle = role("middle").stdin(Stdio::null()).stdout(Stdio::piped()).spawn().unwrap();
    let mut out = std::io::BufReader::new(middle.stdout.take().unwrap());
    let watched = line_with(&mut out, "WATCHED");
    assert_ne!(watched, "");
    middle.kill().unwrap();
    middle.wait().unwrap();
    // `watched` inherited the pipe: the end of file comes only once it is gone too (Windows: the
    // job closed with its owner; Linux and macOS: its parent watch).
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    assert!(!rest.contains(ENDED), "a process of the tree was not ended: {rest}");
}

/// Starts `name` contained, with its standard input and output piped, and says "go".
fn contained(name: &str) -> (std::process::Child, Tree) {
    let mut c = role(name);
    c.stdin(Stdio::piped()).stdout(Stdio::piped());
    prepare(&mut c);
    let child = c.spawn().unwrap();
    let tree = Tree::contain(&child).unwrap();
    let mut child = child;
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), b"go\n").unwrap();
    (child, tree)
}

/// Review finding A: the end of a child is observed without reaping it, so its tree is killed
/// while its id still names its group; a tree killed after its child was reaped signals nothing.
/// (One test: the refusal counter is process-wide.)
#[test]
fn a_tree_is_killed_only_while_its_child_is_unreaped() {
    let (mut child, tree) = contained("quick");
    let mut rest = String::new();
    child.stdout.take().unwrap().read_to_string(&mut rest).unwrap();
    assert!(rest.contains("QUICK"), "{rest}");
    // The child closed its output when it ended; wait (without reaping) until the system says so.
    while !exited(&mut child).unwrap() {
        std::thread::yield_now();
    }
    assert!(exited(&mut child).unwrap(), "asking again finds it again: it was not reaped");
    let before = refused_kills();
    tree.kill();
    assert_eq!(refused_kills(), before, "the tree was signalled: its child was still unreaped");
    child.wait().unwrap();

    let (mut reaped, late) = contained("quick");
    reaped.wait().unwrap();
    let before = refused_kills();
    late.kill();
    let refused = if cfg!(unix) { 1 } else { 0 };
    assert_eq!(refused_kills(), before + refused, "a tree whose child was reaped is not signalled (Linux and macOS)");
}

/// Review finding C: output is read without blocking, and reading ends when the writers are gone.
#[test]
fn output_is_read_without_waiting_until_its_writers_are_gone() {
    let (mut child, tree) = contained("talker");
    let mut output = Output::new(child.stdout.take().unwrap()).unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(&got).contains("HELLO\n") {
        match output.read_available(&mut buf).unwrap() {
            Read::Bytes(n) => got.extend_from_slice(&buf[..n]),
            Read::Nothing => std::thread::yield_now(),
            Read::End => panic!("ended before HELLO"),
        }
    }
    assert_eq!(output.read_available(&mut buf).unwrap(), Read::Nothing, "the talker sleeps: nothing to read, and no wait");
    tree.kill();
    loop {
        match output.read_available(&mut buf).unwrap() {
            Read::End => break,
            Read::Nothing => std::thread::yield_now(),
            Read::Bytes(_) => {}
        }
    }
    child.wait().unwrap();
}

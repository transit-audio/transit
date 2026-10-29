//! Issue #205: every call the host thread makes into plug-in code (a creation, a request for an
//! instance) runs inside a host-call scope, so the formats' servicing and editor `idle` reached
//! from inside it wait for the next tick; the tick itself runs outside any call.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_plugin_host::{Config, Hooks, Host};

#[test]
fn jobs_run_inside_a_host_call_and_ticks_outside_one() {
    let (ticked, ticks) = mpsc::channel::<bool>();
    let ticked = Mutex::new(ticked);
    let hooks = Hooks {
        tick: Some(Arc::new(move || {
            let _ = ticked.lock().unwrap().send(transit_dsp::host_call::active());
        })),
        ..Hooks::default()
    };
    let host = Host::new(Config { hooks, inline: false, ..Config::default() });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let (in_creation, mut plugin) = host
        .submit(move |ctx| (transit_dsp::host_call::active(), ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))))
        .wait(Duration::from_secs(5))
        .unwrap();
    assert!(in_creation, "a creation is a host call into plug-in code");
    assert!(!ticks.recv_timeout(Duration::from_secs(5)).expect("a tick ran"), "the tick runs outside any host call");
    // A request for the instance (a preset load) runs inside one too.
    let mut reader = plugin.state_reader().unwrap();
    let mut job = reader.begin_save();
    let saved = loop {
        match job.poll() {
            transit_dsp::JobPoll::Waiting => std::thread::yield_now(),
            other => break other,
        }
    };
    assert!(matches!(saved, transit_dsp::JobPoll::Ready(_)));
    assert!(!transit_dsp::host_call::active(), "the caller's thread has no call on its stack");
    drop((reader, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn an_inline_job_runs_inside_a_host_call() {
    let host = Host::new(Config { inline: true, ..Config::default() });
    let inside = host.submit(|_| transit_dsp::host_call::active()).wait(Duration::from_secs(5));
    assert_eq!(inside, Some(true));
    assert!(!transit_dsp::host_call::active(), "and the scope ends with the job");
}

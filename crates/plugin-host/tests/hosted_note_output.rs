//! ADR 0064 slice HM1: the plug-in host's wrapper forwards the note output seam (the readable
//! output, the block's events and call report, the lifecycle and the stage reset) to the instance
//! it wraps, on the caller's thread.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use transit_dsp::hosted_out::{CallRecord, CallReport, HostedOut, Lifecycle, NoteOutputInfo, OutForm, OutKind, OutValues};
use transit_dsp::{Plugin, PluginInfo};
use transit_plugin_host::{Config, Host};

/// A plug-in with a note output: one event a block, one call under generation 9 with the reset
/// before it, and a count of its stage resets.
struct Notes {
    resets: Arc<AtomicU32>,
}

const EVENT: HostedOut = HostedOut {
    offset: 5,
    seq: 0,
    kind: OutKind::NoteOn,
    port: 1,
    channel: 2,
    key: 60,
    note_id: -1,
    form: OutForm::Native,
    values: OutValues::Velocity(0.5),
};

impl Plugin for Notes {
    fn info(&self) -> &'static PluginInfo {
        &common::INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn note_output(&self) -> Option<NoteOutputInfo> {
        Some(NoteOutputInfo { read: 1, declared: 2 })
    }
    fn take_note_output(&mut self, out: &mut dyn FnMut(HostedOut) -> bool) -> CallReport {
        out(EVENT);
        let mut r = CallReport::default();
        r.record(CallRecord { generation: 9, reset_before: true, error: false });
        r
    }
    fn lifecycle(&self) -> Lifecycle {
        Lifecycle { generation: 9, active: true }
    }
    fn restart_processing(&mut self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
    }
    fn collect_note_output(&mut self, on: bool) {
        self.resets.fetch_add(if on { 10 } else { 100 }, Ordering::SeqCst);
    }
}

#[test]
fn the_wrapper_forwards_the_note_output_seam() {
    let host = Host::new(Config { inline: false, ..Config::default() });
    let resets = Arc::new(AtomicU32::new(0));
    let r = Arc::clone(&resets);
    let mut p = host.submit(move |ctx| ctx.adopt(Box::new(Notes { resets: r }))).wait(Duration::from_secs(10)).expect("adopted");
    assert_eq!(p.note_output(), Some(NoteOutputInfo { read: 1, declared: 2 }));
    assert_eq!(p.lifecycle(), Lifecycle { generation: 9, active: true });
    let mut got = Vec::new();
    let report = p.take_note_output(&mut |e| {
        got.push(e);
        true
    });
    assert_eq!(got, vec![EVENT]);
    assert_eq!(
        report.calls().collect::<Vec<_>>(),
        [CallRecord { generation: 9, reset_before: true, error: false }],
        "the call report, through the wrapper"
    );
    p.restart_processing();
    assert_eq!(resets.load(Ordering::SeqCst), 1, "the stage reset reached the instance");
    p.collect_note_output(true);
    assert_eq!(resets.load(Ordering::SeqCst), 11, "the collection switch reached the instance");
    drop(p);
    assert!(host.shutdown(Duration::from_secs(10)));
}

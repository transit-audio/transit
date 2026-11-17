//! The render's transport requests (ADR 0062 section 11): the start returns the Play's sequence;
//! the job keeps every Stop from the moment it submits it and reports it once, on the poll after
//! it (with or without a result) or in what a cancel returns; a Stop the Player refused without a
//! fault is owed; a refused Play is disposed of without a Stop.
use super::*;
use crate::realtime_render::{RenderReport, RenderStops, TapState};

/// A sink whose `finish` waits until the test lets it go: the capture completes while the writer
/// is still running.
struct Gated {
    gate: std::sync::mpsc::Receiver<()>,
}

impl RenderSink for Gated {
    fn main(&mut self, _: &[Vec<f32>], _: usize) -> Result<(), String> {
        Ok(())
    }
    fn stem(&mut self, _: usize, _: &[Vec<f32>], _: usize) -> Result<(), String> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        let _ = self.gate.recv();
        Ok(())
    }
    fn abort(&mut self) {}
}

fn gated(rig: &Rig) -> (std::sync::mpsc::Sender<()>, action_lane::Sequence) {
    let (tx, gate) = std::sync::mpsc::channel();
    let play = rig.player.start_realtime_render(&spec(rig, LEN), Box::new(Gated { gate })).unwrap();
    (tx, play)
}

/// Pump until every frame the files need is captured, without polling the render (so no Stop
/// is submitted yet).
fn until_captured(rig: &Rig) {
    let tap = Arc::clone(rig.player.render_tap.as_ref().unwrap());
    pump(rig, 2_000, |_, _| tap.state() == TapState::Captured);
    assert_eq!(tap.state(), TapState::Captured);
}

/// The applied Stop frontier.
fn stop_frontier(rig: &Rig) -> action_lane::Sequence {
    rig.player.recovery_progress().unwrap().0
}

/// The receipt of `sequence` among the Player's notices.
fn receipt(rig: &Rig, sequence: action_lane::Sequence) -> Option<action_lane::Outcome> {
    let mut notices = [None; 64];
    let count = rig.player.poll_notices(&mut notices).unwrap();
    notices[..count].iter().flatten().find_map(|n| match n {
        Notice::Action(r) if r.sequence == sequence => Some(r.outcome),
        _ => None,
    })
}

#[test]
fn the_render_returns_every_stop_it_submitted() {
    let rig = rig();
    let (tx, play) = gated(&rig);
    pump(&rig, 4, |_, _| false);
    assert_eq!(receipt(&rig, play), Some(action_lane::Outcome::Applied), "the returned sequence is the render's Play");
    until_captured(&rig);
    // The capture-complete Stop is reported on the poll that submits it, though the writer has
    // not finished (it waits in `finish`): it is kept from the moment it is submitted.
    let first = rig.player.realtime_render_poll();
    assert!(first.result.is_none(), "the writer is still running");
    assert_eq!(first.stops.submitted.len(), 1, "{first:?}");
    assert!(!first.stops.owed);
    let stop = first.stops.submitted[0];
    // Reported once: later polls report nothing new, with or without a result.
    let again = rig.player.realtime_render_poll();
    assert_eq!(again.stops, RenderStops::default());
    drop(tx);
    let mut done = RenderReport::default();
    until("the writer finishes", || {
        pump(&rig, 1, |_, _| false);
        done = rig.player.realtime_render_poll();
        assert_eq!(done.stops, RenderStops::default(), "nothing new to report");
        done.result.is_some()
    });
    assert!(done.result.unwrap().is_ok());
    pump(&rig, 4, |_, _| false);
    assert!(stop_frontier(&rig) >= stop, "the reported Stop is the one the device applied");
    assert!(!rig.player.is_playing());

    // A cancel returns its own Stop, and no result while the writer was still running.
    let (tx, _) = gated(&rig);
    pump(&rig, 40, |_, p| p.is_playing() && p.position() > START);
    drop(tx);
    let cancelled = rig.player.cancel_realtime_render();
    assert_eq!(cancelled.stops.submitted.len(), 1, "{cancelled:?}");
    assert!(cancelled.result.is_none(), "{cancelled:?}");
    assert!(rig.player.cancel_realtime_render().stops.submitted.is_empty(), "a second cancel finds no job");
    pump(&rig, 4, |_, _| false);
    assert!(stop_frontier(&rig) >= cancelled.stops.submitted[0]);

    // A cancel after the writer finished (no poll collected it) returns the writer's own result.
    let (tx, _) = gated(&rig);
    until_captured(&rig);
    drop(tx);
    until("the writer finishes", || rig.player.realtime_writer_finished());
    let late = rig.player.cancel_realtime_render();
    assert_eq!(late.stops.submitted.len(), 1, "the cancel's own Stop: no poll submitted one");
    assert_eq!(late.result.map(|r| r.map(|d| d.frames)), Some(Ok(LEN)), "the finished writer's result stands");
}

#[test]
fn a_render_stop_that_could_not_be_sent_is_owed() {
    let rig = rig();
    let (tx, _) = gated(&rig);
    until_captured(&rig);
    {
        // Main's owner is borrowed: the capture-complete Stop is refused as busy, not by a fault.
        let _busy = rig.player.owner.borrow_mut();
        let report = rig.player.realtime_render_poll();
        assert!(report.stops.submitted.is_empty());
        assert!(report.stops.owed, "the Stop that could not be submitted is owed");
        let again = rig.player.realtime_render_poll();
        assert_eq!(again.stops, RenderStops::default(), "reported once");
    }
    // The job does not retry it (the caller owns the retries): the transport plays on.
    pump(&rig, 4, |_, _| false);
    assert!(rig.player.is_playing());
    assert_eq!(rig.player.realtime_render_poll().stops, RenderStops::default());
    drop(tx);
    let cancelled = rig.player.cancel_realtime_render();
    assert_eq!(cancelled.stops.submitted.len(), 1, "the cancel's Stop is sent");
    assert!(!cancelled.stops.owed);
    pump(&rig, 4, |_, _| false);
    assert!(!rig.player.is_playing());
}

#[test]
fn a_refused_render_play_disposes_without_stop() {
    let rig = rig();
    let (sink, mix, stem) = file_sink(&rig, LEN);
    rig.player.start_realtime_render(&spec(&rig, LEN), Box::new(sink)).unwrap();
    let before = stop_frontier(&rig);
    // Into the range with the synth's note held, so there is something to release.
    pump(&rig, 400, |_, p| p.is_playing() && p.position() > (AT + C) as i64);
    until("the note-on is on the port", || sent(&rig).contains(&vec![0x92, 60, 100]));
    assert!(!released(&sent(&rig)));
    // The caller saw the render's Play refused: it disposes of the job.
    let report = rig.player.dispose_realtime_render();
    assert_eq!(report.stops, RenderStops::default(), "no Stop is submitted");
    assert!(report.result.is_none());
    assert!(!rig.player.realtime_render_running());
    assert!(!mix.exists() && !stem.exists(), "no file");
    assert_eq!(std::fs::read_dir(&rig.dir).unwrap().count(), 0, "no part file either");
    until("the note-off and All Notes Off after the note-on", || released(&sent(&rig)));
    // Nothing stopped the transport (here the Play was in fact admitted, and it plays on).
    pump(&rig, 8, |_, _| false);
    assert_eq!(stop_frontier(&rig), before, "no Stop reached the device");
    assert!(rig.player.is_playing());
}

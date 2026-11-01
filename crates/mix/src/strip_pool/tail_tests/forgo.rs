//! Closed-window full drives must not commit a preceding block's phase-1 buffers.
use super::*;
use transit_rt::task_graph::{GraphRun, drive};

fn prior_block(rig: &mut PlanRig) {
    host(rig, |h, phase1, _| {
        for task in 0..phase1 {
            assert_eq!(h.render_here(task), Rendered::Usable);
        }
    });
}

/// timing: expired-deadline the drive receives an already-reached deadline, with no race.
#[test]
fn closed_forgo_phase1_contributes_no_previous_block_audio() {
    let (mut rig, ..) = fixture(false);
    prior_block(&mut rig);
    host(&mut rig, |h, phase1, _| {
        assert!(h.p1[..phase1].iter().all(|st| st.rendered.block == h.g - 1));
        assert!(h.p1[..phase1].iter().all(|st| st.buf.iter().flatten().all(|x| *x == 0.75)));
        let graph = h.graph.clone();
        let mut run = GraphRun::new(graph.tasks());
        drive(h, &graph, &mut run, Instant::now());
        for slot in 0..phase1 {
            assert_eq!(h.pool.set.outcome(slot), Some(Outcome::Overrun(OverrunCause::Unstarted)));
        }
        assert_eq!(
            h.tail.as_ref().unwrap().main.iter().flatten().filter(|x| **x != 0.0).count(),
            0,
            "MIX routed a home Forgo strip's previous-block audio"
        );
    });
}

/// Supplies an explicitly completed source to the full drive, without a clock race.
struct CompletedSource<'a, 'b, 'c> {
    host: &'a mut Host<'b, 'c>,
    source: usize,
}
impl GraphHost for CompletedSource<'_, '_, '_> {
    fn set(&mut self) -> &mut WorkerSet {
        self.host.set()
    }
    fn start(&mut self, task: usize, window: Window) -> Start {
        if task == self.source { Start::Ready } else { self.host.start(task, window) }
    }
    fn refused(&mut self, task: usize) {
        self.host.refused(task);
    }
    fn render_here(&mut self, task: usize) -> Rendered {
        self.host.render_here(task)
    }
    fn poll(&mut self, deadline: Instant) -> transit_rt::worker_set::Poll {
        self.host.poll(deadline)
    }
    fn take(&mut self, task: usize, outcome: Outcome) -> bool {
        self.host.take(task, outcome)
    }
    fn skipped(&mut self, task: usize, origin: usize) {
        self.host.skipped(task, origin);
    }
}

/// B3 with one pinned track-key dependent and an explicitly completed current source.
/// timing: expired-deadline the completion is supplied before an already-Closed drive.
#[test]
fn closed_forgo_keeps_the_pinned_dependent_current_and_audible() {
    let (mut rig, mut s, aux, probe) = fixture(false);
    let source_id = s.tracks[0].id;
    s.track_mut(source_id).unwrap().mixer.output = Route::None;
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    s.track_mut(aux).unwrap().mixer.input_monitor = true;
    rig.publish(&s);
    prior_block(&mut rig);
    let before = probe.process.load(Ordering::Relaxed);
    host(&mut rig, |h, phase1, _| {
        let pinned = h.p1.iter().position(|st| st.id == aux).unwrap();
        let source = h.p1.iter().position(|st| st.id == source_id).unwrap();
        assert_eq!(h.render_here(source), Rendered::Usable);
        // Supply distinct current completion samples after rendering/stamping the source;
        // the pinned strip's preceding output remains 0.75 like the Forgo strips.
        for channel in &mut h.p1[source].buf {
            channel.fill(0.25);
        }
        let graph = h.graph.clone();
        let mut run = GraphRun::new(graph.tasks());
        drive(&mut CompletedSource { host: h, source }, &graph, &mut run, Instant::now());
        for slot in (0..phase1).filter(|slot| *slot != pinned && *slot != source) {
            assert_eq!(h.pool.set.outcome(slot), Some(Outcome::Overrun(OverrunCause::Unstarted)));
        }
        assert_eq!(probe.process.load(Ordering::Relaxed), before + 1);
        assert_eq!(probe.key.load(Ordering::Relaxed), 0.25_f32.to_bits());
        assert_eq!(h.p1[pinned].rendered.block, h.g);
        assert!(!h.pool.is_silent(pinned));
        let main = &h.tail.as_ref().unwrap().main;
        assert!(main.iter().flatten().any(|x| *x > 0.0), "B3 pinned dependent must remain audible");
        assert!(main.iter().flatten().all(|x| (0.0..=0.25).contains(x)), "MIX added Forgo residue to the pinned dependent");
    });
}

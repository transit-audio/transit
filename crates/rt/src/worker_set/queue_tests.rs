//! R5/R6: injected stamps and explicit reader/worker steps, never scheduling assertions.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::queue::{CAPTURE_BOUND, QueueSnapshot};
use super::tests::{Behavior, Fake, STALL, set_of};
use super::*;
use crate::testing::measure;

fn publish(set: &mut WorkerSet, slot: usize, stamp: u64, deadline: u64, period: u64) {
    set.queue_deadline_ns = deadline;
    set.queue_period_ns = period;
    assert_eq!(set.publish_stamped(slot, |_| stamp), Ok(Published::Published));
}

fn two_reads(start: Instant, end: Instant) -> impl FnMut() -> Instant {
    let mut next = Some(start);
    move || next.take().unwrap_or(end)
}

/// Mutation: counting refused/Busy/forgone calls as attempts or classifying reclaim by its end
/// time violates these exact totals. Exercises both callback completion times and slot reuse.
#[test]
fn r5_counters_partition_every_published_attempt_exactly_once() {
    let fake = Fake::new(3, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    let shared = Arc::clone(&set.shared);
    let base = Instant::now();
    let at = |ns| base + Duration::from_nanos(ns);
    let origin = shared.figures.nanos(base);
    assert!(set.begin_block(1));
    publish(&mut set, 0, origin, origin + 100, 80);
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    shared.run_attempt(0, token, two_reads(at(20), at(100))); // Equality: neither over queue nor late.
    assert_eq!(set.finish(base, |_| panic!("already done")).mixed, 1);
    assert_eq!(set.publish(0), Err(PublishRefused::Twice));
    assert_eq!(set.shared.queue.counts.snapshot(), QueueSnapshot { published: 1, claimed: 1, classified: 1, ..QueueSnapshot::default() });

    // Pre-deadline reclaim that finishes before, then after its injected deadline.
    for (generation, end) in [(2, 50), (3, 101)] {
        assert!(set.begin_block(generation));
        set.set_cost_estimate(0, Duration::from_nanos(1));
        publish(&mut set, 0, origin, origin + 100, 80);
        assert!(set.reclaim_before_deadline(0, at(10), at(100)));
        let mut renders = 0;
        set.render_inline(0, at(100), &mut |_| renders += 1, two_reads(at(10), at(end)));
        assert_eq!(renders, 1);
        assert_eq!(set.outcome(0), Some(Outcome::Inline));
    }
    assert_eq!(shared.queue.counts.snapshot().inline_pre, 2);
    assert_eq!(shared.queue.counts.snapshot().unclaimed_at_deadline, 0, "a late inline completion is still inline_pre");

    assert!(set.begin_block(4));
    set.set_inline_at_deadline(0);
    publish(&mut set, 0, origin, origin + 100, 80);
    publish(&mut set, 1, origin, origin + 100, 80);
    assert_eq!(set.forgo(2), Ok(Outcome::Overrun(OverrunCause::Unstarted)));
    let mut rendered = Vec::new();
    let r = set.finish(base, |s| rendered.push(s));
    assert_eq!(rendered, [0]);
    assert_eq!((r.inline, r.overrun), (1, 2));

    // Claimed but unfinished at the sweep; next block Busy is not another attempt.
    assert!(set.begin_block(5));
    publish(&mut set, 0, origin, origin + 100, 80);
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    shared.queue.claimed();
    assert_eq!(set.finish(base, |_| panic!("claimed")).overrun, 1);
    assert!(set.begin_block(6));
    assert_eq!(set.publish(0), Ok(Published::Skipped));
    assert_eq!(shared.queue.counts.snapshot().published, 6);
    shared.queue.classify(0, origin + 20, origin + 101); // Late without a long queue.
    token.complete();

    // Reuse: long queue+late, late claim stamp, then a caught runner fault, all classified once.
    for (generation, claim, end, panic) in [(7, 21, 101, false), (8, 101, 110, false), (9, 1, 2, true)] {
        assert!(set.begin_block(generation));
        publish(&mut set, 0, origin, origin + 100, 80);
        if panic {
            fake.set(0, Behavior::Panic);
        }
        let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
        shared.run_attempt(0, token, two_reads(at(claim), at(end)));
        set.finish(base, |_| panic!("already done"));
    }
    assert_eq!(set.outcome(0), Some(Outcome::Faulted));
    let counts = shared.queue.counts.snapshot();
    assert_eq!(
        counts,
        QueueSnapshot {
            published: 9,
            claimed: 5,
            classified: 5,
            inline_pre: 2,
            unclaimed_at_deadline: 2,
            deadline_reclaimed_tail: 1,
            forgone: 1,
            queue_over: 2,
            late_w: 3,
            late_after_queue: 2,
            tail_callback: 0,
        }
    );
    assert!(counts.conserves());
    assert_eq!(set.stats().runner_panics, 1);
}

/// Mutation: overwriting the stamps on refusal, or stamping after publish, changes classification.
#[test]
fn r5_publication_stamps_precede_claim_and_refusal_does_not_replace_them() {
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    let shared = Arc::clone(&set.shared);
    assert!(set.begin_block(1));
    let mut reads = 0;
    set.queue_deadline_ns = 100;
    set.queue_period_ns = 80;
    assert_eq!(
        set.publish_stamped(0, |_| {
            reads += 1;
            10
        }),
        Ok(Published::Published)
    );
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    assert_eq!(
        set.publish_stamped(0, |_| {
            reads += 1;
            1000
        }),
        Err(PublishRefused::Twice)
    );
    assert_eq!(reads, 1, "one read per successful attempt, none for refusal");
    shared.queue.claimed();
    shared.queue.classify(0, 31, 101);
    token.complete();
    let c = shared.queue.counts.snapshot();
    assert_eq!((c.queue_over, c.late_w, c.late_after_queue), (1, 1, 1));
    assert!(c.conserves());
}

/// Mutation: moving the stamp stores after the publish operation makes the immediate claimant
/// see the preceding attempt's stamps. The publish adapter steps the worker at the Release
/// boundary, before returning to the publisher; there is no scheduler race to win.
#[test]
fn r5_worker_can_classify_before_the_publish_call_returns() {
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    let shared = Arc::clone(&set.shared);
    assert!(set.begin_block(1));
    shared.queue.stamp(0, 1_000, 1_000, 1_000); // Distinct preceding attempt.
    set.queue_deadline_ns = 100;
    set.queue_period_ns = 80;
    assert_eq!(
        set.publish_with(
            0,
            |_| 10,
            |jobs, slot| {
                jobs.publish(slot)?;
                let token = jobs.slot(slot).unwrap().try_claim().unwrap();
                shared.queue.claimed();
                shared.queue.classify(slot, 31, 101);
                token.complete();
                Ok(())
            }
        ),
        Ok(Published::Published)
    );
    let c = shared.queue.counts.snapshot();
    assert_eq!((c.queue_over, c.late_w, c.late_after_queue), (1, 1, 1));
    assert!(c.conserves());
}

/// Mutation: capturing before settlement produces a response with the final claim unclassified;
/// capturing after monitoring publication includes an attempt beyond the requested boundary.
#[test]
fn r6a_capture_waits_for_final_claim_and_excludes_later_publications() {
    let mut set = set_of(&Fake::new(2, Duration::ZERO), 0);
    let shared = Arc::clone(&set.shared);
    let mut reader = set.measurement_reader().unwrap();
    assert!(set.measurement_reader().is_none(), "only one consumer");
    assert!(set.begin_block(1));
    publish(&mut set, 0, 10, 100, 80);
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    shared.queue.claimed();
    set.finish(Instant::now(), |_| panic!("claimed"));
    assert!(reader.request(41));
    set.capture_measurement(false);
    assert!(reader.read().is_none());
    assert!(!reader.request(42));

    assert!(set.begin_block(2));
    set.capture_measurement(false);
    publish(&mut set, 1, 20, 100, 80); // Monitoring keeps rendering while the request is pending.
    set.finish(Instant::now(), |_| panic!("no history"));
    shared.queue.classify(0, 35, 101);
    token.complete();
    assert!(set.begin_block(3)); // Returns the late result before the capture boundary.
    set.capture_measurement(true);
    publish(&mut set, 1, 30, 100, 80);
    let response = reader.read().unwrap();
    assert_eq!(response.request, 41);
    assert!(response.complete && response.counters.conserves());
    assert_eq!(response.counters.published, 2, "the third publication is after capture");
    assert_eq!(shared.queue.counts.snapshot().published, 3);
    set.finish(Instant::now(), |_| panic!("no history"));
}

/// Mutation: dropping the claimed==classified equation accepts this deliberately corrupt row.
#[test]
fn r6b_missing_classification_violates_conservation() {
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    let mut reader = set.measurement_reader().unwrap();
    assert!(set.begin_block(1));
    publish(&mut set, 0, 10, 100, 80);
    let token = set.shared.jobs.slot(0).unwrap().try_claim().unwrap();
    set.shared.queue.claimed();
    token.complete(); // Deliberately omit the worker's classification.
    set.finish(Instant::now(), |_| panic!("done"));
    assert!(reader.request(1));
    set.capture_measurement(true);
    let response = reader.read().unwrap();
    assert!(response.complete);
    assert!(!response.counters.conserves(), "a complete but non-conserving snapshot is invalid");
}

/// Mutation: waiting forever or using a different handoff for incomplete responses fails here.
#[test]
fn r6c_unsettled_request_is_incomplete_at_one_thousand_blocks() {
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    let shared = Arc::clone(&set.shared);
    let mut reader = set.measurement_reader().unwrap();
    assert!(set.begin_block(1));
    publish(&mut set, 0, 10, 100, 80);
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    shared.queue.claimed();
    assert!(reader.request(71));
    for _ in 1..CAPTURE_BOUND {
        set.capture_measurement(false);
        assert!(reader.read().is_none());
    }
    set.capture_measurement(false);
    assert!(!reader.request(72));
    // Even a successful boundary cannot overwrite an unacknowledged incomplete payload.
    shared.queue.classify(0, 11, 99);
    token.complete();
    assert!(set.begin_block(2));
    set.capture_measurement(true);
    let response = reader.read().unwrap();
    assert_eq!(response.request, 71);
    assert!(!response.complete && !response.counters.conserves());
    assert_eq!(response.counters.classified, 0);
    assert!(reader.request(72));
    set.capture_measurement(true);
    let complete = reader.read().unwrap();
    assert_eq!(complete.request, 72);
    assert!(complete.complete && complete.counters.conserves());
}

/// Mutation: overwrite before acknowledgment. All boundaries conserve identically, but their
/// queue/late bits differ, so conservation cannot disguise a torn or replaced payload.
#[test]
fn r6d_reader_paused_mid_copy_across_two_capture_boundaries() {
    use std::sync::mpsc::sync_channel;
    let mut set = set_of(&Fake::new(0, Duration::ZERO), 0);
    let mut reader = set.measurement_reader().unwrap();
    let c = &set.shared.queue.counts;
    c.published.store(4, Ordering::Relaxed);
    c.claimed.store(4, Ordering::Relaxed);
    c.classified.store(4, Ordering::Relaxed);
    c.queue_over.store(1, Ordering::Relaxed);
    c.late_w.store(2, Ordering::Relaxed);
    c.late_after_queue.store(1, Ordering::Relaxed);
    let original = c.snapshot();
    assert!(reader.request(81));
    set.capture_measurement(true);
    let mailbox = Arc::clone(&set.mailbox);
    let (paused_tx, paused_rx) = sync_channel(0);
    let (resume_tx, resume_rx) = sync_channel(0);
    let consumer = std::thread::spawn(move || {
        let mut fields = 0;
        let response = reader
            .copy_with(|| {
                fields += 1;
                if fields == 1 {
                    paused_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }
            })
            .unwrap();
        (reader, response)
    });
    paused_rx.recv_timeout(STALL).unwrap(); // Observable mid-copy state; timeout only bounds a hang.
    for (queued, late, both) in [(3, 4, 3), (4, 1, 0)] {
        let c = &set.shared.queue.counts;
        c.queue_over.store(queued, Ordering::Relaxed);
        c.late_w.store(late, Ordering::Relaxed);
        c.late_after_queue.store(both, Ordering::Relaxed);
        assert!(c.snapshot().conserves());
        assert!(!mailbox.request(82), "a second request attempted while the consumer is mid-copy");
        let ((), allocations) = measure(|| set.capture_measurement(true));
        assert!(allocations.is_zero());
    }
    // Reaching this release proves the two callback boundaries did not wait on the reader.
    resume_tx.send(()).unwrap();
    let (mut reader, response) = consumer.join().unwrap();
    assert_eq!(response.request, 81);
    assert!(response.complete);
    assert_eq!(response.counters, original, "every copied bit belongs to the first capture");
    assert!(reader.request(82));
    set.capture_measurement(true);
    let next = reader.read().unwrap();
    assert_eq!(next.request, 82);
    assert_eq!((next.counters.queue_over, next.counters.late_w, next.counters.late_after_queue), (4, 1, 0));
}

/// Mutation: repeated capture or publication accounting allocates on the callback.
#[test]
fn queue_capture_and_seam_installation_allocate_nothing() {
    let mut set = set_of(&Fake::new(2, Duration::ZERO), 0);
    let mut reader = set.measurement_reader().unwrap();
    let ((), allocations) = measure(|| {
        for block in 1..=1_200 {
            set.install_test_seams();
            assert!(set.begin_block(block));
            assert!(reader.request(block));
            set.capture_measurement(true);
            assert!(reader.read().unwrap().counters.conserves());
            set.set_inline_at_deadline(0);
            publish(&mut set, 0, 10, 100, 80);
            set.finish(Instant::now(), |_| {});
        }
    });
    assert!(allocations.is_zero(), "{allocations:?}");
}

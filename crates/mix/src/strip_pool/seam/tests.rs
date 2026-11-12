//! E72 and #294: the held-strip claim watch is decided by progress, never by how long the pool waited.
//! The clock and the progress reads are injected, so no test sleeps.

use super::{CLAIM_HANG, ClaimWatch};
use std::time::Duration;

#[test]
fn claims_that_keep_arriving_outlive_the_ceiling() {
    let mut watch = ClaimWatch::new(0);
    // A starved host: each claim lands after a pause longer than the old 60 s bound, and the total wait
    // exceeds the ceiling. Only lack of progress matters.
    for claimed in 1..=20 {
        assert!(!watch.stalled(|| claimed, || Duration::from_secs(claimed as u64 * 61)));
    }
}

#[test]
fn a_pool_that_waited_a_minute_for_a_worker_is_not_stalled() {
    let mut watch = ClaimWatch::new(0);
    assert!(!watch.stalled(|| 0, || Duration::from_secs(61)), "the old 60 s bound is gone (E72)");
    assert!(!watch.stalled(|| 0, || CLAIM_HANG - Duration::from_nanos(1)));
}

#[test]
fn no_claim_for_the_whole_ceiling_is_a_hang() {
    let mut watch = ClaimWatch::new(0);
    assert!(watch.stalled(|| 0, || CLAIM_HANG));
}

#[test]
fn a_stopped_worker_exhausts_the_budget_since_its_last_claim() {
    let mut watch = ClaimWatch::new(0);
    let advanced = CLAIM_HANG * 2;
    assert!(!watch.stalled(|| 1, || advanced), "progress after a descheduled observer wins");
    assert!(!watch.stalled(|| 1, || advanced + CLAIM_HANG - Duration::from_nanos(1)));
    assert!(watch.stalled(|| 1, || advanced + CLAIM_HANG));
}

#[test]
fn an_observer_pause_cannot_pair_old_progress_with_new_time() {
    let mut watch = ClaimWatch::new(0);
    let progress = std::cell::Cell::new(0);
    assert!(!watch.stalled(
        || progress.get(),
        || {
            // A worker claims while the observer is paused in its clock read.
            progress.set(1);
            CLAIM_HANG * 2
        },
    ));
}

#[test]
fn progress_observed_after_a_pause_renews_at_the_observation_time() {
    let mut watch = ClaimWatch::new(0);
    let mut first_read = true;
    assert!(!watch.stalled(|| 1, || if std::mem::take(&mut first_read) { Duration::from_secs(1) } else { CLAIM_HANG * 2 }));
    assert!(!watch.stalled(|| 1, || CLAIM_HANG * 2));
    assert!(watch.stalled(|| 1, || CLAIM_HANG * 3));
}

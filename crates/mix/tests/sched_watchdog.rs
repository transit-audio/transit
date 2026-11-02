//! E49: the scheduled hold's duration is not evidence that its owner stopped progressing.
#[allow(dead_code)]
#[path = "common/stall.rs"]
mod stall;

use stall::{HANG, Watchdog};
use std::time::Duration;

#[test]
fn progressing_blocks_can_outlive_the_hold_watchdog() {
    let mut watchdog = Watchdog::new(0);
    // A worker observes each completed block after a pause longer than the old 60 s bound.
    // Total hold duration also exceeds the last-resort ceiling; only lack of progress matters.
    for block in 1..=20 {
        assert!(!watchdog.stalled(|| block, || Duration::from_secs(block as u64 * 61)));
    }
}

#[test]
fn a_stopped_owner_exhausts_the_budget_since_its_last_progress() {
    let mut watchdog = Watchdog::new(0);
    let advanced = HANG * 2;
    assert!(!watchdog.stalled(|| 1, || advanced), "progress after a descheduled observer wins");
    assert!(!watchdog.stalled(|| 1, || advanced + HANG - Duration::from_nanos(1)));
    assert!(watchdog.stalled(|| 1, || advanced + HANG));
}

#[test]
fn an_observer_pause_cannot_pair_old_progress_with_new_time() {
    let mut watchdog = Watchdog::new(0);
    let progress = std::cell::Cell::new(0);
    assert!(!watchdog.stalled(
        || progress.get(),
        || {
            // The owner advances while the observer is paused in its clock read.
            progress.set(1);
            HANG * 2
        },
    ));
}

#[test]
fn progress_observed_after_a_pause_renews_at_the_observation_time() {
    let mut watchdog = Watchdog::new(0);
    let mut first_read = true;
    assert!(!watchdog.stalled(|| 1, || if std::mem::take(&mut first_read) { Duration::from_secs(1) } else { HANG * 2 }));
    assert!(!watchdog.stalled(|| 1, || HANG * 2));
    assert!(watchdog.stalled(|| 1, || HANG * 3));
}

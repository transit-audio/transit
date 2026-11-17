//! ADR 0062 section 6: the record start's stage through its ticket-owned state word. The audio
//! side publishes each stage by one compare-and-swap from the matching ticket's non-terminal
//! state, never retried; Main's cancel retries when it loses to an advance and stops on a terminal
//! stage. The interleavings are driven deterministically through `cancel_with`.
#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use transit_rt::testing::measure;

fn point(n: u64) -> Point {
    Point { host_ns: n * 1000, output_frame: n, position: Position { sample: n as i64, fraction: 0, denominator: 1 } }
}

#[test]
fn a_late_stage_write_never_overwrites_cancelled() {
    let (mut control, mut audio) = pair(EngineEpoch(1));
    let reader = control.prepare().unwrap();
    let t = reader.ticket();
    reader.cancel();
    assert_eq!(reader.status(), Status::Cancelled);
    let (written, heap) =
        measure(|| [audio.counting_in(t), audio.waiting(t), audio.start(t, point(1)), audio.fail(t, Some(StartFailure::Clock)), audio.fail(t, None)]);
    assert!(heap.is_zero(), "{heap:?}");
    assert_eq!(written, [false; 5], "no stage is published over Cancelled");
    audio.cancel(t);
    assert_eq!(reader.status(), Status::Cancelled);

    // Nor over the other terminal stages: Failed keeps its reason, Started its boundary.
    let failed = control.prepare().unwrap();
    assert!(audio.counting_in(failed.ticket()));
    assert!(audio.fail(failed.ticket(), Some(StartFailure::Speed)));
    failed.cancel();
    control.cancel();
    assert!(!audio.waiting(failed.ticket()) && !audio.start(failed.ticket(), point(2)) && !audio.fail(failed.ticket(), None));
    assert_eq!(failed.status(), Status::Failed(StartFailure::Speed));
    let started = control.prepare().unwrap();
    assert!(audio.waiting(started.ticket()));
    assert!(audio.start(started.ticket(), point(3)));
    started.cancel();
    assert!(!audio.fail(started.ticket(), Some(StartFailure::Clock)) && !audio.counting_in(started.ticket()));
    assert_eq!(started.status(), Status::Started(point(3)));

    // A stale ticket's stage never touches the ticket that superseded it, and reads as Cancelled.
    let old = control.prepare().unwrap();
    let new = control.prepare().unwrap();
    assert!(!audio.counting_in(old.ticket()) && !audio.fail(old.ticket(), Some(StartFailure::Position)));
    assert_eq!((old.status(), new.status()), (Status::Cancelled, Status::Pending));
    // Advances are monotonic: no stage goes back, and repeating the current one writes nothing.
    assert!(audio.waiting(new.ticket()));
    assert!(audio.waiting(new.ticket()), "already waiting");
    assert!(!audio.counting_in(new.ticket()), "Waiting never returns to CountingIn");
    assert_eq!(new.status(), Status::Waiting);
}

#[test]
fn main_cancel_wins_over_each_stage_advance() {
    // `advances` stage advances land between Main's reads and its compare-and-swaps, one per
    // attempt: the cancel loses each time, looks again, and wins on the next attempt.
    let stages: [fn(&mut Publisher, Ticket) -> bool; 2] = [Publisher::counting_in, Publisher::waiting];
    for advances in 0..=2 {
        let (mut control, mut audio) = pair(EngineEpoch(1));
        let reader = control.prepare().unwrap();
        let t = reader.ticket();
        let mut attempts = Vec::new();
        let cancelled = cancel_with(&reader.shared, t, |attempt| {
            attempts.push(attempt);
            if attempt < advances {
                assert!(stages[attempt](&mut audio, t), "advance {attempt} lands");
            }
        });
        assert!(cancelled, "{advances} advances");
        assert_eq!(attempts, (0..=advances).collect::<Vec<_>>(), "one attempt per lost race, then the win");
        assert_eq!(reader.status(), Status::Cancelled);
        assert!(!audio.start(t, point(1)), "the start that follows the advances cannot publish");
    }
    // A terminal stage between the read and the compare-and-swap ends the cancel: it never
    // overwrites Started or Failed, after any advance.
    let terminals: [fn(&mut Publisher, Ticket) -> bool; 3] =
        [|a, t| a.start(t, point(7)), |a, t| a.fail(t, Some(StartFailure::Timeline)), |a, t| a.fail(t, None)];
    for (which, terminal) in terminals.iter().enumerate() {
        for advances in 0..=2 {
            let (mut control, mut audio) = pair(EngineEpoch(1));
            let reader = control.prepare().unwrap();
            let t = reader.ticket();
            let mut attempts = 0;
            let cancelled = cancel_with(&reader.shared, t, |attempt| {
                attempts += 1;
                if attempt < advances {
                    assert!(stages[attempt](&mut audio, t));
                } else if attempt == advances {
                    assert!(terminal(&mut audio, t));
                }
            });
            assert!(!cancelled, "terminal {which} after {advances} advances");
            assert_eq!(attempts, advances + 1, "the cancel stops on the terminal stage");
            let want = [Status::Started(point(7)), Status::Failed(StartFailure::Timeline), Status::Cancelled][which];
            assert_eq!(reader.status(), want);
        }
    }
}

#[test]
fn stage_exhaustion_is_refused() {
    let (mut control, mut audio) = pair(EngineEpoch(1));
    control.next = u64::MAX - 15;
    let last = control.prepare().unwrap();
    assert_eq!(last.ticket().sequence, u64::MAX - 7, "the last identity leaves room for three flag bits");
    assert!(control.prepare().is_none(), "exhaustion is refused, never an aliased identity");
    // Every stage of the last ticket is representable.
    assert!(audio.counting_in(last.ticket()));
    assert_eq!(last.status(), Status::CountingIn);
    assert!(audio.waiting(last.ticket()));
    assert_eq!(last.status(), Status::Waiting);
    assert!(audio.fail(last.ticket(), Some(StartFailure::Position)));
    assert_eq!(last.status(), Status::Failed(StartFailure::Position));
    // The four-step fixture of the two-bit field is no longer a valid last identity.
    let (mut control, _) = pair(EngineEpoch(1));
    control.next = u64::MAX - 7;
    assert!(control.prepare().is_none());
}

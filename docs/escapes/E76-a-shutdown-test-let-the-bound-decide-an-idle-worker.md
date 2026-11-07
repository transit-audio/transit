# E76: a shutdown test let the bound decide an idle worker's fate (#346)

1. **Failure.** `worker_set::tests::shutdown_quarantines_a_stalled_worker_within_its_bound`
   (`crates/rt/src/worker_set/tests.rs`) failed twice in one day in the `transit-rt` release
   test run, both times on branches that do not touch `crates/rt`. In a review gate, with nine
   compiler processes on the host, `shutdown` took 412 ms against `BOUND + SLACK` (200 ms plus
   200 ms). In a landing chain's rt release gate, before the push, the report was
   `Quarantine { workers: [0, 1], strips: [1] }` against an expected single worker: the idle
   worker was quarantined beside the stalled one. Neither run was repeated; nothing landed.
2. **Mechanism.** `WorkerSet::shutdown(bound)` (`crates/rt/src/worker_set.rs`) sets the stop
   flag, wakes every worker explicitly (its `woken` flag and an unpark, so no worker depends on
   a poll) and then joins each one against a single deadline `bound` after the wakes. A worker
   that has not exited by the deadline is abandoned and reported, as documented. The idle
   worker's exit depends only on when the OS schedules the woken thread; on a loaded host that
   can take longer than 200 ms, and the product correctly reports a worker that did not return.
   The report stays correct where it matters (the idle worker was inside no strip, so `strips`
   was `[1]`; at process exit the cost is one leaked idle thread). This is a test defect, not a
   product defect: the test let a 200 ms wall-clock bound decide who was quarantined, and it
   held an upper wall-clock bound on the whole call. A debug-only 300 ms sleep at worker exit
   reproduces the landing chain's message on every run of the old test.
3. **Why checks missed it.** `cargo xtask timing` flags short bounds (under 100 ms) and short
   paced loops; a 200 ms bound plus a 200 ms slack is above both thresholds, and the join's
   deadline decides an outcome inside the product call, not in a loop the scan sees. The
   neighboring idle-set test held a 10 s upper bound of the same kind, which was simply too
   generous to fail.
4. **Regression test.** `shutdown` is now its two halves, `stop_and_wake` and `join_within`,
   with no behavior change. The test's single named helper, `shutdown_once_returned`, runs the
   stop and the wakes, waits (under the 60 s stall bound) until the idle worker has exited, and
   only then starts the bound. The test finds the stalled worker by the strip it is inside and
   asserts the exact report (only that worker, strip 1), that `shutdown` returned while the
   stalled worker was still inside the strip (it never waits on it), the `Claimed` slot, and
   the bound as a lower bound only, which holds by construction on a monotonic clock. The
   `took < BOUND + SLACK` assertion is gone; `SLACK` now covers `finish` only. The idle-set
   test asserts every worker joined under the stall bound and drops its 10 s ceiling. Controls:
   releasing the stall (the stall path removed) fails the report assertion with an empty
   quarantine; removing the wake from `stop_and_wake` fails the stalled-worker test at its
   stall bound (the idle worker stays parked). With the 300 ms exit sleep the new tests pass.
   Twenty release runs during a parallel release build of the workspace passed.
5. **Gate class.** E49: a bounded join or wait must not let its bound decide the fate of a
   thread whose progress depends only on scheduling. Wait (stall-bounded) for every thread the
   test expects to return before starting the bound, so the bound abandons only what the test
   holds; assert the bound only as a lower bound by construction, never as an upper bound.
6. **Protocol change, owner, blind spot.** The rule is in
   `docs/contributing/evidence/gates.md`. Owner: the maintainer. Blind spots: the idle-set test
   does not prove the wake on shutdown, because freshly spawned workers may see the stop before
   they first park; the stalled-worker test does, unless its idle worker is preempted between
   its last sweep and its park (a false pass, never a false failure).
   `retirement_quarantines_a_worker_that_does_not_return` (`crates/mix/tests/render_workers.rs`)
   still holds a 2 s upper bound on a 500 ms retirement; it is tracked on #346 for the same
   treatment. The rule is a review rule; the timing scan does not see a deadline inside a
   product call.

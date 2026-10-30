# E66: a held-source test let its deadline pass before the worker wake

1. **Failure.** `strip_pool::graph_tests::a_graph_change_waits_for_the_held_task_and_the_old_result_is_never_read`
   (`crates/mix/src/strip_pool/graph_tests.rs`) failed on the macOS CI runner at 8208eb0b (run
   41135685553): the pinned strip's `Wait` plug-in spun for its 60 s stall bound ("the hold was
   never entered"), then the callback thread's join failed. It had passed in the 29 earlier macOS
   runs, and on Windows and in the WSL Linux gate at the same sha. No user-facing defect: the
   product did what it is built to do.
2. **Mechanism.** `deadline_block` set an explicit block deadline 5 ms after its own clock read,
   armed the pinned strip to wait until the worker entered the key source's hold, and rendered.
   The graph driver (`crates/rt/src/task_graph.rs`, `Driver::wake`) wakes the workers for what it
   published only while `Instant::now() < deadline`, by design: a worker started past the deadline
   would hold its strip into the next block. If the callback thread lost 5 ms between the clock
   read and the wake (a preempted thread on a loaded runner), the key source was published but no
   worker was woken, the parked worker never claimed it, and the pinned strip rendered next and
   waited for an `entered` flag no worker could set. The poll that would have taken the source back
   at the deadline runs only after the pinned strip returns, so the block could never finish. A
   protocol deadlock opened by a scheduling event, not a slow test: the 60 s bound was not a
   starved verdict (E61) and more time would not have helped. The two sibling tests that use the
   same helper with the handshake (`a_late_then_busy_key_source_...`,
   `an_aux_keyed_from_a_skipped_aux_is_skipped_too`) had the same defect.
3. **Why checks missed it.** The local gates run on hosts where the callback reaches the wake in
   microseconds, and a worker still sweeping after the previous block can claim the source without
   a wake, so the window needs a stall of the test thread at one exact point. The #226 audit marked
   the four tests OK ("held task, explicit block deadline") and `cargo xtask timing` rule (e)
   accepts an explicit instant (`set_block_deadline`) as a deadline of the test's own: neither saw
   that the explicit instant also decides whether the worker is woken, so a short one is a
   wall-clock outcome (E49 class) even with a held worker.
4. **Regression test.** Reproduced on Windows by a debug-only 6 ms sleep between the deadline and
   the render: all three handshake tests fail every run with the CI message. The helper now gives
   the block a far strip deadline (`STALL`) and makes the held source miss it through the
   deterministic deadline (`MixEngine::hold_until_alone`, `strip_pool::seam`: the poll's deadline
   comes once only the held strip is unresolved and a worker has claimed it); the pinned strip
   waits only on `entered`. With debug-only 50 ms sleeps before the render and before the worker
   enters the hold, all five graph tests pass; 40 runs of the graph tests pinned to one core and 5
   full `transit-mix` lib runs pinned to two cores passed. Intent and assertions unchanged.
5. **Gate class.** A test that waits for a worker to enter a hold keeps the block's real deadline
   far and lets the deterministic deadline decide the miss; a short explicit instant is a
   wall-clock bound when anything waits on the worker it gates.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`,
   and the audit rows are corrected. Owner: the coordinator. Blind spot: rule (e) cannot tell an
   explicit instant that only decides outcomes from one a wait depends on; it stays a review item.

See also: [E49](E49-echo-latency-test-wall-clock-bound.md) and [E61](E61-a-starved-gate-run-shared-the-host-with-cold-builds.md).

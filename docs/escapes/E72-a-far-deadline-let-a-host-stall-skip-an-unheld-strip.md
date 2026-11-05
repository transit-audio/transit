# E72: a far deadline let a host stall skip an unheld strip

1. **Failure.** The WSL Linux gate of chain ED (dcd4a36d, #332 part 2, 2026-11-04) failed
   `crates/mix/tests/sched_equivalence.rs::the_graph_equals_the_muted_serial_render_under_seeded_stall_schedules`,
   1 of 22 cases: "both latent, the key early" through 2 workers, seed 26, property 1 at
   block 36, strip 5 (Bass): "an overrun of its own, not held by the schedule (outcome
   Some(Overrun(Unstarted)) ...)". The Windows workspace run on the same sha had passed. The
   failing case took about 61 s (72.35 s for the 22 cases, 11.04 s for the 21 that passed), the
   shrinker removed none of the 12 holds (no smaller schedule failed), and the recheck of the
   printed schedule passed. That pattern points to a one-off event, not a defect that the
   schedule reproduces.
2. **Mechanism.** The harness gave every graph render a strip deadline of `FAR`, 60 s after the
   block's start (`crates/mix/tests/common/sched.rs`, `set_strip_deadline(Some(FAR))`), and
   armed the deterministic deadline each block (`MixEngine::hold_until_alone`). The seam
   (`crates/mix/src/strip_pool/seam.rs`, `gated_deadline`) returned that real deadline while any
   published strip outside the held set was unresolved. At it, the driver closes its window
   (`crates/rt/src/task_graph.rs`, `Driver::start`: ready strips are forgone `Unstarted`), and the
   poll overruns every pending strip (`crates/rt/src/worker_set.rs`, `overrun_strip`: still
   published, `Unstarted`). In block 36, Filler 0 was held on one worker. Bass had overrun `Late`
   in block 33 (its own hold), so `OVERRUN_BAR_BLOCKS` (8) barred it from inline renders through
   block 41 (`worker_set/admission.rs`, `bar`; a block count, not a measured duration). Only the
   second worker could render it, so the block waited on that worker's claim. Something stalled for
   at least the remaining 60 s, and Bass missed the block by the clock. None of the
   measured-duration refusals (`inline_refused_worst_case`, a one-nanosecond history against
   60 s) took part. The 60 s deadline itself decided. Which thread stalled, and why, could not be
   determined: the WSL VM had booted at 20:50:46 for the gate, its kernel log did not survive the
   restart, and the Windows System and Application logs show no sleep, resume or
   resource-exhaustion event in the window. Reproduction: seed 26 passed 10 of 10 runs each in WSL
   on dcd4a36d and on cd5afd4f, and on Windows on cd5afd4f. Seed 26 also passed 300 of 300 WSL runs
   on cd5afd4f, and the full seeds passed 20 of 20 runs with the gate's own binary. With `FAR`
   shortened to 2 s, uncommitted, a 3 s `SIGSTOP` at a random point failed 5 of 40 seed-26 runs and
   1 of 30 full runs with the same signature: an unheld strip `Unstarted` or `Late`, "an overrun
   of its own". With the real `FAR`, a 65 s `SIGSTOP` failed 0 of 3 full runs. A stall fails the
   test only when it lands while an unheld strip is pending, which is why the failure is rare.
   The #332 run-in (`crates/mix/src/offline.rs` and the re-export in `lib.rs`) does not touch the
   realtime path, and the stall reproduces on cd5afd4f, so the run-in is exonerated.
3. **Why checks missed it.** E66 made "keep the block's real deadline far, let the deterministic
   deadline decide the miss" the rule, and `cargo xtask timing` rule (e) accepts a strip deadline
   of 10 s or more (`far-deadline`) as a test's own deadline. Both treat a far deadline as no
   deadline. Property 1 asserts that no strip outside the schedule ever overruns. That is a
   wall-clock outcome bounded by the far deadline, although the harness's doc said "nothing here
   is a clock or a thread-timing outcome" (E49). The held strips' refusal had the same coupling:
   an hour of cost history refuses an inline render only against a shorter deadline. With the
   clock deadline removed, the first full run of the fix failed every case with "held by the
   schedule, but rendered (Inline)".
4. **Fix and regression test.** The armed deterministic deadline now has no clock deadline. While
   `hold_until_alone` is armed, also with nothing held, the pool's deadline is `seam::never` (ten
   years from the block's start), so every strip outside the held set is waited for. The deadline
   comes only when the held strips are the only ones unresolved and, with workers, each is
   claimed. `MixEngine::clock_deadline` disarms it. The harness arms it from the first block,
   drops `FAR`, and gives held strips a cost history of `Duration::MAX`. A hang guard aborts the
   run when no block finishes for `common::stall::HANG`: a hang is a failure, never a counted skip.
   `CLAIM_BOUND`, the seam's last-resort panic, is aligned to the same ten minutes. Regression
   test: `strip_pool::graph_tests::the_deterministic_deadline_never_lets_the_clock_skip_a_strip_it_does_not_hold`
   uses a zero strip deadline, so the clock has ended every block before it starts. Armed with
   nothing held, every strip renders; armed with the kick held, the kick is `Late` with two
   workers and `Unstarted` with none, and only its dependents miss. On cd5afd4f the test fails
   ("0 workers, TrackId(1): Overrun(Unstarted)"); with the fix it passes. With the fix, seed 26
   passed 40 of 40 runs under a 3 s `SIGSTOP` and 3 of 3 full runs under a 65 s one.
5. **Gate class.** E49 (Derek's rule: no test asserts a scheduling or real-clock outcome; ADR 0029
   amendment section 8, "realized through deterministic seams only"). A far deadline is still a
   clock. A test whose input is which strips miss a block takes the clock away completely, and a
   hang ends in a guard's failure, not in an outcome the test counts. E66's rule is kept for its
   case (the worker wake) and narrowed: "far" stops a wake from being lost, but it does not make
   an outcome deterministic.
6. **Protocol change, owner, blind spot.** The coordinator owns the E49 rule in
   `docs/contributing/evidence/gates.md`. This landing adds the narrowed rule as a bullet next
   to E66's, and ADR 0029 section 8 now says the deterministic deadline leaves no clock deadline
   otherwise. Open items, tracked as a follow-up issue: in `cargo xtask timing` rule (e),
   `far-deadline` stops satisfying a test that asserts a per-strip outcome (such a test arms the
   deterministic deadline instead; the scan needs the outcome assertions' provenance, and the
   trial in E70 shows how a function-local check over-rejects), and an audit of the other pool
   tests that assert per-strip outcomes under a 30 s or 60 s strip deadline
   (`strip_pool/tail_tests.rs`, `render_pool.rs` tests, `crates/engine/tests/modulator_insert.rs`),
   which belong to the same class.

# E62: an autosave test polled the clock for a background write

1. **Failure.** `recovery_tests::every_change_is_autosaved_within_a_second`
   (`crates/ui-studio/src/recovery_tests.rs`) failed on a loaded Linux host (#189) at the
   `assert!(eventually(...))` after a change. It passed on the quiet rerun. No user-facing defect:
   the product writes the autosave.
2. **Mechanism.** The test handed a write to the autosave thread and then polled the folder every
   10 ms, at most 300 times (3 s), through the `eventually` helper. The thread writes the session
   to disk with a synced write; on a host whose CPUs were taken away (the E61 class, and a wall-clock bound like E49), the thread was
   not scheduled, or the write not finished, inside the 3 s. The bound measured the host's load,
   not the product. The product already offered the completion signal: `Autosaver::flush`
   (`crates/engine/src/autosave.rs`) sends a `Flush` job through the writer's own queue and
   returns when everything handed over so far is on disk, and it is what `Save` and
   `write_autosave` use (`crates/ui-studio/src/host/autosave.rs`, `autosave_to`). `landed(ticket)`
   is the non-blocking form. No seam was needed, and the audio path is untouched.
3. **Why checks missed it.** `cargo xtask timing` rule (d) (`xtask/src/timing_loops.rs`) flags
   sleep, park and yield loops that exit on a short wall-clock deadline (an `Instant` or
   `elapsed()` compared against a short `Duration`). This loop's bound was an iteration count of
   `sleep` calls (300 x 10 ms) in a helper, with no clock read, so it was outside what the lint
   checks. On
   a quiet host the write lands in a few milliseconds, so every normal gate passed.
4. **Regression test.** The test now calls `flush()` on the app's autosaver after each change and
   then asserts on the folder, with no clock in it. If a change did not hand a write over, the
   `expect` or the assertion fails at once instead of timing out. Intent unchanged: every change
   is autosaved. After the fix: 50 of 50 single-test runs passed on Windows. The sibling
   `an_exit_without_quit_offers_unsaved_changes_at_the_next_launch` polled the same way for the
   folder deleted after `session.save`; it flushes now too, and the `eventually` helper is gone.
5. **Gate class.** A test that waits on a background thread waits on its completion signal (a
   join, a counter or ticket, a channel, a synchronous flush), never on a wall-clock bound.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`.
   Owner: the coordinator. Blind spot: `cargo xtask timing` does not see an iteration-counted
   poll loop (its loop rule keys on wall-clock exits); it stays a review item until a lint for `sleep` inside a `for` poll is worth it.

See also: [E49](E49-echo-latency-test-wall-clock-bound.md) and [E61](E61-a-starved-gate-run-shared-the-host-with-cold-builds.md), the other wall-clock-bound escapes.

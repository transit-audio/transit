# E60: a recovered-autosave test left the save order to the clock

1. **Failure.** `autosave::tests::unsaved_work_is_offered_once_its_session_closes`
   (`crates/engine`) panicked at `crates/engine/src/autosave.rs:858`,
   `assertion failed: leftovers(&root).is_empty()`, in the Linux gate of the #172 chain at
   717043d5, a tree that does not touch autosave (#175). The same tree passed the Windows gate. No
   user-facing defect: the product removes a recovered leftover explicitly (below).
2. **Mechanism.** `leftovers` offers an autosave folder whose session file is at least as new as
   the session file it came from (`saved >= origin_saved`). The `>=` is deliberate (3c9d4fa7,
   `work_autosaved_within_the_saves_timestamp_tick_is_still_offered`): file times are only as fine
   as the kernel tick, and work autosaved right after a save, in the same tick, must not be taken
   for older. The test writes an autosave, recovers it, saves the recovered session with
   `session.save`, and expects `leftovers` to judge the folder outdated and delete it. That holds
   only when the save lands in a later tick than the autosave write. The WSL2 kernel runs at
   HZ=100 (10 ms ticks), and ext4 stamps file times from the coarse clock, so a fast host often
   finishes recover and save inside the autosave's tick: both files carry the very same time and
   the folder stays offered. A probe on the failing runs printed identical times to the
   nanosecond, for example `autosave=1793209628.773163449 save=1793209628.773163449`. Windows file
   times are fine enough that the save is always later there. The product does not rely on this
   path: the egui and studio hosts' `recover` write a fresh autosave and then call
   `autosave::discard` on the leftover, and `session.save` never touches autosave folders, so
   hypothesis 1 of #175 (a save that leaves a recovered leftover behind) does not hold for the
   product. Hypothesis 2 (a lock held) is ruled out because a held lock skips the folder, giving an
   empty result, and hypothesis 3 (two tests sharing a folder or name) because the failure
   reproduces with the test running alone: 165 of 300 runs of the single test failed on WSL at
   717043d5, and 3 of 15 runs of the whole engine lib.
3. **Why checks missed it.** The test's outcome depended on how long recover and save take
   relative to a 10 ms tick: earlier Linux gates happened to cross a tick boundary, and Windows
   always does. The sibling same-tick test pinned the equal-time case for the product but the
   older test was not revisited when `>` became `>=`.
4. **Regression test.** The test now dates the leftover's session file to the Unix epoch before
   saving, stating the order it asserts (the autosave is older than the save) instead of leaving
   it to the clock; the equal-time case stays covered by the sibling test. After the fix: 0 of 300
   single-test runs and 15 of 15 whole-lib runs passed on WSL. The product code is unchanged.
5. **Gate class.** A test whose assertion depends on the order of two file modification times sets
   those times itself (`File::set_modified`) instead of relying on the real clock between two
   writes: coarse ticks make "later" equal, and a deliberate `>=` then keeps the older item.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`.
   Owner: the coordinator. Blind spot: `cargo xtask timing` finds real-clock reads in test loops,
   not a test that compares file times written by the filesystem; that stays a review item.

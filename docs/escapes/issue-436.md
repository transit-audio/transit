# E81: four tests counted on a race's outcome or a real-clock instant under load (#423, #381, #436)

1. **Failure.** Four tests failed once each in workspace runs on a heavily loaded host, in crates the
   branches under test did not touch, and were not rerun:
   `player::midi_out::tests::a_wrap_ends_held_notes_at_the_wrap_instant_and_drops_discarded_playback`
   ("MIDI output thread did not reach: 4 sends", #423);
   `strip_jobs::tests::every_publish_ends_mixed_inline_or_overrun_under_free_running_workers`
   (`mixed > 0 && inline > 0`, #381);
   `stream_tests::the_buffer_switch_thread_joins_mmcss_at_its_first_switch_and_leaves_it_at_the_stop`
   (the MMCSS revert ran on a thread other than the driver's, #436); and, on Linux,
   `render_graph::continuity_across_upstream_missed_skips` (no stall made a strip skip, #436).
   No user saw anything: each hunt found the product correct under every interleaving.
2. **Mechanism.** Each pass depended on a scheduling outcome.
   (a) The wrap test (`crates/playback/src/player/midi_out/tests.rs`) stamped a wrap 300 ms ahead of
   the host clock and waited up to 10 s for four sends. A held note stamped "now" is scheduled at the
   instant the output thread drains it; when the thread drained nothing until after the wrap, the
   note fell after its lap's release and the scheduler dropped it (`Worker::drain`, the release's
   `retain`, `midi_out.rs`), which is right (sent after the release it would hang), so only two sends
   ever came and the guard fired. Reproduced by holding the thread with the existing `stall` seam:
   the guard fires, and after the release the port holds exactly the restart and the later note.
   (b) The strip-jobs test (`crates/rt/src/strip_jobs/tests.rs`) let free-running workers race the
   publisher's reclaim; the product's contract is only that exactly one wins. A scratch where the
   publisher waits for every claim reproduced "8464 mixed, 0 inline" with every identity holding.
   (c) The MMCSS test (`crates/audio-asio/src/stream_tests.rs`) needed a driver thread switching every
   1 ms to win the session's 20 ms release wait (`Session::release_mmcss`, `mmcss.rs` `release`);
   past it the host reverts, the documented fallback (known issue K33, with its own test). It also
   slept 20 ms for a state store after a counter.
   (d) The continuity test (`crates/mix/tests/render_graph.rs`) needed a worker to take a stall on a key
   source; stalls are taken only by worker renders (`tests/common/workers.rs`), and a callback that
   renders the sources inline takes none. With every strip inline (no workers, a seeded cost) the
   test fails its liveness count with every identity holding.
3. **Why checks missed it.** `cargo xtask timing` flags short wall-clock bounds (rule a), sleeps
   pacing a measurement (b), throughput floors (c), short loop exits (d) and pool-test deadlines (e).
   None of these shapes was present: the wrap test's guard is 10 s and its clock dependency is the
   stamp `now + 300 ms`; the strip-jobs coverage is `> 0` of counters a race decides, with no clock in
   the function; the MMCSS wait is inside product code and the 20 ms sleep precedes no measurement;
   the continuity test carried rule (e)'s `explicit-deadline` marker, which accepts a test's own
   deadline as deciding its outcomes (the open item E72 recorded: an explicit or far deadline is
   still a clock).
4. **Regression test.** Each test now decides every path through a seam, keeping what it proved:
   (a) the real scheduler without its thread on a test-only clock (`Worker::clock`), stepped through
   the wrap for three drain interleavings (after every push, once before the wrap, once after it);
   controls: removing the release's `retain` fails "the discarded note is not sent", and scheduling
   the release one nanosecond early fails "nothing ends before the wrap".
   (b) one block in eight forces each path (workers parked at an epoch handshake, the publisher
   waiting for every strip done, one strip held inside its worker until consumed) and asserts its
   outcome; controls: a reclaim that also takes a claimed slot fails "two threads inside one strip's
   processor", and a reclaim that never wins fails "block 1: forced inline".
   (c) the MMCSS hooks carry the release's clock (`Hooks::now`, `Instant::now` in production); the
   counting fakes stop it for the test, so the host waits for the driver thread's switch; the test
   waits on the registration's state, not a sleep; control: a host that reverts without waiting fails
   "and reverted on itself".
   (d) four blocks hold a key source on a worker under the deterministic deadline
   (`MixEngine::hold_until_alone`, a `Duration::MAX` cost history) and assert the source `Late` and
   its dependents `UpstreamMissed`; control: recording an upstream skip as `Unstarted` fails at block
   300.
5. **Gate class.** E49: no test asserts a scheduling or real-clock outcome. Its pass never waits for
   an instant it stamped against the real clock, never needs a thread to win a product's wait, and
   never counts a path only a race takes; each path is forced through a seam, and free races stay only
   for identities that hold under any schedule.
6. **Protocol change, owner, blind spot.** A review rule in `docs/contributing/evidence/gates.md`
   beside E76 and E77. Owner: the maintainer. Blind spot: no line scan tells a race's outcome from a
   count. A candidate scan rule (a short `thread::sleep` directly before an assertion needs a
   `sleep-ok` marker naming the negative check it precedes) has 19 hits on the tree today, most of
   them deliberate negative checks, so it is a follow-up, not part of this landing. The shared
   10 s `until` helper of the MIDI output tests and the ASIO rig's 10 s host timeout remain hang
   guards whose pass path does not depend on them, but a starved host can exceed them; listed in the
   follow-up.

# E73: a monitored port acknowledged its routes before it captured SysEx

1. **Failure.** The macOS job of CI run 41965585585 on main 7a746a3c (2026-11-05) failed
   `player::native::tests::monitor_counts_clock_instead_of_listing_it_and_notes_realtime_and_sysex`
   (`crates/playback`) at the native tests' deadlock guard: "native MIDI worker did not reach
   the required state", 10 s after the wait began. The push did not touch `crates/playback` or
   `crates/midi-io`, and Linux and Windows passed the same commit. A user with the MIDI Monitor
   open could miss a SysEx that a controller sent just after the monitor opened its port; nothing
   else of the port was lost.
2. **Mechanism.** When the monitor is on, every input port is open and the device's SysEx capture
   must be switched on in its callback (`InputConsumer::set_sysex_capture`; the callback reads the
   flag at each `F0`, `crates/midi-io/src/ring.rs`). The ingress worker
   (`crates/playback/src/player/ingress.rs`, `Pump::step`) did that only at the per-step switch,
   after its update loop. The same loop handles the port's route update and stores its
   acknowledgment (`applied`), which `Native::status` reports as the port's routes. A thread that
   waited for the routes could therefore deliver a SysEx after the acknowledgment and before the
   switch. The `F0` then read capture off and the SysEx was never captured. Two orderings open
   the window: the monitor already on when the port is attached, with the routes acknowledged in
   the same step (the CI test's case), and the monitor switched on after the attach step, with the
   routes acknowledged in a later step. In both, `applied` was stored before the switch of that
   step. A review of the first fix found the second ordering. The test waits for
   Stop, the SysEx and sensing together. Stop and sensing arrived, the SysEx never did, and the
   guard ended the wait. The window is the rest of one worker step, microseconds on a quiet host,
   so it is hit only when the worker is preempted right after the acknowledgment while the test
   thread runs. The CI log fits a stuck wait, not a slow one. The test failed at 04:30:32.42, so the
   failing wait began about 04:30:22.4, beside its alphabetical neighbor
   `monitor_shows_an_unrouted_port_and_leaves_routing_alone` (finished 04:30:22.59). In those 10 s
   more than 140 other tests finished, among them four native-worker tests that ran beside it
   (the last, `idle_worker_taps_step_pedal_and_learnable_actions_without_delaying_routes`, at 04:30:23.43). The
   playback binary's 282.88 s sits inside the previous macOS runs' 250 to 315 s. The worker cannot
   lose a wake: it parks with a 20 ms timeout (`ingress::IDLE`). The freshness readout cannot read a
   wrong clock: sensing is judged at the injected `read_at`, which equals the `FE`'s injected stamp
   (E59). Overflow is ruled out: fifteen deliveries sit far below the ring capacities. Reproduction
   on Windows: the test passed 20 of 20 runs alone and the full playback library 5 of 5 runs at 48
   test threads. With an uncommitted 50 ms sleep after the acknowledgment, it failed 5 of 26 runs
   with the CI signature (`tests.rs:16`, 10.04 s), at the wait for Stop, the SysEx and sensing.
3. **Why checks missed it.** The test waited on an acknowledgment that did not cover every effect
   it then relied on. "Routes applied" was read as "the port is ready for the monitor", but the
   worker published it before the capture switch. E59 made the test wait for every effect it
   asserts. That rule covers what a test waits for afterwards, not what the acknowledgment before
   its deliveries promises.
4. **Regression test and fix.** Two tests in `player::ingress::tests`, one per ordering, deliver a
   SysEx from a test hook (`Pump::after_routes_applied`, test builds only) the moment the worker
   stores the acknowledgment, then step the worker and expect the SysEx in the monitor:
   `sysex_sent_once_the_routes_are_acknowledged_reaches_the_monitor` (monitor on at the attach)
   and
   `sysex_sent_once_the_routes_are_acknowledged_reaches_a_monitor_switched_on_between_the_attach_and_the_routes`.
   Each fails deterministically without its part of the fix ("the SysEx was not captured: []"):
   the first on 7a746a3c, the second on the first fix alone. With the fix both pass. The fix
   switches capture on in two places, with the same wait-free call as the per-step switch. A port
   that arrives while the monitor is on switches capture on as it is attached. A route update
   switches it on before it stores `applied` if the monitor is on and the port does not capture
   yet. An acknowledgment therefore never precedes capture while the monitor is on. With the
   attach-time fix and the 50 ms
   widening, the native test passed 20 of 20 runs. The native tests' wait helpers are now
   `#[track_caller]`, so the guard names the line that waited.
5. **Gate class.** A worker's acknowledgment is stored only after every effect it stands for, and a
   test that acts on an acknowledgment has a test that acts in the instant after it is stored (a
   hook at the store), not only a thread that usually loses the race. It joins E49 (no test outcome
   decided by scheduling), E59 (wait for every effect asserted) and E72 (a hang guard ends in a
   failure, never in an outcome the test counts): here the guard did its job, and the defect was a
   lost message behind it.
6. **Protocol change, owner, blind spot.** The coordinator owns the rule in
   `docs/contributing/evidence/gates.md`; this landing adds it as a bullet after E59's. Blind spot:
   the native tests' guard is still a 10 s wall-clock budget from the start of each wait. Under E72
   it should become a no-progress ceiling. A host stall of 10 s would fail it with no defect behind
   it. That conversion is a follow-up. macOS was not run for this record; the mechanism was read
   from the code and the CI log and reproduced on Windows by widening the window.

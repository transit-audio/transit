# E59: Active Sensing was stamped when the worker looked, not when it arrived

1. **Failure.** `player::native::tests::monitor_counts_clock_instead_of_listing_it_and_notes_realtime_and_sysex`
   (`crates/playback`) failed once with `sensing: false` (Clock 125 BPM as expected) during a #168
   gate run on a branch that does not touch `crates/playback` or `crates/midi-io`, and passed on
   the previous run of the same tree (#170). A user could see the MIDI Monitor's sensing
   indicator lag one worker step behind other input; no wrong steady-state reading was found.
2. **Mechanism.** Active Sensing (`FE`) is filtered by the midi-io ring by default, so the ingress
   worker learns of it only through the ring's `filtered` counter, read once per step in
   `Mon::watch` (`crates/playback/src/player/ingress.rs`), before the step drains the ring. Watch
   published the sensing time as `consumer.now_ns()`, the worker's clock when it looked. The test
   delivers `FE`, a SysEx and Stop from its own thread while the worker runs. If a step's watch
   reads the counter just before the `FE` is pushed, the same step still pops the SysEx and drains
   Stop, so the test's readiness wait (Stop and SysEx seen) is met while sensing is published only
   in the next step: the readout says `false`. The suspected cause in #170, two host origins, did
   not hold on this path: the native player opens every input with its own origin
   (`InputConfig { host_origin: origin.instant(), .. }` in `crates/playback/src/player/native.rs`),
   so `consumer.now_ns()` was already on the monitor's origin, and a stamp read after the test's
   `base` is always within the 2 s `SENSING_TIMEOUT_NS` of `read_at`. The defect in the code was
   the stamp itself: a freshness time taken from the consumer's clock rather than the message's
   arrival, which also made the outcome depend on real time.
3. **Why checks missed it.** The window is one worker step between the counter read and the drain,
   with the test thread's three deliveries landing inside it; a quiet host rarely interleaves that
   way. The test injected the Clock stamps and the readout (E49 recurrence) but not the `FE`, and
   its wait covered two of the three effects it asserts.
4. **Regression test.** `filtered_active_sensing_is_judged_by_its_arrival_stamp` and
   `passed_active_sensing_is_judged_by_its_event_stamp` (`crates/playback/src/player/ingress/tests.rs`)
   and `a_filtered_active_sensing_records_its_own_host_stamp` (`crates/midi-io/src/tests.rs`) use
   injected stamps and readouts only: a filtered `FE` 1 s before the readout reads sensing, one 3 s
   before does not, and a passed-through `FE` keeps its event stamp. The clock-independent
   negatives are the midi-io stamp test and the monitor-off test's `mon.sensing == 5_000_000_000`
   assertion (`monitor_off_costs_the_worker_and_the_callback_nothing`): both pin the published
   stamp to the injected arrival exactly, which the old code (the worker's clock at notice time)
   never produces. The arrival-stamp test also fails on the old code, but only while the real
   clock is not near its injected readouts, so it is not the proof. The fix: the ring
   stores the last filtered `FE`'s host stamp at push (a relaxed `AtomicU64` beside the counters,
   allocation-free) and watch publishes that stamp. The native test delivers the `FE` with
   `deliver_at` on the player's origin and waits for sensing as well.
5. **Gate class.** Tests must not read a real clock: a freshness or rate is judged from the
   message's own arrival stamp, never from a consumer's clock read when it notices, and a test
   injects both the stamp and the readout. A test that waits on another thread waits for every
   effect it asserts, since one step can publish them unevenly. A related hazard, not this
   escape's mechanism: two host origins in one process are two clocks, so stamps and readouts
   must share one origin (`docs/contributing/evidence/gates.md`).
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spot: the timing check (`cargo xtask timing`) finds real-clock reads in test loops, not a
   product path that stamps with the consumer's clock; that stays a review item.

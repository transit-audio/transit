# E49: the echo latency test asserted a 10 ms wall-clock bound (#87)

1. **Failure.** CI run 40118759045 (macOS, main e7a7b04) failed
   `live_input_on_an_armed_external_track_is_echoed_without_an_instrument` with
   `echo took 10.962083ms` against `latency < 10 ms`. The test passed on 0e94b85, and the changes
   merged since (#82, #84) do not touch the echo path.
2. **Mechanism.** The test measured an `Instant` difference between the test thread delivering
   input and the output thread's send, across the ingress worker and the output thread's 1 ms
   poll. #62 tightened the bound from 50 ms to 10 ms when the worker started waking on data. On a
   shared runner, two thread hand-offs exceed 10 ms whenever the host is busy. The product was
   not wrong; the test was.
3. **Why checks missed it.** It passed on every local machine and on earlier CI runs; the jitter
   is load-dependent. A tight wall-clock bound looks like a stronger test, and the review of #62
   took the comment "generous for a loaded CI machine" at its word.
4. **Regression test.** The same test now asserts a 250 ms stall bound with its rationale in
   the comment. The wake-on-data property itself stays with
   `an_idle_ingress_worker_wakes_on_the_input_callback_not_on_a_timer`
   (`crates/playback/src/player/native/tests.rs`), which uses the worker's own
   callback-to-pop timestamps and a median.
5. **Gate class.** No wall-clock latency bound under about 100 ms in a unit test: use a
   deterministic signal, or a stall bound with a stated rationale (`docs/contributing/evidence/gates.md`).
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spots: other tests still hold tight wall-clock bounds (`midi_out/tests.rs` echo 30 ms and
   lap deadline 10 ms, `native/tests.rs` max wake 14 ms, `runtime/tests.rs` block time 10 ms);
   they are tracked in #87 for the same treatment.

Recurrence 2026-10-24: `an_idle_ingress_worker_wakes_on_the_input_callback_not_on_a_timer` failed
its 14 ms max-wake assertion (max 18.39 ms, median 35.9 µs) in a full-workspace run beside other
builds. Fixed (#87): that max is now a 250 ms stall bound (the median bound stays and fails a
timer-poll regression); the echo bound in `midi_out/tests.rs` is 200 ms (below the 250 ms offset
it must ignore); the next-lap bound is 100 ms (a lap held to the old deadline is ~200 ms late).
`runtime/tests.rs` worst block is an `#[ignore]` hardware-timing test and is left as is.

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

Recurrence 2026-10-24 (macOS CI run 40137883187): `a_creation_that_never_returns_is_stuck_after_the_timeout_and_never_blocks_a_frame`
(`crates/plugin-host/tests/host_thread.rs`) asserted at least 10 frames in the 300 ms creation
timeout window and saw 9. That is a throughput floor, not the property. Fixed (#87): the test now
requires more than one frame (a blocked loop sees a single poll) and a 250 ms bound on the longest
frame, plus the Stuck status; the 1 s slow-creation test has the same floor relaxed to `n >= 2`
beside its existing worst-frame bound. Other `Duration::from_millis` values in the plug-in host
tests are sleeps, timeouts or deadlines, not measurements.

Sweep 2026-10-24 (#87, complete): every remaining throughput floor and sub-100 ms measurement
ceiling in a non-ignored test is converted; the rule is that none remain.
- `crates/playback/src/player/midi_out/tests.rs`: per-send lateness `< 50 ms` is a 250 ms stall bound, plus `dropped` and `unsent` counters at 0 (the `late` counter trips at 2 ms, so it is not asserted); the clamp bound is already a stall bound.
- `crates/playback/src/player/runtime/tests.rs`: `>= 20` blocks during a 500 ms creation is `>= 2` blocks plus a 250 ms bound on the longest gap between blocks; the frame, save-deadline and `FRAME_BUDGET` bounds are stall bounds and are marked.
- `crates/ui-studio/src/plugin_host_tests.rs`: `>= 10` frames during a 1 s load is `>= 2`; the existing 400 ms worst-frame bound is the stall bound.
- `crates/vst3-host/tests/slow_create.rs`: `>= 20` blocks during creation is `>= 2` plus a 250 ms longest-gap bound (the ignored hardware-timing companion keeps its strict numbers).
- `crates/audio-asio/src/stream_tests.rs`: stop `< 100 ms` is a 250 ms stall bound beside the deterministic `driver_stopped` check; `host.rs` (700 ms against a 200 ms host timeout) is a stall bound and is marked.
- `crates/engine/src/cmd/clap.rs`: the info wait plus 300 ms is a stall bound and is marked.
- `crates/playback/src/player/native/tests.rs`: the 2 ms / 5 ms median wake bounds stay by design (a median of 25 against the 15.6 / 20 ms timer tick); the margin is now documented.

Recurrence 2026-10-24 (#87, #91): CI run 40149862372 (macOS) failed
`monitor_counts_clock_instead_of_listing_it_and_notes_realtime_and_sysex` with a BPM of 26.0: the
test paced eleven Clock pulses with `thread::sleep(20ms)` and asserted a wide range, and the
runner stretched the sleeps. Fixed: the fake input gained `deliver_at`, which stamps the host
arrival time, the test stamps the pulses 20 ms apart and asserts 125.0 within 0.5. Rule added to
`gates.md`: no rate, BPM or latency is derived from sleep pacing. The remaining sleeps in the
native tests wait for the worker to idle or poll for a state; none feeds a derived rate.

Recurrence 2026-10-25 (Realtime run 40160491467, macos-x86_64): `audio_thread_render_does_not_wait_for_a_saturated_global_pool`
(`crates/mix/tests/render_pool.rs`) failed `worst < BOUND` with "a 512-frame block of 16 strips took
93.446819ms with the global pool saturated" against 50 ms. The product was right: an audio-thread
render reaches only the dedicated render pool (`run_strips`, `crates/mix/src/lib.rs`), and the
global pool's jobs sleep, so the time was CPU contention on the runner, not queueing (locally
175 µs idle, 15-47 ms with the test and eight spinning processes pinned to four cores). Fixed: the
test asserts the ordering instead. Every global worker holds a job that ends only on release, and
no job may have ended when the eight renders return; a render that needed a global worker would
finish only after one did. Mutation: dispatching audio-thread strips to the global pool fails it
deterministically (24 jobs ended, 10 s). The sweep missed it because the 50 ms bound sat in a
`const`; `cargo xtask timing` rule (a) now follows `const`/`static`/`let` names bound to a short
duration, which also surfaced one unmarked by-design median in `native/tests.rs` (now marked).

Recurrence 2026-10-25 (#87, #91; CI run 40205285227, windows asio and macos): the rewritten Clock
test panicked "a rate from eleven pulses" (`crates/playback/src/player/native/tests.rs`). The
pulses were stamped with `transit_midi_io::host_now_ns()`, whose origin is fixed at midi-io's first
use in the process, while the readout (`Native::take_monitor`, `native.rs`) judges the one-second
Clock timeout against `HostOrigin::default()` (audio-backend), fixed at a different first use. The
two origins differ by however long the test process ran between them, so on a slow runner the stamps
looked more than 1 s old and the rate was None. Two hidden dependencies behind the injected
stamps: a second host origin, and the real clock at read time. Fixed: the test stamps on the
player's own origin, and `take_monitor_at(now)` lets it read at the last stamp (production's
`take_monitor` passes the real clock, unchanged). Mutation: reading 2 s after the stamps fails it
deterministically. Rule added to `gates.md`; no `cargo xtask timing` rule, since a line scan cannot
tell two origins apart.

Recurrence 2026-10-25 (#76, macOS CI on 8da26d2a): the async-import expiry test stopped polling its kept item before checking the abandoned item, so a scheduler pause could exceed its 100 ms TTL. The test now injects the polling clock, verifies retention at 99 ms and expiry at 100 ms while the polled item stays done, and uses real time only for a marked worker-completion stall bound. Disabling TTL expiry or poll timestamp refresh fails the corresponding state assertion (not a timeout); 20/20 runs passed with the test and two spinning processes pinned to one CPU. Production defaults to Instant::now under the same lock; constants and dependencies are unchanged.

Recurrence follow-up 2026-10-25 (#76 siblings): the remaining async-import tests that slept against a TTL, grace or away window now drive the injected clock by hand: `a_caller_away_longer_than_the_limit_loses_nothing_when_it_returns` (was a 5.3 s sleep against a 100 ms TTL; now checks 99 ms and 100 ms after the return), `a_caller_that_was_away_finds_all_its_items_when_it_comes_back` (was a 500 ms sleep against a 50 ms grace; the clock is advanced once and then frozen) and `cancel_leaves_no_media_and_no_document_change` (was a 200 ms sleep; waits for the worker count to reach zero behind a marked stall bound). `wait` now fails at once if the polled item expires. Mutations (away reset removed, never-polled items counted idle, cancel keeping the job) fail at assertions.

Amendment 2026-10-25 (#110): under a full-workspace load `idle_worker_taps_step_pedal_and_learnable_actions_without_delaying_routes`
saw a median of 11.5 ms against its 2 ms bound, and `a_wrap_ends_held_notes_at_the_wrap_instant_and_drops_discarded_playback`
failed on a rerun (its 30 ms wrap lay within reach of a stalled test thread, so the "discarded" note could fall due
before the release was queued, and its 40 ms sleep stood in for "everything due has been processed"). Both passed
`cargo xtask timing` because the `// timing: median` tag admitted any median. Fixed: the wrap test puts the wrap 300 ms
ahead and replaces the sleep with a later sentinel note whose send proves everything due earlier was sent or dropped;
the two wake-latency medians (this one and `an_idle_ingress_worker_wakes_on_the_input_callback_not_on_a_timer`) moved
to `#[ignore]` measurement tests, and the gated tests keep their delivery and ordering assertions plus a 500 ms
stall bound. A bound alone does not guard the #62 class, because the worker's 20 ms idle park timeout pops events
without any wake; the two gated tests therefore start the worker with a 30 s idle timeout
(`Worker::start_with_idle`, `Native::new_with_idle`; production keeps 20 ms), so only a wake on data meets the
500 ms bound. Mutation: `park_timeout` -> `sleep` in the worker fails both.
non-ignored test is flagged like any other.

Recurrence 2026-10-25 (Windows gate under load, #97): `a_reader_never_pairs_a_value_with_another_publishes_count`
(`crates/playback/src/player/midi_map.rs`) failed `assert!(seen > 0)`: the writer published 400k times and
finished before the reader took one pair, so the test depended on which thread ran first. Fixed: the writer
runs until the reader has checked a fixed 20,000 pairs, each against `value == publish count`, so the observation
count no longer depends on scheduling. Mutation: `take` reading the value and count from two separate atomics
fails the pairing assertion (3 of 3 runs). 30 of 30 runs pass with a `cargo check` running beside them.

Recurrence 2026-10-25 (#106): `a_cancelled_or_silent_asio_calibration_stores_nothing_and_says_why`
and `measure_on_the_asio_calibration_line_times_the_loop_and_stores_it_for_this_buffer_size` failed
under load ("the audio device did not answer: no measurement"; nothing stored). The tests pump 800
driver blocks with a thread per block, and the window's measurement gave up on the real 8 s wait
(`Pending::began.elapsed()` in `hardware_measure.rs`), which a starved run exceeds. Fixed: the wait
reads `measure_age`, which tests freeze through `measure_elapsed`; the pump loops are count-bounded.
A new test advances the injected age past the wait and asserts the give-up text and that nothing is
stored. Mutation: not storing the calibration in `ui.audio` fails the measure test. No
`cargo xtask timing` rule: the clock is read in production code, outside what a scan of test
lines can see.

Recurrence 2026-10-25 (#106): the playback clamp test now checks its exact queued deadline against an echo stamped in the same scheduler drain and retains port delivery; the two bounce tests keep fake callbacks running until render completion and stop adoption instead of exhausting a fixed block budget, with the existing 10 s stall guards and sample/grid/click assertions retained.

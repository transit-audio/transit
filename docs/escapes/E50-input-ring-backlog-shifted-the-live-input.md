# E50: the live-input ring kept a per-session backlog in front of the reader (#88)

1. **Failure.** The External Instrument Measure hardware investigation (2026-10-24) found that C
   moved frame for frame with the live-input ring's backlog B. On a Scarlett at 64, C was 1083,
   1303, 1341 and 1540 frames for B = 0, 192, 256 and 448. A C stored in one session placed
   notes 5.4 to 5.9 ms early in a session with a different B. The same backlog added 4 to 9 ms
   to input monitoring and shifted hardware insert round trips. It had been known for weeks as
   K22, rated cosmetic and described as "a measurement in the first moments can read about one
   block long".
2. **Mechanism.** `InputRing` is a plain FIFO. `open_input` cleared it, but the input callback
   kept pushing, and the plan began popping only when it adopted the ring a few blocks later.
   From then on push and pop ran in lockstep. The frames queued before the first pop became a
   permanent delay for that attachment, and the attach timing decided how many there were.
3. **Why checks missed it.** Every test of a ring consumer started from an empty ring. The
   fake-device insert tests and the Scarlett hardware test called `clear_monitor` after the
   adoption ("start from an empty ring (K22)"). That workaround hid the defect from the gates
   that would have shown it. The ring's own tests asserted FIFO preservation, which is right
   during a session but says nothing about the backlog at attach. The 326-frame figure in ADR
   0020 came from the cleared test.
4. **Regression test.** Fake-backend pre-fill tests:
   - `input_monitoring_latency_excludes_*` and `a_hardware_insert_round_trip_excludes_*`
     (`crates/playback/src/player/device/tests.rs`);
   - `an_adopted_input_reads_the_newest_capture_*` (`plan_bridge/tests.rs`);
   - the ring tests `the_aligning_read_drops_*`, `a_clear_mid_session_*` and
     `alignment_waits_*` (`record.rs`).

   With the discard removed they fail. The insert test reads 390 frames for a 326-frame loop
   with no pre-fill (the adoption lag alone), and more with a pre-fill. The Scarlett hardware
   test no longer clears the ring and asserts B = 0.
5. **Gate class.** When a consumer reads a queue that is filled before the consumer attaches,
   test it with the queue pre-filled before the attach. Do not test it after a clear that the
   product never does. A test workaround that names a known issue is a defect report, not a
   fixture.
6. **Protocol change, owner, blind spot.** The fix aligns the ring at attach and after every
   clear to a named target: `ALIGNED_BACKLOG_BLOCKS_PAIRED` = 0 and
   `ALIGNED_BACKLOG_BLOCKS_UNPAIRED` = one read block. The contract is in
   `docs/contributing/knowledge/audio-thread.md`. Owner: the coordinator. Blind spots:
   - The unpaired target is reasoned, not measured on WASAPI shared or cpal hardware.
   - A stall after alignment still leaves a backlog until the next clear.
   - The External Instrument Measure tests on their branch must add the pre-fill case.

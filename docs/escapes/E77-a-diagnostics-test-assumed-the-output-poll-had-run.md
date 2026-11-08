# E77: a diagnostics test assumed the output thread's poll had run (#410)

1. **Failure.** `midi_learn_live_tests::the_diagnostics_report_lists_the_live_midi_ports_mappings_and_last_source`
   (`crates/ui-studio/src/midi_learn_live_tests.rs`) failed once on 2026-11-08 in a workspace
   test run on an unrelated branch: the diagnostics report's MIDI section lacked its
   `Output: Model 12 · not open` line. The test passes alone. Nothing landed on it.
2. **Mechanism.** The report reads the input ports from the MIDI worker's status and the output
   ports from the `transit-midi-out` thread's status, which that thread publishes from its own
   periodic port poll (`PORT_POLL`, 250 ms, `crates/playback/src/player/midi_out.rs`). The test
   waited (stall-bounded) for both inputs to open, the learn, the mapping and the last note, and
   then asserted the whole report. None of those waits involves the output thread, so whether its
   first poll had published the output port was a scheduling outcome. On a loaded host the
   thread had not run its first poll when the report was taken. A test defect, not a product
   defect: the report correctly lists what the output thread has seen.
3. **Why checks missed it.** `cargo xtask timing` sees short bounds and paced loops; this test's
   waits are stall-bounded and correct for what they name. The missing piece is a wait that is
   not there, which a line or syntax scan cannot see. Alone, the inputs' waits take longer than
   one output poll, so the gap only showed when the output thread was starved.
4. **Regression test.** Before the report, the test now waits (through its `until` helper, the
   10 s stall bound, `// timing: stall-bound` marker) until `midi_status().output.ports` lists
   `Model 12`, the exact state the report reads, and only then asserts. Class check: the other
   three live tests in the file assert only states they wait for, or absences that a delay can
   only make pass (the port staying open, the fader held back by pickup); the
   `midi_output_header_tests` run without a Player, so no output thread exists there; no other
   `ui-studio` test runs a live Player with MIDI. Control: with the output port added by the
   scripted fake backend just before the report (so the next output poll is up to 250 ms away)
   and the new wait removed, the test fails at the report assertion on 5 of 5 runs; with the wait
   in place it passes 5 of 5. The fixed test passed 20 runs in a row during a parallel
   release build of `transit-mix`.
5. **Gate class.** E49: a test asserts a scheduling outcome. A test that asserts state produced
   by several threads waits, stall-bounded, for each producer's state in the form the assertion
   reads; a wait for one thread never stands in for another.
6. **Protocol change, owner, blind spot.** The rule is in
   `docs/contributing/evidence/gates.md`. Owner: the maintainer. Blind spot: a review rule only;
   the timing scan cannot see a missing wait.

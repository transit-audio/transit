# Live input ran late at a converted device rate and drifted later after a loop wrap (#724)

1. **Failure.** Two defects reached released builds.
   - **Converted device rate.** When the device rate differed from the session rate, live input
     (a monitored input, hardware insert returns, External Instrument returns) reached the plan
     about one device callback late, and it dropped out for a while at the start. Measured through
     the fake device, 48 kHz on 44.1 kHz at a 128-frame buffer was 173 plan frames late (3.6 ms
     with the buffer-size term), where the decision records had stated about 0.75 ms.
   - **Any rate.** After a loop wrap or a count-in end that fell inside a callback, the live input
     moved later and underran for the rest of playback. At 48 kHz on 48 kHz with 64-frame buffers
     it went from on time to 23 frames late, then 46, then 51, with 51 frames of silence on the
     way. With a larger buffer it drifted up to about one buffer.
   The same cause also made a count-in at a converted rate 15 frames short, and live MIDI at a
   converted rate late by a block.
2. **Mechanism.** At a converted rate the player rendered a whole quantum of plan frames as soon as
   the converter's taps neared the end of the pending block, and most of that block was presented
   in the next callback. The live input ring aligned its oldest frame to the block's first frame
   (`InputRing::pop_into`), which baked in about one callback. A loop wrap or a count-in end inside
   a callback cut the render blocks against the callback cuts for the rest of playback, at any
   rate, and the next block read capture frames that had already been read.
3. **Why checks missed it.** No test ran live input through the fake device at a converted rate:
   at the base commit `crates/playback/src/player/device/tests/` had no converted-rate file, and
   the loopback, insert and monitoring tests ran at equal rates. The converter's own tests
   (#525) compared output against a closed form but never the input side. No test wrapped a loop
   inside a callback with a live input playing, so the drift at equal rates was never observed.
4. **Regression tests.** In `crates/playback/src/player/device/tests/converted_rate.rs`:
   `hardware_probe_extra_delay_stays_within_the_tested_ranges` (eight rate pairs, buffers from 32
   to 1024 frames, changing callback sizes; the probe's extra reading stays under a millisecond),
   `a_long_warm_up_at_a_slow_drift_settles_to_the_same_figure` and
   `a_loop_wrap_inside_a_callback_leaves_the_live_input_where_it_was` (24 impulses through a loop
   at equal rates, each as early as the no-loop reference, no silence; fails on the base code).
   In `crates/playback/src/player/audio/tests/count_in.rs`:
   `the_latch_presents_the_start_sample_at_a_converted_rate_too` (the full bar, 88,200 and 192,000
   output frames; the base code gave 88,185) and `a_tone_sounding_across_the_latch_has_no_step_at_any_rate`.
   In `crates/playback/src/player/audio/midi/tests.rs`:
   `a_pickup_on_a_count_in_click_lands_on_that_click_at_a_converted_rate_too` (the base code gave
   4,395 against 4,410) and `live_notes_at_a_converted_rate_are_late_by_at_most_the_converters_reach`.
   In `crates/playback/src/player/audio/tests.rs`:
   `consumed_playhead_is_independent_of_render_ahead_and_callback_partition` gained a loop case.
5. **Gate class.** A path that reads live input is tested through the fake device at converted
   rate pairs and several callback sizes, and across a loop wrap and a count-in end that fall
   inside a callback, against a no-loop or equal-rate reference. A claim about a delay in a
   decision record names the pairs and buffers it was measured at.
6. **Protocol change, owner, blind spot.** Each callback now renders only what it plays, up to the
   converter's reach at the callback's last frame (`Phase::reach`). The aligning read keeps the
   whole capture block. The count-in latch and the timeline sit on the output frame that presents
   the start sample (`Phase::ahead`, `Timeline::lead_in`); the converter is not restarted there.
   The ADR 0015, 0017, 0020 and 0052 amendments and the v0.7.3 notes state the measured figures.
   The playback gate owns the tests. Blind spots: the probe's extra reading is not an exact
   monitoring delay (the probe bypasses output conversion), and it is not fixed per rate pair; it
   depends on the converter's phase, the history it has retained and where callbacks end. A
   deterministic term for paired devices is tracked in #709 and needs a record amendment first.
   Starting Play or Record from a stop at a converted rate still cuts audio that is sounding (#725).

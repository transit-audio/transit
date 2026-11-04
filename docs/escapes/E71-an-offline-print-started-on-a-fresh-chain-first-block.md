# E71: an offline print started on a fresh chain's first block (#332)

**Status: draft.** The mono fold-back fix has landed. The warm-up fix waits on the maintainer's
decision about ADR 0038 (see item 6).

1. **Failure.** While testing #318 on dev build 7cc65268, the maintainer bounced a mono guitar
   track in place. The track had a six-take stack with a comped take active and one hosted VST3
   amp plug-in as its only insert. The printed clip started near silence and rose over roughly
   the first bar (2 s at 120 bpm). Playing the same track live from the session start had no
   fade.
2. **Mechanism.** There are two findings, both in `crates/mix/src/offline.rs`.
   - **Warm-up in the print.** Every offline print goes through `RangeRender`:
     - Bounce in Place and Commit (`render_track_pre_fader` from `crates/engine/src/io.rs`)
     - Freeze (`render_track_pre_fader_timed` from `crates/engine/src/cmd/freeze.rs`)
     - File > Bounce and Bounce Paths (`RangeRender::try_new` and `try_new_paths` from
       `crates/engine/src/io/bounce.rs`)

     `RangeRender::build` compiles the session with new plug-in instances, adopts the plan and
     renders the range's first frame on the first block those instances ever process. A live
     chain is different: the player renders every block while the transport is stopped
     (`crates/playback/src/player/audio.rs`, `set_processing(!playing, ...)`), so a chain has
     been running for as long as it has existed when the user presses play. Some things a
     plug-in does on its first blocks settle before play live but land in the print offline:
     - an activation fade
     - smoothing toward the parameter values the host sends after its stored state (a hosted
       plug-in gets them as changes at frame 0 of the print)
     - a lane's value applied over a stored value

     The built-in reproduction is a Gain insert stored at -inf with a 0 dB lane. Against a 0.5
     source, the print read 0.00052, 0.197, 0.316, 0.459 and 0.497 at frames 0, 480, 960, 2400
     and 4800, and reached the source only at frame 9931 (207 ms). The Gain's 20 ms smoother
     (`crates/dsp/src/plugins/utility.rs`) was moving from the stored value to the lane's value.
     A stand-in hosted plug-in that fades in over 0.5 s after activation printed its whole fade.
   - **Mono fold-back.** `render_track_pre_fader_timed` rendered a mono track centered and
     folded it back with `sqrt(2)`. The -3 dB pan law's center gain is 10^(-3/20), not
     1/sqrt(2), so every mono Bounce in Place, Commit and Freeze print was +0.0103 dB
     (0.50059325 for 0.5).

   Plain stereo prints, and built-in inserts without lanes, had no ramp. A compressor's attack
   from silence is the same live and offline. The take stack was printed from its active
   playlist, with no clip fade applied twice.
3. **Why checks missed it.** Offline tests compared one offline render with another (Freeze with
   Bounce in Place, one block size with another), or checked levels after the start. Some tests
   used a freshly adopted engine playing from frame 0 as the "live" reference, which is not what
   the player does. None compared a print's first frames with the source or with live play from
   a stop. Every built-in snaps its stored parameters at creation (`initialize_params`,
   `crates/mix/src/instance.rs`), so a ramp needed a lane or a hosted plug-in, and tests never
   load third-party plug-ins.
4. **Regression test.**
   - **Mono fold-back (landed).** `a_plain_print_is_the_source_bit_for_bit_from_frame_zero`
     (`crates/mix/src/offline_prime_tests.rs`, stereo and mono, 0.5 then a step to -0.25)
     checks the print bit for bit. So does
     `a_plain_bounce_of_a_take_stack_prints_the_active_take_bit_for_bit_from_frame_zero`
     (`crates/engine/tests/bounce_in_place.rs`, a mono two-take stack through `track.bounce`).
     Both fail on 7cc65268 with 0.50059325.
   - **Warm-up.** `an_automated_gain_insert_prints_its_lane_value_from_frame_zero` and
     `a_hosted_plug_in_warm_up_settles_before_the_first_printed_frame` fail on 7cc65268 with
     the values above. They are ignored until the decision.
   - **Dynamics.** `a_dynamics_insert_prints_what_the_live_path_plays_from_frame_zero` holds
     the compressor's print equal to the live main mix.
5. **Gate class.** An offline render must equal live play from a stop, from its first frame, and
   any fold or gain the print adds must be exact. Tests of a print compare its first frames with
   the source or with that live reference bit for bit, never only a level after the start. The
   gate rule is added to `docs/contributing/evidence/gates.md` when the warm-up fix lands.
6. **Protocol change, owner, blind spot.**
   - **Pending decision.** The candidate warm-up fix runs a freshly adopted offline chain for a
     few seconds with the transport stopped at the range start and discards the output. That
     makes the print match live play from a stop. It contradicts ADR 0038's #280 rule that
     time-zero automation smooths from the stored value (#287 item 2 asks whether to start at
     the automation value, as Ableton and Logic do). It also contradicts the fresh-engine live
     reference of four existing tests:
     - `crates/mix/tests/prepared_params.rs`
     - two tests in `crates/mix/tests/offline_latency.rs`
     - `crates/engine/tests/time_stretcher_insert.rs`
     - `crates/ui-studio/src/midi_learn_hosted_tests.rs`, which reads the first printed block
       for a hosted parameter delivery

     The fix waits on an ADR 0038 amendment. The mix crate's reviewers own the gate.
   - **Blind spots.**
     - A plug-in whose warm-up runs longer than any run-in can still reach the print. So can a
       plug-in that finishes loading on its own thread by wall-clock time, because an offline
       render runs faster than real time.
     - Hosted plug-ins always run in realtime process mode offline
       (`crates/vst3-host/src/ffi/instance.rs`). Offline process mode is a candidate
       follow-up.
     - AudioSuite-style processing (`apply_plugin` in `crates/dsp/src/offline.rs`) also starts
       on a freshly prepared instance, and is not covered.
     - The maintainer's plug-in is not available to tests, so a fix for it is confirmed only by
       a retest on a dev build.

<!-- transit-index
title: Clock domains and timestamp quality: the clock contract annex to ADR 0012 (gates G4 and G6)
-->
# ADR 0017: clock domains and timestamp quality

- **Status:** proposed 2026-10-18; component-level G4 passed, hardware timing open; amended 2026-11-15, accepted after three judging rounds: the band-limited rate converters (Amendment 2026-11-15, below); amendment 2026-11-16, proposed: corrections of fact after #724 (Amendment 2026-11-16, below)
- Date: 2026-10-18. Status: proposed for joint review.
- Parent: [ADR 0012](0012-midi-device-layer.md), gates G4 and G6.
- Implementation status (2026-10-19 09:58 EDT): the original `f9078c7` jitter
  failure was corrected by retaining bounded observation spans. Full CI and independent
  re-review passed; clock/loop policy integrated through `06a936b`. **G4 fake-device
  passed at component level.** Player/take placement migration and hardware timing
  remain open. A live silent probe found WASAPI capture anchor failures, output
  non-convergence on a second open, and Focusrite ASIO backend timestamps that move
  backwards while host arrivals increase. Both ASIO opens ended without maps.
  Numerical
  backend estimates retain unknown offset uncertainty; these results do not certify
  hardware accuracy or demonstrate audio sample loss.
- Joint review: the pinned backend sources were reviewed on 2026-10-18;
  the clarifications below incorporate that review, with error bounds and early
  clock safety tests retained as described under G4.
- Applies to current CPAL 0.16.0 and the engine audited at e6acafa/d469e8f.

## What the device API actually provides

CPAL's timestamp origin is unspecified and its source depends on the backend.
Its type alone does not prove that two streams, a MIDI backend, and Rust Instant
share an epoch or resolution. [CPAL 0.16 StreamInstant](https://docs.rs/cpal/0.16.0/cpal/struct.StreamInstant.html).

The pinned implementation imposes further limits:

- WASAPI `host/wasapi/stream.rs:529–582` obtains a QPC time from IAudioClock,
  discards its device-position result, and estimates output presentation by adding
  `frames_available / sample_rate`. This is not a measured DAC position/latency.
  In a shared render buffer, queued padding is the relevant first-frame wait;
  adding the available space can create a systematic offset. Preserve padding
  when the backend exposes it, and test/calibrate rather than assuming this value.
- WASAPI capture uses the first frame's QPC timestamp from GetBuffer. The Windows
  API also reports discontinuity and timestamp-error flags. A backend adapter must
  retain those quality signals instead of treating every numeric timestamp as valid.
  [IAudioCaptureClient::GetBuffer](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer).
- ASIO `host/asio/stream.rs:637–651, 898–948` converts the callback's system time,
  then estimates playback/capture time by adding/subtracting one driver buffer.
  These helpers do not use the driver's reported input/output latencies or expose
  a device sample-position anchor to playback. A nominal buffer is not a complete
  interface latency measurement.
  The CPAL adapter also does not check system-time validity. A hardware-free probe
  of the original 0.16 StreamInstant arithmetic found that subtracting 128 frames at
  48 kHz from time zero returns a malformed future timestamp, about 4.2923 seconds
  after the callback, rather than panicking. This refutes the specific zero-time
  panic hypothesis, but confirms the need to validate timestamps before fitting.
  A real driver returning invalid zero time has not been demonstrated.
  The product's current fork pin `31b5dd4` repairs that signed arithmetic, verified
  by public API regression tests. It does not repair missing backend validity flags
  or establish an ADC/DAC offset.

The backend descriptions above come from source inspection. The live metadata
probe recorded separately now supplies device counterexamples to clock availability,
not a physical latency measurement. G4 supplies the observation seam and synthetic
checks; hardware timing still requires valid anchors and independent calibration.

## Distinct coordinates

| Coordinate | Owner and meaning | Discontinuity |
|---|---|---|
| Host time H | Monotonic process time, integer nanoseconds relative to an explicit runtime origin | New runtime origin |
| Backend time B | Timestamp from an identified audio/MIDI backend clock domain | Backend reset, wrap, invalid observation |
| Output frames D | Monotonic frame ordinal for the output stream; actual sample rate is estimated separately | Device generation changes |
| Input frames I | Separate capture-stream ordinal, even with the same nominal rate | Capture generation changes |
| Render frames R | Monotonic engine work ordinal, including processing ahead of presentation | Engine/format restart; same-rate plan edits preserve R and carry a separate plan generation |
| Session samples S | Position in the musical session, at the session sample rate | Play, stop, seek, loop wrap, rate change |

Equal nominal rates do not prove equal clock rates. Split devices require an
estimated input/output rate ratio and a controlled asynchronous resampler; a
periodic discard of queued audio is not drift correction. SRC filter delay and
route/plugin delay remain named components of the timing model.

**The rate converters** (amended 2026-11-15, #601). The device-rate converter (session to output
device) and the live input converter (input device to session) are the band-limited reader of
[ADR 0056](0056-audio-clip-editor.md) decision 8. They convert at the ratio of the nominal rates
and correct no drift between clocks; the asynchronous resampler above is still open. Each keeps an
exact rational phase, an integer count of units over a fixed denominator, and never adds the rate
once per frame; the reader receives that phase with its fraction rounded down to a multiple of
2^-32 frame. At unit rate (equal rates, speed 1) and a whole-frame phase, both pass every finite
sample through bit for bit; a fractional phase acquired earlier (at another speed) is kept when
the rate returns to 1 and is read through the reader. The live input replaces a non-finite sample
with zero, as it did before. The reader is linear phase, so the converters move no position on any timeline: the output
frame at which a render frame is presented is unchanged. Their filter delay is look-ahead instead.
Each callback renders only up to the converter's reach at its last frame: the render frames that
frame's taps read, which lie less than 16·max(1, q) render frames past its position, q being the
session rate times the speed over the device rate. The plan is never rendered a whole callback
ahead, at a loop wrap or a count-in end inside a callback included (amended 2026-11-16, #724; it
rendered a whole callback from the middle of a callback before). The live input converter reads
up to 16·max(1, q) input frames past its position, q being the input rate over the session rate.
At its aligning read the ring discards queued input down to the read's input frames plus
`halo(q)` = ceil(16·max(1, q)) + 1 (0 at equal rates) plus any slack. That is a target, met only
when that much input is queued. With less queued, the first read stops at the first frame whose
taps are not all queued, reads silence for the rest, and that stop sets the input's place; with
more, the first read need not stop and the input sits further back (amended 2026-11-16, #724).
Both enter any round trip measured on the plan's timeline, by an amount that depends on the converters' relative phase, the captured input history kept at alignment and the callback endpoints met afterward; the device buffer influences those but does not determine the result alone
(the Amendment 2026-11-16 below gives what the tests pin);
[ADR 0015](0015-io-routing-abstraction.md) section 5 and
[ADR 0020](0020-external-instrument-device.md) decision 4 say how a stored figure accounts for
them (#709, not built), so this delay is counted once.

The drift open item for a second device has one implemented rule so far, for System Audio
loopback capture (#268, `crates/playback/src/system_audio.rs` and `system_audio_rate.rs`): the
rate ratio is estimated from frame counts against QPC time, with each side timed as follows: the
loopback packets by the QPC of their first frame (WASAPI's `GetBuffer` position), the input
callbacks by the stamp's host callback time (the QPC read when the block arrived) on every
backend, never by `backend_frame_ns`, which on native ASIO is the driver's sample position at the
nominal rate and would make the device side exactly nominal. An invalid stamp falls back to the
reader's own arrival clock (QPC on Windows). Both sides carry a clock-domain tag; a pair on
different domains holds the estimate. The packets, timed by exact positions, are fitted by least
squares over the last 10 s, and a packet whose time steps more than 50 ms past its frames' time
(a loopback that paused and resumed with no discontinuity flag) restarts that line on itself: the
offset is permanent. The callbacks are timed by arrival, which is the true time plus a delay that
is never negative, so the device clock is the lower envelope of the arrival times and that side
is fitted to it: over every point of a 2048-point ring up to 60 s (20 s at callbacks of 10 ms or
less, 41 s at 960 frames), the lower convex hull of the points, the hull edge with the most points
within 1 ms above it, and least squares over those points only. A late callback, whatever its
delay, lies more than 1 ms above that line and has no weight, so no late callback or spell of
them that eases off within a few seconds restarts the line or moves the reading (an easing over
8 to 20 s reads 100 to 379 ppm off at 128 frames and up to 74 ppm at 8192, the class of a plain fit);
3 ms of arrival jitter reads within 20 ppm in the tests. Only a lasting offset restarts the device line, on the callback that
shows it: callbacks above the band for 3 s and at least 32 of them (a device that lost frames),
or one callback more than 0.25 ms below the envelope (plus how far the envelope's slope can miss
it), which no delay can put there (an offset that ended). After any restart the estimate is held
until the refilled fit's slope has a standard error within 4 ppm, taking its points as spread
over the 1 ms band. A lasting offset of 1 ms or less is neither and moves the reading for one
window (K100). The ratio is fed forward as the trim of the band-limited reader of ADR 0056
decision 8 (amended 2026-11-15, #603; it was a linear interpolator) and reported, as the clamped
estimate, as the drift; a slow proportional-integral term on the ring fill (the integral held
within 50 ppm) only brings the fill back to its target of one input callback, one packet and
30 ms. The reader's position for the n-th output frame after an anchor is x0 + n·q; at every input
callback the anchor moves to the position the old anchor and rate give the next output frame, and
only then does the new trim apply, so a trim change bends the position's slope and never moves the
position. The fill is counted from the reader's position. The reader reads up to 16·max(1, q)
frames past it, inside the target, so the target and the reported latency are unchanged and the
margin before the ring runs dry is that much smaller. A priming or a resync keeps up to `halo(q)`
frames (at the largest trimmed rate) before the new position as the reader's history, and starts
at a whole frame. At equal nominal rates the converter passes every sample through bit for bit
while its position stays a whole frame: from a priming or a resync on, for as long as the trim
stays exactly 0 (a fraction gained under a nonzero trim is kept when the trim returns to 0).
Drifts up to 1000 ppm are tested; the applied trim is limited to 2000 ppm. It
does not yet cover a second hardware input; that still needs the controlled asynchronous
resampler above.

## Observation and publication contract

A device observation carries direction, device generation, backend clock-domain
id, first-frame ordinal, frame count, nominal rate, callback timestamp,
capture/presentation timestamp when available, and quality/uncertainty. Quality
is one of measured hardware position, backend estimate, callback-arrival fallback,
or invalid. Unknown latency is represented as unknown, never zero.

At callback entry the runtime also observes its host clock. A pair of timestamps
is a calibration observation with uncertainty, not an equality: callback dispatch
delay, driver timestamp resolution and sample-position semantics matter. Use
multiple observations to estimate offset/rate and reject discontinuities. A
backend that cannot provide a sufficiently precise relation must expose degraded
accuracy; adding a buffer cannot recover timestamp information already lost.

The audio thread owns current output-frame advancement and adopts precomputed
calibration/transport changes at explicit boundaries. A non-realtime runtime owner
collects bounded observation records and prepares revised clock estimates. No
callback waits for the worker. Overflow, invalid observations, excessive residuals,
and stale estimates are counted. Published estimates have a generation and a
validity horizon. Reader retries must be bounded; keep the last valid snapshot or
return unavailable instead of spinning on a writer.

Publish residual jitter of the fitted relation separately from absolute offset
uncertainty. A callback-entry fit can improve relative monitoring placement without
establishing an accurate ADC/DAC offset. State precision per backend and direction;
the worst component resolution is a lower limit on achievable precision, not a
valid upper bound on combined error. End-to-end bounds must also include bias,
calibration error, timestamp granularity and delivery delay. Audio settings should
show quality, both uncertainty measures, and whether loopback calibration exists.

## Mapping into the session

Session mapping is a bounded history of segments, not one global offset. A segment
identifies a device/transport generation, a half-open output-frame interval,
the corresponding session sample/fraction, session/device rate ratio, play state,
loop iteration and the render plan's measured delay convention. Split segments at
seek, loop wrap, start/stop and speed changes, including several boundaries inside
one callback. Integer/rational accumulation retains fractional phase across blocks.

The compiler/runtime must agree exactly once where each delay is accounted for:
device output latency, SRC delay, main/route compensation, and input calibration.
An impulse test is the authority for this convention. Do not subtract plugin
latency both in the render plan and again during take placement. Each recorded
take stores its placement calibration and generation so a later device change
cannot silently reinterpret it.

Record arm and Play become one runtime transaction with a chosen timeline boundary;
the UI's call order is not the recording clock. Capture blocks carry their first
frame's time and sequence, allowing data before the chosen boundary to be trimmed
and later frames to be placed independently of writer scheduling. Stop closes an
admission generation and waits off the audio thread for admitted capture work to
finish publishing, then drains through that final sequence. Device loss must not
require another callback to acknowledge an already idle producer.

## MIDI admission and late events

Ingress preserves backend arrival time, clock-domain id, port/device generation,
channel, note identity and stable sequence. This is the time the backend observed
the event, not a claim about the physical keypress. Recording uses this mapped
time; monitoring uses the same time plus an explicit lookahead budget.

Express lookahead in frames or time, not "one callback" when callback sizes vary.
It bounds scheduling jitter only while arrival delay and clock uncertainty fit
the declared budget. Each event has an explicit result:

- Within a future/current processing interval: scheduled at its exact offset.
- Too late: render at the earliest legal offset, increment lateness counters and
  retain the original arrival time for recording; releases remain protected.
- Stale generation, invalid mapping or full queue: apply a documented recovery
  policy and count it; do not reinterpret an old event under a new epoch.

Seek, restart, port disconnect and stale note generations trigger a bounded
release protocol agreed with F10. Equal-time event ordering is stable and must
preserve sustain/note-release semantics. Fixed latency does not imply zero jitter
for arbitrarily delayed OS delivery.

WinMM's [MIM_DATA arrival timestamp](https://learn.microsoft.com/en-us/windows/win32/multimedia/mim-data)
uses milliseconds from midiInStart. Preserve
that granularity and the MIDI port's origin instead of advertising sample-accurate
keypress capture merely because the output uses sample offsets. Verify the chosen
MIDI backend's actual timestamp path before selecting a precision target.

## Runtime ownership proposal (G6)

Introduce a non-UI `AudioRuntime` in playback (or a dedicated L4 runtime crate if
dependencies require it), with both front ends calling the same lifecycle methods.
It owns Player, Recorder, audio configuration, clock generations, recording
transactions, and eventual MIDI port runtime. Session edits still go through the
engine. Owning runtime methods that manipulate editors and plugin controllers run
on the required main thread; putting them in a non-UI type does not grant worker
thread affinity.

| Context | Owns / may do | Must not do |
|---|---|---|
| Runtime/main-thread facade | Anchored processing holder, plugin controllers/editors, prepared plans, commands and typed acknowledgements | Device/writer joins; depend on UI repaint for event timing |
| Native lifecycle owner | Device transitions, stream close/join, recording finalization; borrow the anchored holder | Own/finally drop plugins; wait for main while holding its locks |
| Output callback | Render state, frame advancement, bounded event admission, telemetry | Device open/close, controller calls, allocation, waiting |
| Input callback | Capture sequence and bounded frame publication | File I/O, waiting for output/writer |
| Recording worker | WAV writes, final drain, peak summaries | Infer placement from its wake time |
| Clock worker | Calibration estimates and quality checks | Mutate active render state |
| MIDI ingress/scheduler | Timestamped bytes and bounded scheduled output | Own musical transport independently |

The proposed native/main split, pumped quit, state authority and WebAudio adapter are
specified in the runtime-owner contract (ADR 0012, gate G6). Main
continues to anchor processing owners until explicit quiescence; lifecycle requests
wake the facade independently of periodic repaint. This is a proposed G6 decision,
not an implemented migration or G12 hardware result.

## G4 test evidence required

The fake device must drive the same observation, calibration and event-admission
code used by the real adapter. A separate ideal-clock calculator is not sufficient.
Keep the adapter/fake in playback or its successor runtime crate; audio-io remains
file codecs. Expose a `fake-device` feature when cross-crate tests need the seam.
Tests inject known ground truth without sleeps or hardware:

1. Distinct input/output callback partitions, rates, backend epochs and arrival
   delays; an impulse and MIDI note map to the expected sample within the declared
   precision, independent of callback partitioning.
2. Loop wrap inside a block, several wraps in one block, seek, stop, fractional
   rate conversion, and speed changes; no old segment is used after its generation.
3. Restart A→B→A with stale observations/events in flight; invalidate old mappings,
   release held notes, and prohibit stale capture from entering a new take.
4. Delayed and reordered observations, timestamp errors, clock drift, queue full,
   missing callbacks, zero frames, and arithmetic extremes; bounded recovery and
   correct counters, with no callback allocation or unbounded retry.
5. Late events under a fixed lookahead budget, boundary events exactly at block
   end, and release events during overload. Recorded arrival time stays unchanged.

An ASIO/WASAPI backend test must additionally demonstrate which observations are
measured versus estimated. Hardware loopback then validates residual placement at
64/128/256 frames and 44.1/48/96 kHz. Neither fake-device tests nor a backend estimate
alone establishes hardware sample accuracy.

Before MIDI depends on the map: cases 1–3 and the clock-only safety cases from 4
(invalid/stale observations, zero frames, arithmetic limits, bounded publication)
must pass. MIDI event overload/release cases in 4–5 land with G5. A fake event can
exercise timestamp placement before the full note/port identity model exists;
it must still use the production mapping path.

## Amendment 2026-11-15: the band-limited rate converters (#601, #603)

Accepted 2026-11-15 after three judging rounds (proposed the same day). It describes the change for #601 and #603 and lands with it. The device-rate
converter, the live input converter and the System Audio converter now read through the
band-limited reader of ADR 0056 decision 8 instead of two-point linear interpolation. It replaces
one clause of the System Audio drift paragraph ("fed forward as the trim of a linear
interpolator") and adds the paragraph on the rate converters after "Distinct coordinates"; every
other statement of this record stands, the drift estimate, its servo, its target and its trim
limit included.

- **Why.** The System Audio paragraph named a linear interpolator that no longer exists, and the
  record said nothing about what a converter's filter does to timing. The reader is linear phase,
  so a converter adds no delay on any timeline; it adds render-ahead on the output and backlog on
  the input, which a round trip measured on the plan's timeline contains (ADR 0015 section 5,
  ADR 0020 decisions 2 and 4).
- **Evidence** (tests of the change for #601 and #603). Equal rates at a whole-frame position pass
  through bit for bit:
  `equal_rates_pass_every_bit_through_whatever_the_partition`,
  `equal_rates_through_the_player_are_bit_exact_pass_through`,
  `monitor_passes_through_at_equal_rates` and
  `equal_nominal_rates_with_zero_drift_pass_every_bit_through`. No position moves:
  `an_impulse_peaks_at_the_output_frame_the_timeline_maps_it_to`. The input's look-ahead is the
  backlog: `a_click_through_the_input_converter_peaks_at_its_input_position_and_the_backlog_reports_the_look_ahead`
  (on a paired device, with reads of a whole number of input frames, B equals `halo(q)` at 48 into
  44.1, 44.1 into 48 and 96 into 48 kHz, and 0 at 48 into 48). A trim change never moves the position:
  `a_trim_change_bends_the_position_and_never_moves_it`. A resync reads the stream behind the new
  position: `a_resync_starts_with_the_streams_frames_behind_the_position`. Output independent of the
  block and callback partition: `the_converter_output_is_independent_of_the_render_partition_at_every_rate_pair`,
  `the_player_output_is_independent_of_the_callback_partition`.
- **Judging rounds.** Three, together with the 2026-11-15 amendments of ADR 0015, 0020 and 0056.
  Round 1 asked for four changes, all made: bit-for-bit pass-through only at a whole-frame phase (a
  fraction gained at another speed, or under a nonzero System Audio trim, is kept and filtered);
  only finite samples (the live input zeroes a non-finite one, as before); the exact rational phase
  reaching the reader with its fraction rounded down to 2^-32 frame; and B equal to `halo(q)` only
  for reads of a whole number of input frames. Round 2 found nothing further here; round 3 settled.

## Amendment 2026-11-16: corrections of fact after #724 (proposed)

Proposed 2026-11-16, for judging together with the 2026-11-16 amendments of ADR 0015, 0020 and
0052. It corrects the paragraph on the rate converters to the code that landed for #724
(1af45806..12e06b36) and changes no decision of this record. Two sentences said the output renders
up to 16·max(1, q) render frames "further ahead" and the input's alignment keeps `halo(q)` "more"
input frames. The first was not what the player did: it rendered a whole callback ahead whenever a
block started in the middle of a callback (always at a converted rate, and after a loop wrap or a
count-in end inside a callback at any rate), so the live input reached the plan about one callback
late. The second stated a target of the alignment as if it were what places the input. Both are
replaced in place (the paragraph after "Distinct coordinates").

- **What the code does.** Each block is sized to the rest of the callback:
  `let quantum = self.read.reach(callback_frames - at, step, self.plans.engine().max_block())?;`
  (`crates/playback/src/player/audio.rs`), where `Phase::reach` returns the end of the last output
  frame's span, `band_limited::span(x, q).end` (`crates/playback/src/player/audio/phase.rs`). At
  equal rates and a whole-frame position that is the callback's last frame. The input is placed by
  two lines of `crates/playback/src/record/monitor.rs`: the aligning read keeps
  `read.saturating_add(lookahead).saturating_add(slack).max(last_push.saturating_add(lookahead))`
  input frames, and the reader stops at the first frame where
  `band_limited::span(x, q).end > queued as i64`. A plan that attaches an input calls only
  `input.align_on_next_read()` (`crates/playback/src/plan_bridge.rs`), so an input can attach with
  more than the target queued.
- **What decides the added round trip.** The converters' relative phase, the captured input history kept at alignment and the callback endpoints met afterward; the device buffer influences those but does not determine the result alone. It is not a
  function of the two rates.
- **What the tests pin** (`crates/playback/src/player/device/tests/converted_rate.rs`; each run
  starts the input empty, after four callbacks without capture; eleven callback patterns: 32, 64,
  128, 192, 256, 512, 600 and 1,024 frames and three patterns of changing size). The round trip
  over the equal-rate one, as a range the test allows at every pattern (session on device): 48 kHz
  on 44.1 kHz, 34 to 35 plan frames; 44.1 on 48, 32 to 33; 48 on 96, 31 to 32; 96 on 48, 61 to 62;
  48 on 88.2, 32 to 33; 44.1 on 88.2, 31 to 32; 48 on 176.4, 32 to 33; 48 on 192, 31 to 32; equal
  rates, exactly 0. The test does not assert which value of a range occurs; one run is pinned
  exactly: 35 at 48 on 44.1 with 192-frame callbacks after a warm-up of 4,000 callbacks, with no
  later silence. The input reads silence while it starts, at most 18 session frames at 48 on 44.1,
  at most 16 at the other converted pairs and at most 31 at 96 on 48, then at most one frame; no
  input is discarded, every probe click is found and the clicks agree with no spread. The test
  names: `the_converted_rate_term_does_not_depend_on_the_device_buffer`,
  `a_long_warm_up_at_a_slow_drift_settles_to_the_same_figure`.
- **What the figures are.** They are readings of the hardware probe, which adds its click after the
  output converter: `fill_inner` calls `probe.add_click(frame, at)` on the converted frame, with
  `at` from `self.read.index()`, the floor of the converter's position, and `HwProbe::add_click`
  indexes the unconverted click with it. The click is held on the device frames whose position
  falls on its plan frame instead of being converted. Averaged over a whole orbit of phases from a
  whole-frame start, that hold lags by 73/147 of a plan frame at 48 on 44.1, 48 on 88.2 and 48 on
  176.4, 159/320 at 44.1 on 48, 1/4 at 48 on 96 and 44.1 on 88.2, 3/8 at 48 on 192 and 0 at 96 on
  48; a single click lags by its own phase, and `analyze` takes the best whole-frame lag. So the
  figures are not the delay a send through the converter, a monitored input or an External
  Instrument's return sees, and no test isolates the hold's share.
- **Render-ahead bound.** At 48 kHz on 44.1 kHz the plan is rendered at most 20 frames ahead of
  what the playhead shows, with and without loop wraps inside callbacks
  (`consumed_playhead_is_independent_of_render_ahead_and_callback_partition`); a loop wrap renders
  up to the callback's end and no further
  (`seek_past_loop_end_continues_linearly_and_a_seek_back_engages_the_loop`). With a 48 kHz session,
  a live note due at once lands 17 plan frames late on a 44.1 kHz device and 16 on a 96 kHz device,
  at callbacks of 64 to 512 frames, and on time with the live input's 128-frame lookahead
  (`live_notes_at_a_converted_rate_are_late_by_at_most_the_converters_reach`). At the session's
  rate the live input stays where it was through loop wraps, with no silence
  (`a_loop_wrap_inside_a_callback_leaves_the_live_input_where_it_was`).
- **Stated limits of the current code, by reading, no test yet.** (1) At 48 on 192 with fixed
  64-frame callbacks every callback ends at output phase 3/4, where the last tap leads by 15.25
  render frames, not the 15.75 the phase lattice allows; the input then settles at an added round
  trip of 31.0 plan frames, so the higher value of a range need not occur. (2) At 48 on 96 with
  64-frame callbacks and enough input queued when the input attaches, the aligning read keeps
  64 + 33 = 97 input frames, the first read never stops, and the next block, which starts 16 plan
  frames after the callback's first output frame, reads input from 33 device frames before it: an
  added round trip of 16 + 33/2 = 32.5 plan frames, above the range the tests pin. The #709 slice
  must add tests for both.
- **Not pinned.** The lower end of the start silence (the release notes gave 15 frames) has no
  test.

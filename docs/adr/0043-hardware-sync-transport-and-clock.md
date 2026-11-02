# ADR 0043: Hardware sync: transit follows and sends MIDI transport and clock

- **Status:** proposed 2026-11-03
- **Date:** 2026-11-03
- **Scope:** maintainer decision, issues [#93](https://github.com/derekmwright/transit/issues/93) and [#183](https://github.com/derekmwright/transit/issues/183); transport follow, clock follow and clock/transport send.
- **Reopen when:** the reference rig cannot meet the measured jitter budget; the filter cannot follow the admitted tempo ramps without audible corrections; recorded material cannot retain its musical placement under the runtime map; or a second synchronization protocol needs to arbitrate the source.

## Context

Hardware sequencers must be able to start transit, set its tempo, and follow its transport.
The compatibility rig is an Arturia KeyStep Pro over USB, which can send and receive clock and
transport. A loop through its own ports exercises both directions. This is synchronization of
musical time, not a replacement for the audio interface's sample clock.

Today `Session::tempo` is a `transit_time::TempoMap` (960 model ticks per quarter), stored by
`crates/model/src/session.rs`. Its implementation is in `crates/time/src/tempo.rs`, not model.
`TransportStatus` reports playing, recording and session-sample position; `TempoGesture` in
`crates/engine/src/lib.rs` belongs to document editing and retiming. Neither is a clock follower.
`cmd/transport.rs` queues host requests. Playback applies them through the bounded action lane;
`player/audio.rs`, `player/audio/phase.rs` and `timeline_map.rs` distinguish rendered-ahead audio
from consumed device frames. Locate discards pending rendered audio and changes the transport
epoch; a loop wraps session position without rewinding the device clock.

The input parser in `crates/midi-io/src/ring.rs` separates interleaved real-time bytes from
channel messages and stamps them with `host_arrival_ns`. On this base it **offers** F8/FA/FB/FC
and F2 to the input ring; they are ignored for musical playback, not dropped by the parser.
The [Monitor](../contributing/knowledge/midi-monitor.md) already observes them. Its display-only
`ClockRate` and one-second expiry are not a follower. [MIDI Learn](../contributing/knowledge/midi-learn.md)
matches notes/CCs; sync must work with no learned mapping, note route or Monitor window.

[MIDI output](../contributing/knowledge/midi-output.md) schedules host deadlines derived from
device presentation frames. This is sample-resolution scheduling, not proof of sample-accurate
USB delivery. [ADR 0020](0020-external-instrument-device.md) distinguishes the plan presentation
instant from Main's delay compensation A; [ADR 0027](0027-external-instrument-real-time-render.md)
owns hardware rendering. Neither contract can be replaced with UI time or input FIFO depth.
K22's monitor alignment is not a clock fit; K14, K31, K44, K46, K49 and K50 remain relevant
limits in [known issues](../known-issues/README.md). This base has no row explicitly naming
#185 or a zero-length MIDI note defect; retain the actual device-placement and short-note
regressions rather than inventing a closed status for either.

The convention references need a qualification. [Ableton's MIDI sync guide](https://help.ableton.com/hc/en-us/articles/209071149-Synchronizing-Live-via-MIDI)
describes external tempo and transport, a source indicator and sync delay. Its separate
Link Start Stop Sync switch is not the MIDI input switch. Apple's current
[synchronization overview](https://support.apple.com/guide/logicpro/synchronization-overview-lgcpfffd9371/mac)
explicitly says Logic sends MIDI Clock but cannot receive it. Logic's Tempo Interpreter is
another mechanism. Thus the requested disabled tempo track and incoming BPM display are transit's
external-clock policy, using Ableton's follower model; we do not claim both products implement
MIDI-clock following. Logic supplies the output-ordering and freewheel precedents below.

## Decision

### 1. Clock domains and one authority

Use named coordinates and retain clock keys, epochs, quality and uncertainty at each conversion.
`H` is integer monotonic host nanoseconds from the Player's shared host origin, the same origin
used by MIDI recording. `D` is the output device's monotonically consumed sample ordinal at
device rate `Fd`; capture has its own ordinal and map. `S` is session samples at session rate
`Fs`, with seeks and cycle wraps. `q` is quarter notes, obtained from the active tempo map.
Render frames `R` can lead `D`; callback number is none of these coordinates.

Incoming MIDI clock `C_in` is a count of F8 messages, 24 pulses per quarter, each stamped at
arrival on the MIDI input thread as `H_n`, before worker or UI delay. Convert arrival H through
the same `ClockMap::host_to_frame` and historical placement machinery as recording. Do not
reinterpret a WinMM timestamp as H or as D. Arrival stamps retain `Arrival` quality and USB/driver
uncertainty even with a good audio map. Outgoing `C_out` is a series of quarter-note crossings,
mapped to D and then through `ClockMap::frame_to_host` to output-thread deadlines.

| Mode | Transport authority | Tempo and musical phase authority | Audio scheduling and recording |
|---|---|---|---|
| Internal (all inputs Off) | Local transport | Saved session tempo map: S to q | Device D; existing rate conversion and historical D to S mapping |
| External transport only (Transport) | Chosen input's Start/Stop/Continue/SPP; local Stop always works | Saved session tempo map; F8 has no effect | Device D, as internal |
| External clock (Transport and Clock) | Chosen input's transport; local Stop always works | Incoming tick count anchors q; filtered tempo interpolates between ticks; runtime external map replaces saved tempo events | Device D remains the sample clock; historical D/S/q segments place events |

Only one input is the active source. Setting a second port to a non-Off mode turns the former
one Off in the same preferences transaction. There is no first-message-wins arbitration.
Source/mode/offset changes are accepted only while stopped and after a take has finalized.
Opening a session always starts stopped; record engagement is never persisted (ADR 0011).

### 2. Following transport

Audio/MIDI settings show **Off / Transport / Transport and Clock** per input port, default Off.
A selected port is opened even without a note route. The transport bar shows **Internal**,
**Transport: <port>**, or **External <bpm> · <port> · <state>**. The state is Waiting, Acquiring,
Locked, Freewheel or Missing; position uncertainty is an additional explicit warning.

| Message from the selected port | Operation |
|---|---|
| Start, FA | From stopped, locate to the enabled, nonempty cycle start, otherwise zero, then play. This cycle-start interpretation is an intentional transit convention; wire Start means song zero. |
| Continue, FB | From stopped, resume the retained position, including a preceding SPP; do not return to the cycle start. |
| Stop, FC | Cancel a pending start; stop and finalize recording through the normal stop path; retain position for Continue. Release playback notes and external destinations. |
| Song Position Pointer, F2 low high | `p = low + 128*high`, `q = p/4`, model tick `240*p`. Locate to that absolute sixteenth using the active map; do not wrap it into the cycle. Seeking at/past the cycle end follows today's play-through rule. |

Ignore Start or Continue when already playing (including recording or a start already pending),
Stop when stopped with no pending start, and SPP that is the current pending/applied target.
Do not implement these as toggles. An SPP does not itself start playback. Malformed or incomplete
F2 is counted and ignored; a real-time byte between its two data bytes remains independent.
An SPP received while playing is tolerated as an explicit locate, even though transit never
transmits SPP to a receiver that it has left running.

Transport-only mode applies a start at the next admitted action boundary without waiting for F8.
Clock-follow mode prepares on Start/Continue and begins at the device frame of the first
following F8 plus the input delay (tick zero at the chosen start, then tick one a period later).
If that frame is already behind rendered audio,
apply at the earliest unrendered boundary, report late frames, and slew toward the source; never
backdate captured data. Clocks while stopped may acquire tempo but do not advance song position.

**Recording:** external Start/Continue records only when at least one track is armed **and**
the user has engaged Record. Record in external mode prepares the ticket and displays
**Waiting for external start**; arming alone never records. This is the requested Logic/Ableton
armed-plus-record workflow, not a claim that MIDI Start carries a record bit. External starts
bypass the local count-in. A duplicate Start during a take is ignored, not a restart or a
discard. Stop finalizes the take and clears Record engagement; arm states remain. For a real
locate during a take, finalize the current pass first, then locate, and resume a fresh pass if
Record remains engaged. Preserve both passes in the Record operation; use ticket cancellation
and acknowledge finalization before starting the next one. If preparation fails, stay stopped
with the take preserved and the reason visible. No SPP may throw away a take.

Local Play/Record in clock-follow mode waits for the source; local Stop is immediate and cancels
queued starts using the source generation and offered-sequence cutoff. Fresh external messages
after that cutoff may start again. In transport-only mode local controls keep their usual meaning.
Half-speed and scrub are refused in clock-follow mode; leave external sync while stopped to use
them. MIDI Learn transport mappings do not consume or duplicate system sync messages.

A port may still carry notes and controllers normally. Real-time bytes are stripped out of the
channel stream by the existing parser and dispatched separately; they do not become notes, echo
traffic or recorded MIDI channel events. No track is implicitly armed, monitored or muted.

### 3. Clock estimator, lock and drift

Run the estimator on the MIDI ingress worker, never on the audio callback. All arithmetic below
uses injected arrival stamps and a supplied readout time. Admit 5..1000 quarter notes/minute,
the tempo map's range; reject nonfinite or nonpositive results instead of clamping bad input
into apparent lock. Keep 25 ordered stamps (24 intervals, one quarter-note window):

```text
N = 24; M = 4
d_i = H_i - H_(i-1)
m = median(d_i)
MAD = median(abs(d_i - m))
J = max(3*MAD, 0.10*m, 200000 ns)
I = { i in last 24 intervals : d_i > 0 and abs(d_i - m) <= J }
T_hat = sum(d_i for i in I) / |I|                 # ns per MIDI tick
B_hat = 60e9 / (24*T_hat)                        # quarter notes/minute
A_n = median(H_i + (n-i)*T_hat for i in I)        # filtered H of tick n
D_n = host_to_frame(A_n + input_delay_ms*1e6)
q_n = q_start + n/24                            # unwrapped quarter position
q_target(D) = q_n + (D-D_n) * B_hat/(60*Fd)
```

With fewer than 18 accepted intervals retain the last valid estimate and show Acquiring;
before the first full window, a median of available positive intervals is provisional only.
Outlier rejection affects the estimate, **not the tick count**: every received F8 increments
the running count exactly once, even when its interval is rejected. Never silently create ticks
to explain a long gap. A nonincreasing stamp, a known ring loss, or (after acquisition) an
interval outside `[0.5*T_hat, 1.5*T_hat]` makes position uncertain: an omitted/extra pulse cannot
be distinguished from transport or driver trouble. Drop phase lock and freewheel; reacquire
tempo from a fresh window, but require SPP or Stop then Start to reestablish absolute phase.
A sustained large tempo step can therefore require resync; gradual ramps within this band do not.
Start resets `n=0` and `q_start` to its chosen start; Continue resets them to the retained
unwrapped phase. SPP resets them to `q=p/4`, clears count uncertainty and starts acquisition
again. The first subsequent F8 anchors that position rather than advancing it by one tick.

Every 24 new intervals evaluate a complete beat. A consistent beat has at least 22 accepted
intervals, `MAD/m <= 0.05`, an estimate within 2% of the previous complete beat, valid device
mapping, no count uncertainty, and phase error at most 2 ms expressed as device samples.
Lock after **M = 4 consecutive consistent beats** (the first complete beat seeds the comparison
and counts as beat one if its other checks pass; the next three also satisfy the 2% check).
Any failed beat returns to Acquiring; counter/port/clock generation changes reset the streak.
Tempo can lock while stopped, but song phase is established only by the start/SPP rules.
The indicator reports musical lock and timestamp quality separately; Locked never means
calibrated sub-millisecond wire timing.

**Position follows count, not integration of the displayed BPM.** At each applied block boundary
let `q_play` be the continuous unwrapped rendered musical phase. The controller uses:

```text
v = B_hat/(60*Fd)                               # quarters per device sample
e = q_target(D_block) - q_play
W = 24*T_hat*Fd/1e9                             # one beat, device samples
v_block = v + clamp(e/W, -0.01*v, +0.01*v)
q_next = q_play + block_device_frames*v_block
phase_error_samples = e/v
```

The +/-1% term corrects drift over time; feed-forward `v` follows tempo ramps. Evaluate the
target at the device presentation frame of the audio being prepared, including converter
phase, not the UI cursor or the most recently consumed frame regardless of render lead.
An anchor update changes rate, never assigns `q_play = q_target`. Thus phase discontinuity
from drift correction is zero, tighter than **X = 1 ms** (`ceil(Fd/1000)` samples). Start from
stopped, an explicit locate/SPP and a normal cycle wrap are intentional discontinuities, not
drift correction. The underlying tick count stays unwrapped across cycles; only the session
view wraps, with a new loop iteration and the existing release/chase semantics. Large errors
remain visible and slew; lock is not declared merely because BPM looks stable.

**Clock loss:** when `H_now - H_last > 24*T_hat` (strictly more than one beat), unlock and show
**Freewheel: clock lost, last <bpm>**. Continue playing and recording at the last valid tempo
with the phase-correction term removed until Stop; never switch to the saved tempo mid-take.
Before any valid tempo, Waiting uses the saved tempo at the start position as a provisional
fallback, clearly labeled. An unplug marks Missing and uses the same freewheel policy.
Returning pulses recover tempo but cannot certify the missing count; require an explicit
position reset as above. This supersedes #93's early suggestion to fall back to session tempo.
The precedent is Logic's documented **Jam Sync** option, which keeps running at the last tempo
on loss of sync; applying that recording-preserving policy to MIDI Clock and choosing a
one-beat timeout are transit decisions, not claims about Logic's MIDI receiver or its default.
See [Logic's synchronization settings](https://help.apple.com/logicpro/mac/9.1.6/en/logicpro/usermanual/chapter_43_section_3.html).

### 4. Active tempo, rendering and recording

External mode disables saved tempo events and tempo editing from acquisition through freewheel;
Locked displays **External <bpm>**. Keep the saved tempo map intact. Use a runtime piecewise map
of `(D, S, q, rate, transport epoch, loop iteration)`; each adopted block extends the last
segment continuously with `v_block`. Device frames still advance audio at Fs/Fd; the rate
correction changes musical phase and the beat grid, not the hardware sample rate or the pitch
of every audio clip. S remains the elapsed session-sample coordinate between locates/wraps;
q is read through the runtime map, not by applying the saved map to S while external is active.
For a locate outside retained history, extrapolate from the current pass's latest tempo segment
to obtain S, then establish a new epoch at the requested q. Before any external segment exists,
use the saved map for this initial coordinate only. Cycle boundaries are musical addresses:
crossing the cycle's end q wraps to its start S/q and increments the iteration, even when the
actual elapsed sample length differs from the saved map's loop length. Do not also wrap at the
old sample end. Runtime history is keyed by epoch/iteration so two visits to one beat cannot
alias. Source changes while stopped preserve the current musical position under the new map.

This needs render plumbing, not repeated `tempo.set` commands or `TempoGesture` edits. At plan
preparation derive immutable musical addresses from the document's map for ticks-timebase clip
starts, MIDI content, loop boundaries and automation. Evaluate those addresses against the
runtime map while following; keep samples-timebase and time-locked material in S. As in
`engine::retime`, audio clip starts may follow beats but audio lengths and file offsets do not
stretch automatically. A held MIDI note, pedal release or automation point must not be replayed
when a new anchor is adopted. Meter events stay musical; split at meter/tempo boundaries with
ADR 0038's `render_span` rule and supply the same followed quarter/bar position and effective
tempo (`60*Fd*v_block`) to built-ins, hosted plug-ins, automation, LFOs, sequencers and metronome.
The UI's incoming BPM is B_hat; diagnostics may also show the small phase-corrected rate.

Record input **as today against D** with the record-start ticket and clock fit. Keep both the
original event stamp/device frame and the applied historical D/S/q segment; MIDI-to-beats
conversion uses the segment at the event, never today's estimate or worker wake time. A delayed
drain across a ramp, seek or cycle must yield the same result as immediate drain. MIDI takes
store the resulting musical ticks in today's clip representation; a ticks-timebase clip's
stored start is converted back through the saved map for persistence, and through the runtime
map for live playback. Samples-timebase placement remains sample based. Raw audio retains its
device-aligned samples and duration. Cut cycle takes at the actual applied wrap, not an assumed
constant-tempo loop duration. Preserve controller-only passes and enforce paired note-off order
and a minimum one-sample audible duration before quantizing to stored ticks. If stored note
endpoints round to the same model tick, retain at least one model tick; do not create a
zero-length note or reorder a controller across its neighboring note.

The recording journal must retain the applied mapping segments needed for recovery, in an
additive, versioned journal extension; old journals keep their old interpretation. Flush those
segments with event batches on control and retain journals until the autosave contains the
take. Exhausted history/storage stops the pass through normal finalization with a recoverable
take and an explicit fault, rather than guessing beats or dropping captured audio. This is a
slice-3 gate, not a later recording follow-up. No audio FIFO backlog is used to infer phase:
K22 concerns monitoring; the device placement contract concerns recording.

External tempo history is runtime state, not a newly saved session tempo track. Leaving external
mode while stopped restores the saved map: MIDI retains its recorded beats, raw audio retains
its duration, and playback at a different saved tempo need not reproduce the original live
performance. Show that limitation in help. Capturing a reusable external tempo map is later
work. Offline render uses the saved map and sends no sync; refuse it while external mode is
active with an explanation to leave external mode first. ADR 0027 real-time bounce also requires
internal mode in this scope, so an external Stop/SPP cannot invalidate its fixed-range capture.

### 5. Sending clock and transport

Each output port has **Clock** and **Transport** switches, both off by default, and **Song
Position on locate**, default on when Transport is enabled (inactive otherwise). Clock alone
serves devices whose LFOs need tempo without transport. Transport alone sends state and position
without F8. Send no ticks while stopped in this scope; free-running stopped clock from #183 is
deferred. Ports used only for sync are opened by the output worker independently of tracks.

Generate ticks from actual applied transport spans, not a sleeping BPM timer. In internal or
transport-only mode the crossings are `q_k = k/24`, model tick `40*k`, converted through the
session tempo map including changes mid-bar. Use exact fractional crossings and round once to
the first device frame at or after the crossing; never sum rounded tick intervals. Under
external clock, derive crossings from the applied runtime map, regenerating the followed phase
for other devices rather than forwarding input F8 arrival jitter.

The callback publishes bounded Copy spans with transport/port generations, converter phase,
map revision, A and the device presentation mapping. The output worker enumerates the 24-ppqn
crossings and queues deadlines on the existing output path. Default sync deadline is the audible
Main reference, `H_k = P(R(S_k)) + A/Fs * 1e9 + output_delay_ms*1e6` at unit playback speed;
use the span's rate conversion for other speeds. This is the bare-output convention in
ADR 0020, **not** an External Instrument's uncompensated note stamp. Per-port output delay can
compensate a particular hardware chain; it never borrows a track's offset or changes note stamps.
If a deadline has already passed, send once as late and count it; do not claim the requested
negative offset was achieved. Loss of a valid presentation map is labeled approximate and
disqualifies a timing benchmark, as it does for note output.

Wire ordering is explicit (F2 is a three-byte System Common message, not a real-time byte):

- Starting at song zero sends **Start, then first Clock**, with the first F8 representing q=0.
  Reserve a preparation lead of `max(one tick period, 5 ms)` before that tick. It is lead time,
  not an extra counted tick. Prepare playback far enough ahead on a local start; an external
  start that arrives too late is sent in order and reported late, not silently backdated.
- Resume without a locate sends **Continue, then Clock**, retaining the next unsent tick and
  fractional phase. Stop sends **Stop** and cancels all future ticks for that transport generation.
  A duplicate local request that changes no state emits nothing.
- A representable stopped locate sends **SPP** if enabled; nonzero initial play sends **SPP,
  Continue, Clock**. An unaligned stopped locate retains a pending target until the
  next-sixteenth rule below can apply at playback; it does not transmit a floored SPP.
  A playing locate or cycle jump sends **Stop, SPP, Continue, Clock** at the new position, with
  no F8 between Stop and Continue. Stop is due at the jump boundary, never early enough to
  truncate the old lap. The first new-position F8 follows the ordered preparation sequence;
  report any resulting resync gap rather than advancing a hardware song before the local
  jump. Old-generation ticks at/after the jump are invalidated.
  This is Logic's documented [Stop/SPP/Continue cycle-jump mode](https://support.apple.com/en-au/102005).
  SPP while the receiver is running is forbidden by this sender's MIDI compatibility contract.
- SPP represents only `0..16383` sixteenths. For an unaligned locate, leave transit's local
  position unchanged but keep that output stopped until the next representable sixteenth,
  then send its SPP/Continue and first tick; show **Waiting for sixteenth**. Do not floor SPP
  and pretend fractional position was transmitted. An out-of-range position leaves that output
  stopped with **Position outside MIDI SPP range** until a representable start. At a normal
  Continue with no locate, retained sub-sixteenth state needs no SPP.
- With Song Position disabled, Start restarts a device's pattern on an initial play or loop
  restart; a playing locate uses Stop/Start, no SPP. The UI labels this **Pattern restart;
  song position not followed**. Note playback routing remains independent.

The protocol basis is the MIDI Association's [message summary](https://midi.org/summary-of-midi-1-0-messages)
and [MIDI 1.0 Detailed Specification](https://midi.org/midi-1-0-core-specifications): 24 clocks per
quarter, six clocks per SPP unit, and Start followed by clocks. The chosen preparation lead and
sub-sixteenth waiting rule are transit policies. Equal-deadline sync events retain the ordering
above and precede new note-ons, while note releases retain their safety reserve. Sync traffic
must not consume that reserve. A late backlog never bursts an unbounded train of catch-up ticks:
if a tick is a whole tick period late, fault that port's sync, cancel queued ticks, issue Stop
when Transport is on, and require a fresh start/resync; report missed ticks.

**Feedback:** never enable sync sending to the same physical device as the active sync input
(`same_midi_device`, including aliases); keep the requested switches but show them suspended.
The existing note echo guard and storm detector are insufficient because Clock is excluded.
Other output ports may follow transit; there must be one source in the hardware wiring, with
MIDI thru disabled on return paths. Generation checks prevent software replay, not arbitrary
multi-device cabling loops. The diagnostic loopback below explicitly bypasses the same-device
send guard while disconnecting the returned input from transport control and the follower.

### 6. Preferences and bounded publication

Store per-input mode and `input_delay_ms`, per-output switches and `output_delay_ms`, in
machine preferences (`ui.json`), keyed by backend, stable port id when available, otherwise
name plus ordinal. Delay defaults to zero, finite range -500..500 ms. Positive input delay adds
to the tick's phase-anchor time (transit plays later); positive output delay sends later.
Neither changes raw note/capture stamps. All sync-setting changes apply while stopped. Missing ports keep
their preferences and show Missing; never bind a different device merely because its enumeration
index matches. Reconnection starts a new generation, acquires clock afresh and never starts
playback by itself. An output reconnect cancels old ticks and remains stopped until a fresh
transport start/resync; it never drains the old backlog. Uncertain aliases stay unbound until
selected explicitly.

All new preference fields have defaults; there are no new required session fields, format bump
or `needs` marker. Absent sync preferences default to Off; an older session needs no migration
and does not override existing machine sync preferences. Existing arm and monitor persistence
rules stand. Saving a session does not silently turn an arrival estimate
into a permanent tempo edit.

The OS MIDI input callback only stamps, parses and publishes bounded records. Add a separate
preallocated sync lane (2048 events for the chosen input) so clock floods cannot spend note-off
reserve. The ingress worker consumes each `(port generation, sequence)` once independently of
Monitor/Learn scans. It filters ticks and sends semantic requests to control; control prepares
recording/locates and submits the action lane. A Stop also sets a sticky cancellation generation
at ingress admission so a full queue cannot lose it. Overflow invalidates queued starts, marks
phase uncertain, requests a safe Stop/finalization and reports the loss; no stale Start may
overtake that Stop. A fresh start after acknowledgment is required. Musical clock timeout alone
uses freewheel, not this queue-corruption policy.

Publish a coherent Copy snapshot `(tempo, phase anchor, source generation, clock key/revision,
lock/quality, last arrival, valid-until)` from the worker for adoption once per audio block.
The tempo/anchor pair is atomic as a **whole publication**, not two independent relaxed
atomics: use a preallocated SPSC snapshot queue with release/acquire publication, at most eight
slots drained per block, retaining the newest applicable generation. A full queue coalesces
on the producer; the callback keeps its previous complete snapshot and checks expiry from
device frames if the worker stalls. No seqlock spin/retry loop, lock, allocation, destructor,
sort, estimator or OS send on the callback. Control retires owned storage. Any necessary new
unsafe stays within [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md)'s allowlist and review rules.

## Implementation

Land four slices, each useful and independently gated. Split near-budget Rust modules by
concern before implementation; this document authorizes no file-size exception.

| Slice | Work | Gate before merge |
|---|---|---|
| 1. Transport follow and send | Typed sync ingress, single-source preferences, minimal source indicator, Start/Stop/Continue/SPP state machine, recording tickets, output serialization and feedback guard. Clock follow choice remains unavailable until slice 3. | Parser interleaving and generation tests; state matrix including armed/unarmed and engaged/not-engaged recording; duplicate requests, SPP at bounds, full queues, unplug and local Stop; fake output asserts byte order and no audio-thread sends. |
| 2. Clock send | Applied transport spans to output worker, 24-ppqn deadlines, tempo-map changes, per-port Clock and delay. | Scripted device/map verifies crossings within one device sample, no accumulated rounding, tempo change mid-bar, rate conversion, cycle/locate invalidation, delayed worker, PDC and offset sign; strict callback allocation gate; reference hardware send diagnostic before claiming #183's jitter target. |
| 3. Clock follow | N=24 estimator, M=4 lock, bounded phase controller, runtime beat-map render integration and recording/recovery history. Enable Transport and Clock. | Synthetic jitter/ramp/drift and loss fixtures; exact state transitions and sample-error bounds; MIDI/audio take alignment and journal recovery across map changes, short notes and held notes at cycle seams; block-partition and no-allocation checks; source loss never discards a take. |
| 4. UI and diagnostics | Complete source/lock/quality presentation, preference help, counts, loopback page and benchmark export. | UI state/default/persistence tests with injected stamps; diagnostics arithmetic with scripted samples; real USB loopback report and reference rig signoff, including reconnect and loaded runs. |

Implementation touch points are `midi-io` parsing, new focused playback sync modules beside
`player/ingress.rs` and `player/midi_out.rs`, `action_lane`, `timeline_map`, `record/start`,
`midi_capture`, plan preparation/render transport, the engine's transport/take builders and
Audio/MIDI settings. Extend `TransportStatus` with runtime sync readout rather than storing
it in `Session`. Do not recompile a document on every tick. Keep #185's historical device
placement invariant and ADR 0038's block-split contract in the integration gate.

## Gate

### Deterministic evidence

Drive fake input, device callbacks, control drain and output dispatch explicitly. Supply every
arrival, readout, deadline and device-map observation; never sleep and assert that a worker woke
soon enough. A failing test is retained and investigated, not rerun to get green. The area gates
and [gates.md](../contributing/evidence/gates.md), [CONTRIBUTING.md](../../CONTRIBUTING.md), and
fresh-context [ADR 0009](0009-refutation-review-before-every-push.md) review apply to implementation.
This docs-only proposal runs no Cargo commands and claims no implemented or hardware gate pass.

- At 48 kHz, generate ideal 120 BPM stamps with a fixed zero-mean +/-250 microsecond arrival
  sequence (12 device samples), a single 2 ms outlier, then a 120-to-132 BPM linear ramp over
  60 synthetic seconds and +/-500 ppm source/device rate mismatch. After acquisition, assert
  `abs(B_hat-B_true) <= 0.2 BPM` and `abs(phase_error_samples) <= 96` on these fixtures; assert
  every discontinuity-free boundary preserves phase and the correction term stays within 1%.
  These are proposed acceptance bounds, not measured results. At constant exact tempo expect
  the exact 24-tick estimate and no accumulated drift over one synthetic hour.
- Lock transitions use the four complete consistent beats, including exactly-at/just-past
  one-beat expiry; supply zero/backward stamps, duplicate/dropped F8, a burst sharing a stamp,
  an out-of-range BPM, a tempo step, old generations and invalid/stale clock maps. Verify
  Freewheel retains the last tempo, keeps an active take, and requires phase resync after loss.
- Run full transport sequences with cycle on/off, FA/FB/FC repeated, SPP 0 and 16383, malformed
  F2, realtime bytes inside notes/F2/SysEx, notes and clock on the same port, no note routes,
  Monitor closed and Learn active. Check recording ticket cancellation, pending Start then
  Stop, locate while recording, controller-only takes and recovery; compare delayed versus
  immediate event drain. Negative controls must fail when arrival is replaced by drain time,
  when the saved map is used under external follow, or when raw ticks are integrated as BPM only.
- At session/device rates 44.1/48/96 kHz and block partitions 1, 17, 64, 128 and 512, use a map
  with a change at quarter 2.5 (mid-bar), plus meter changes, to verify `40*k` model-tick output
  crossings. Test locate between sixteenths, maximum SPP, nonzero cycle start, transport-only
  send, Clock-only send, disabled SPP, same-device suppression and loopback isolation. Inject
  A changes and +/-offsets; prove Start precedes first F8, no obsolete tick survives Stop,
  and a stalled sender cannot burst catch-up clocks or exhaust note-release reserve.
- Count allocations/frees and MIDI sends over the actual callback path, including adoption,
  loss, overflow, wrap and recovery. Assert coherent tempo/anchor snapshots under a scripted
  publication race, never a timing-based race. With four or more rendered strips force serial
  unless explicitly testing workers, per the existing gate rule.

### Hardware diagnostics, not a unit test

On the diagnostics page select the KeyStep Pro's USB output and returned USB input. Configure
one clock source: transit sends, the hardware receives/returns, and the diagnostic input is
measurement-only (follower, transport follow and note echo detached). If the device does not
return USB sync directly, use its own MIDI Out-to-In cable with USB routing configured and
record that exact route; verify returned FA/F8/FC counts before measuring. Test hardware-as-source
separately, with transit's send to that device suspended. Never use a closed clock-control loop
as evidence of follower stability.

At 48 kHz and buffers 64 and 128, collect one hour at 120 BPM, then a programmed tempo change
and ramp, both idle and with the reference loaded session. Log tick sequence, intended device
frame `d_k`, returned arrival converted to device frame `r_k`, actual-send stamp, map quality,
late/lost counts, source settings, firmware, backend, rate, buffer and load. Require a valid
audio map throughout; arrival-derived return timing is explicitly an end-to-end USB/driver
measurement, not isolated transmitter jitter or calibration.

```text
L = median(r_k - d_k)                 # one fixed latency for the run
j_k = (r_k - d_k) - L                 # residual in device samples
```

The #183 acceptance target is **max(abs(j_k)) <= 24 samples at 48 kHz** (0.5 ms;
`ceil(0.0005*Fd)` elsewhere), zero lost/extra ticks, and absolute difference of first/last
minute median residuals <=24 samples. Report p50/p95/p99/max and fitted drift in samples/hour
as well; do not subtract a fitted slope to hide drift. Report startup/locate events separately
from steady ticks, with their actual wire ordering. K46's known 1 ms USB/scan sawtooth may fail
this target: a failure is evidence to reopen the budget or improve the path, never permission
to claim success from the filter's smooth BPM. Retain the raw trace and record the run and
limitations under `docs/benchmarks/` with an index row. No hardware result is asserted here.

## Consequences

- A single selected controller can drive transit while notes on that port keep working. Ports
  can independently receive regenerated clock or transport. The defaults preserve today's behavior.
- Tick count determines musical position, but 24 ppqn and arrival jitter limit phase evidence.
  Filtering trades response time for stability; losing ticks cannot be repaired with confidence
  from tempo alone. Freewheel protects a take and visibly relinquishes phase lock.
- The tempo track is inactive while following. Unwarped audio keeps its duration; external tempo
  does not grant general audio time stretching or persist a performance tempo map. Saved MIDI
  beats and raw audio may diverge when later played under a different internal tempo.
- Runtime beat addressing and recording-history integration make slice 3 larger than a BPM
  estimator. They are required to prevent a convincing tempo display above an unsynchronized
  renderer or incorrectly placed take. UI polish cannot be used to defer those contracts.
- Same-device send/follow is suspended to prevent a clock feedback loop. Diagnostics is a
  separate measurement mode. Devices without SPP use the explicit pattern-restart limitation.
- Offset and PDC choices affect measured alignment; USB delivery accuracy remains hardware
  evidence. The proposed 24-sample bound is a signoff target, not a new default justified by tests.

## Alternatives considered

- **Transport only.** Useful slice 1, but does not satisfy external tempo or clock sending.
- **Set the saved tempo on every F8.** Rejected: arrival jitter becomes document edits, plan
  churn and unstable phase; one estimated BPM cannot describe historical recording placement.
- **Integrate filtered BPM forever or hard-seek on every tick.** Rejected: the first drifts away
  from count; the second causes discontinuities, repeated notes and unsafe take boundaries.
- **Use callback count, UI elapsed time or input FIFO backlog as time.** Rejected by the clock,
  recording and K22 contracts. Nominally equal sample rates are not a shared clock.
- **Stop or restore session tempo when ticks disappear.** Rejected for this scope: a cable or
  clock dropout must not abruptly change a recording's tempo or finalize it without Stop.
- **Ableton Link now.** Out of scope: network peers, shared phase and source arbitration need
  their own decision, and Link does not supply these MIDI-only devices' wire transport/SPP.
- **MTC or MMC now.** Out of scope: MTC conveys absolute time rather than 24-ppqn tempo; MMC is
  a different transport protocol. Neither is required for this rig's Clock/Start/Stop contract.
- **A plug-in sync bridge.** Rejected: it cannot own global stopped transport, source selection,
  record tickets, device-clock history and per-port lifetime reliably; bypass/unload would change
  synchronization. The engine already owns the necessary boundaries.

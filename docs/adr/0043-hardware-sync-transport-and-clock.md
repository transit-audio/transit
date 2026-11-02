# ADR 0043: Hardware sync: transit follows and sends MIDI transport and clock

- **Status:** accepted 2026-11-03 (maintainer decision; proposed 2026-11-03, two judging rounds plus one amendment round for the idle clock)
- **Date:** 2026-11-03
- **Scope:** maintainer decision, issues [#93](https://github.com/transit-audio/transit/issues/93) and [#183](https://github.com/transit-audio/transit/issues/183); transport follow, clock follow and clock/transport send.
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
mapped to D and then through `ClockMap::frame_to_host` to output-thread deadlines. While stopped,
the idle grid in section 5 supplies tempo-only crossings; it does not advance S or song q.

| Mode | Transport authority | Tempo and musical phase authority | Audio scheduling and recording |
|---|---|---|---|
| Internal (all inputs Off) | Local transport | Saved session tempo map: S to q | Device D; existing rate conversion and historical D to S mapping |
| External transport only (Transport) | Chosen input's Start/Stop/Continue/SPP; local Stop always works | Saved session tempo map; F8 has no effect | Device D, as internal |
| External clock (Transport and Clock) | Chosen input's transport; local Stop always works | Incoming tick count anchors q; filtered tempo interpolates between ticks; runtime external map replaces saved tempo events | Device D remains the sample clock; historical D/S/q segments place events |

In every mode, enabled idle output uses the active tempo without advancing song position.
Its time anchor is D with an open audio stream, otherwise H; this adds no transport authority.

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
Stop when stopped with no pending start, and an SPP equivalent to a pending locate in the current
transport epoch or to the current position with certain phase. Do not deduplicate against a
last-SPP value remembered after playback has advanced: SPP p, advance, SPP p locates back to p.
Count uncertainty always makes SPP an explicit phase reset, even at the same numerical position;
it must clear uncertainty and restart acquisition rather than disappear as a duplicate.
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
the user has engaged Record. Record in Transport and Clock mode prepares the ticket and displays
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

Evaluate the estimate after every tick with a full window; complete-beat boundaries below govern
lock evaluation, not estimate updates. For an even-sized set, median is the mean of the two
middle values. The interval filter is not a guarantee that every delayed arrival is rejected:
the 2 ms perturbation in the gate remains inside J. Accept up to 0.5 BPM transient error for
ticks 120 through 144 of that exact fixture, returning to the 0.2 BPM steady-state envelope
at tick 145 (25 ticks after the perturbation). The paired long/short intervals cancel while
both remain in the window, but each window edge exposes one of them. This finite window
response is preferable here to treating a plausible arrival delay as a lost pulse. Neither
the independent source-phase bound nor the continuous-phase rule gets a transient exception.

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

Transport and Clock mode disables saved tempo events and tempo editing from acquisition through freewheel;
Locked displays **External <bpm>**. Keep the saved tempo map intact. Use a runtime piecewise map
of `(D, S, q, rate, transport epoch, loop iteration)`; each adopted block extends the last
segment continuously with `v_block`. Device frames still advance audio at Fs/Fd; the rate
correction changes musical phase and the beat grid, not the hardware sample rate or the pitch
of every audio clip. S remains the elapsed session-sample coordinate between locates/wraps;
q is read through the runtime map, not by applying the saved map to S in Transport and Clock mode.
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

External tempo history is runtime state, not a newly saved session tempo track. Leaving Transport
and Clock mode while stopped restores the saved map: MIDI retains its recorded beats, raw audio retains
its duration, and playback at a different saved tempo need not reproduce the original live
performance. Show that limitation in help. Capturing a reusable external tempo map is later
work. Offline render uses the saved map and sends no sync; refuse it while Transport and Clock
mode is active with an explanation to leave that mode first. ADR 0027 real-time bounce also requires
internal mode in this scope, so an external Stop/SPP cannot invalidate its fixed-range capture.

### 5. Sending clock and transport

Each output port has **Clock** and **Transport** switches, both off by default, and **Song
Position on locate**, default on when Transport is enabled (inactive otherwise). **Idle clock**
defaults on and is effective only when that port's Clock is on. It sends F8 while stopped so
hardware can display tempo and run an arpeggiator at that tempo. Clock alone serves devices
whose LFOs need tempo without transport. Transport alone sends state and position without F8.
Ports used only for sync are opened by the output worker independently of tracks.

While playing, generate ticks from actual applied transport spans, not a sleeping BPM timer. In internal or
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

**Idle grid:** the control or MIDI output scheduler thread generates idle ticks, never the audio
callback. Use the same sample-resolution deadline queue and per-port output delay as running
sync. The grid is a tempo oscillator, not a rendered span or a song-position counter. Internal
and Transport-only modes use the saved map's tempo at the stopped playhead; Transport and Clock
uses the followed tempo when Locked and the last valid tempo in Freewheel/Missing. During
Acquiring use the latest valid estimate, retaining the last valid tempo if no new estimate is
valid; before any valid estimate use the labeled saved-tempo fallback from section 3.

With an open, advancing audio stream anchor the grid to consumed device frames D, using its
clock key and D-to-H fit even though transport render time is stopped. With no audio stream use
monotonic H in integer nanoseconds, from an origin owned by the sync service that survives
Player replacement. Bridge a Player's H origin explicitly; never reinterpret a new origin.
At first activation seed an exact fractional grid origin at the supplied D or H readout; its
first tick is one period later. All ports share the underlying grid and apply their own delay;
enabling another port joins its next future crossing without resetting other ports.
Periods are `60*Fd/(24*B)` samples or `60e9/(24*B)` ns. Accumulate exact fractional crossings,
round each deadline up once (at most one sample or one ns), and retain the fractional remainder.
Use D-to-H plus output delay while device-anchored and H plus output delay otherwise. There is
no rendered Main reference to add A to while idle; the running handoff below aligns to Main.
On running-to-idle handoff, seed from the last running crossing's compensated presentation
instant and exact fractional phase, so dropping the render reference does not subtract A or
move the next deadline. Keep the current A revision for fault detection while sending.

A tempo edit, stopped locate into another tempo, or adopted followed-tempo update takes effect
at the next tick: keep that next deadline, latch the latest valid B there, and use its period
for the interval starting at that tick. Admit only that one future idle tick, so no train of
old-tempo ticks is queued. This also makes a stopped tempo change deterministic and continuous.

**Audio-device handover:** serialize open/close with dispatch. At an orderly close convert the
exact next D crossing through the last valid fit into H before retiring that clock, cancel its
unsent device-generation entry, and requeue that same ordinal once in H. At open keep H until
the new D-to-H fit is valid, then convert that same next crossing into the new D domain. Preserve
its host deadline, fractional phase and per-port delay (applied exactly once), within one new
domain unit of rounding; never reset the grid, duplicate a tick or catch up skipped ticks.
These explicit, continuous handovers are the only clock-key-change exception to the fault rule
below. An unexpected reset, missing bridge, expired fit or spacing violation uses that fault
rule; an open but invalid device map is approximate and cannot pass device-timing signoff.
Closing the MIDI output device/port itself stops idle output and cancels its queued generation;
closing only the audio stream hands the still-open MIDI port to H.

Idle F8 carries no song position and implies no transport state. No ticks between Stop and a
later Start/Continue count toward song position, with Song Position either on (Song) or off
(Pattern). The receiver contract is Start resetting to song start on the next Clock, regardless
of the number of prior idle ticks. Stop still releases external destinations and sends FC when
Transport is on; eligible idle ticks continue after FC under the idle generation, with no replay
of canceled running ticks. Clock off, Idle clock off while stopped, port closure, feedback
suspension and the fault cancellation window suppress idle ticks. Transport off alone does not.

**Deadline changes and port faults:** clock ticks never use the note path's `Track::last`
deadline clamp. Each port retains a sync generation, the last successfully sent tick ordinal,
and the last admitted tick's immutable deadline and local period. Check crossings in tick order
before inserting them into the deadline-sorted queue. Never retime already queued ticks when
a later span changes its map. A change of A in any applied span while sending faults that port's
sync, even if its next deadline would still be increasing. A continuous map revision is allowed
only if each new deadline exceeds the previous admitted deadline by at least half the smaller
of their two local tick periods, converted through their respective presentation maps. Equal,
reversed or more closely spaced deadlines fault; an unexpected clock-key reset or a missing
required running span also faults. Stopped idle generation requires clock observations, not
transport render spans.
This permits continuous tempo ramps while refusing a discontinuous compensation or mapping change.
These checks, successful-send accounting and late/bunched-tick faults also cover idle ticks;
ordinary tempo edits use the next-tick rule above, never the note deadline clamp.

The output worker serializes admission, fault handling and dispatch. On detecting a fault,
advance that port's sync generation before any further dispatch, cancel **all unsent sync
events** of the old generation (including earlier pending ticks), and reject later-arriving
old-generation spans. This observation is the cancellation boundary; it cannot undo bytes
already handed to the MIDI driver. Keep their successful-send count and ordinal, never replay
them, and record the canceled ordinal range/count and the triggering span/map/A in diagnostics.
Send Stop as the next sync message when Transport is enabled; a sticky pending Stop survives
queue saturation and precedes any resync. Show **Sync fault: timing changed; resync required**.
There is no claim of continuous receiver position across the missing ticks. Local playback and
recording continue; this fault affects only that port's sync, not the take or note-release reserve.

Recovery requires a fresh transport start or explicit per-port **Resync** request after the
worker acknowledges the fault. Use a new generation and the current applied position, never
the canceled tick count: with Transport and Song Position enabled send Stop (unless already
successfully sent), SPP, Continue, then Clock, using the preparation lead and representable
sixteenth rules below. With Song Position disabled use Stop/Start and show Pattern restart.
Clock-only output emits no transport bytes: it stays faulted until an explicit Resync or fresh
start, resumes at a newly prepared crossing, and warns that the receiver's song position must
be reset externally. Do not automatically recover on the next good span. Already-sent ticks
remain historical facts; only the new ordered position sequence establishes receiver phase.
Idle output is canceled at the same fault boundary and stays silent until this explicit recovery;
a good clock fit alone cannot restart it. If Resync is requested while locally stopped, discharge
the sticky Stop first, acknowledge recovery in a new generation, and resume only tempo-only idle
ticks at a newly prepared future crossing of the shared grid, without resetting other ports.
Retain the locate for a later ordered start; do not
send FA/FB and start the receiver while transit is stopped. Clock-only recovery keeps its warning.

Wire ordering is explicit (F2 is a three-byte System Common message, not a real-time byte):

- Starting at song zero sends **Start, then first Clock**, with the first F8 representing q=0.
  Reserve a preparation lead of `max(one tick period, 5 ms)` before that tick. It is lead time,
  not an extra counted tick. Prepare playback far enough ahead on a local start; an external
  start that arrives too late is sent in order and reported late, not silently backdated.
  From idle, choose a future crossing on the existing grid with this preparation lead, send
  FA, then the next emitted F8 at that crossing as q=0. Reassign musical phase there without
  resetting the time grid or treating its idle ordinal as song position. Prepare local playback
  so its compensated Main q=0 reference is that crossing before per-port delay; running spans
  then take over the same grid. Preserve already-sent idle ticks and cancel only superseded
  queued entries, with one producer owning each boundary tick. No idle F8 may slip between FA
  and the designated first running F8. If the 5 ms minimum exceeds a tick period, suppress
  intervening grid slots during preparation and report that intentional preparation gap;
  the underlying grid stays continuous, with no phase reset or catch-up burst. Late external
  starts retain the existing late-report rule; any unsafe spacing takes the port-fault path.
- Resume without a locate sends **Continue, then Clock**, retaining the next unsent tick and
  fractional musical phase, unaffected by idle tick count. Align the first running crossing to
  the idle grid with the same preparation/handoff rule; do not advance retained song position.
  Stop sends **Stop** and cancels all future ticks for that transport generation, while eligible
  idle output takes over the next crossing without resetting the time grid.
  A duplicate local request that changes no state emits nothing.
- A representable stopped locate sends **SPP** if enabled; nonzero initial play sends **SPP,
  Continue, Clock**. An unaligned stopped locate retains a pending target until the
  next-sixteenth rule below can apply at playback; it does not transmit a floored SPP.
  A playing locate or cycle jump sends **Stop, SPP, Continue, Clock** at the new position, with
  no F8 between Stop and Continue. Stop is due at the jump boundary, never early enough to
  truncate the old lap. The first new-position F8 follows the ordered preparation sequence;
  report any resulting resync gap rather than advancing a hardware song before the local
  jump. Old-generation ticks at/after the jump are invalidated.
  Idle output is also suppressed for this ordered resync sequence, including its preparation
  gap; it cannot insert F8 between Stop and Continue (or Stop and Start in Pattern mode).
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
if a tick is a whole tick period late, enter the port-fault procedure above and report missed
ticks. Also fault rather than dispatch two F8s less than half the smaller local tick period
apart by actual-send stamps; minor lateness must not bunch otherwise ordered deadlines.

**Feedback:** never enable sync sending to the same physical device as the active sync input
(`same_midi_device`, including aliases); keep the requested switches but show them suspended.
The existing note echo guard and storm detector are insufficient because Clock is excluded.
Other output ports may follow transit; there must be one source in the hardware wiring, with
MIDI thru disabled on return paths. Generation checks prevent software replay, not arbitrary
multi-device cabling loops. The diagnostic loopback below explicitly bypasses the same-device
send guard while disconnecting the returned input from transport control and the follower.

### 6. Preferences and bounded publication

Store per-input mode and `input_delay_ms`, per-output Clock, Transport, Song Position and
`idle_clock` switches and `output_delay_ms`, in
machine preferences (`ui.json`), keyed by backend, stable port id when available, otherwise
name plus ordinal. Delay defaults to zero, finite range -500..500 ms. Positive input delay adds
to the tick's phase-anchor time (transit plays later); positive output delay sends later.
Neither changes raw note/capture stamps. All sync-setting changes apply while stopped. Missing ports keep
their preferences and show Missing; never bind a different device merely because its enumeration
index matches. Reconnection starts a new generation, acquires clock afresh and never starts
playback by itself. An output reconnect cancels old ticks and remains stopped until a fresh
transport start/resync, including idle output; it never drains the old backlog. An initial open
of an enabled port may start idle ticks without starting playback. Uncertain aliases stay unbound until
selected explicitly.

All new preference fields have defaults; there are no new required session fields, format bump
or `needs` marker. Absent Clock/Transport preferences default to Off; absent `idle_clock`
defaults to true, inactive when Clock is off. Preserve an explicit false across Clock off/on.
Help says "Send tempo while stopped; does not start or locate the receiver." Song Position
does not change idle behavior. An older session needs no migration
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
| 2. Clock send | Applied transport spans to output worker, 24-ppqn deadlines, tempo-map changes, per-port Clock, Idle clock and delay; stopped D/H grid and device handover. | Scripted device/map verifies crossings within one device sample (one ns without a device), no accumulated rounding, running and stopped tempo changes, idle-to-Start continuity, device open/close handover, rate conversion, cycle/locate invalidation, delayed worker, PDC and offset sign; compensation/map fault, generation cancellation and ordered resync cases below; strict callback allocation gate; running and idle reference hardware diagnostics before claiming #183's jitter target. |
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

- Use these separate exact fixtures at 48 kHz, with a perfect supplied H-to-D map, zero offset,
  and all arrivals/readouts injected. Let H0 = 1e9 ns; round each supplied stamp once to integer
  nanoseconds. Constant-tempo truth is `h_i = H0 + i*60e9/(120*24)`, for i=0..192. Set
  `H_i = h_i + (-250000 ns if i is even, +250000 ns otherwise)` and add exactly 2000000 ns
  to H_120. After four consistent beats (n=96), require `abs(B_hat-120) <= 0.2 BPM` except
  n=120..144 inclusive, where the bound is 0.5 BPM. Check **every tick**, including both
  boundaries: B_120 = 119.5219123506 and B_144 = 120.4819277108 BPM (within 0.00001 BPM
  for stamp rounding); B_145 returns to 120 within that tolerance. All 24 intervals are
  accepted at those two failing boundaries from round 1. Recovery is mandatory by tick 145,
  not a new acquisition grace period. Include an unperturbed counterpart with no exception.
- For the ramp fixture, source time t is seconds, with B(t)=120 for 0..8, then
  `120 + 0.2*(t-8)` for 8..68, then 132 through t=76. Its integrated truth is q(t)=2*t
  through 8; `16 + 2*u + u*u/600`, u=t-8, through 68; then `142 + 2.2*(t-68)`.
  Solve q(t_i)=i/24 for ideal source pulses. Run epsilon in {-0.0005, 0, +0.0005}, set
  `h_i = H0 + t_i*1e9/(1+epsilon)`, and add the same alternating +/-250000 ns jitter,
  with no 2 ms perturbation in these separate ramp runs. True BPM in host/device time is
  `B(t)*(1+epsilon)`. Require error <=0.2 BPM at every post-acquisition tick; there is no
  transient allowance for the ramp or source/device mismatch.
- For both fixtures check `abs(phase_error_samples) <= 96` at every post-acquisition block
  boundary, including ticks 120 and 144 and their following blocks. Independently compare
  q_play with the ideal source q at that device frame, **not** q_target reconstructed by the
  estimator; divide that quarter error by the true quarters/device-sample rate and require
  <=96 samples too. Feed snapshots causally (only arrived ticks) and test the block partitions
  below; neither oracle may hide an extra/missing pulse behind smooth BPM. Require zero
  drift-induced phase jumps and a correction term within 1%. These remain implementation
  acceptance bounds, not claimed gate passes. At constant exact tempo expect the exact
  24-tick estimate and no accumulated drift over one synthetic hour.
- Lock transitions use the four complete consistent beats, including exactly-at/just-past
  one-beat expiry; supply zero/backward stamps, duplicate/dropped F8, a burst sharing a stamp,
  an out-of-range BPM, a tempo step, old generations and invalid/stale clock maps. Verify
  Freewheel retains the last tempo, keeps an active take, and requires phase resync after loss.
- Run full transport sequences with cycle on/off, FA/FB/FC repeated, SPP 0 and 16383, malformed
  F2, realtime bytes inside notes/F2/SysEx, notes and clock on the same port, no note routes,
  Monitor closed and Learn active. Check recording ticket cancellation, pending Start then
  Stop, SPP p then advance then SPP p, same-position SPP after count uncertainty, locate while
  recording, controller-only takes and recovery; compare delayed versus
  immediate event drain. Negative controls must fail when arrival is replaced by drain time,
  when the saved map is used under external follow, or when raw ticks are integrated as BPM only.
- At session/device rates 44.1/48/96 kHz and block partitions 1, 17, 64, 128 and 512, use a map
  with a change at quarter 2.5 (mid-bar), plus meter changes, to verify `40*k` model-tick output
  crossings. Test locate between sixteenths, maximum SPP, nonzero cycle start, transport-only
  send, Clock-only send, disabled SPP, same-device suppression and loopback isolation. Inject
  A changes and +/-offsets; prove Start precedes first F8, no obsolete tick survives Stop,
  and a stalled sender cannot burst catch-up clocks or exhaust note-release reserve.
- Drive stopped idle output at 120 BPM from exact D=0 at 48 kHz: ticks fall at 1000, 2000,
  3000, ... samples, independent of callback partitions or control drain delays. Repeat in H
  with exact period `1e9/48` ns, rounding each crossing once, and at 44.1/96 kHz and fractional
  tempos for one synthetic hour; error is at most one sample or one ns, without accumulation.
  Adopt 60 BPM between ticks at 2000 and 3000: the next tick remains 3000 and latches the new
  rate; subsequent ticks are 5000, 7000, ... . Repeat with followed-tempo updates, a stopped
  playhead move into a new tempo, last-valid freewheel and pre-acquisition fallback.
- After N idle ticks, for N in {0, 1, 37, 192}, prepare Start on the unchanged 120 BPM grid.
  Inject FA at crossing N after that idle F8 (at the origin for N=0), then the first running
  F8 at crossing N+1, exactly 1000 samples later: assert FA-before-F8, the full preparation
  lead, q=0 for that F8 for every N, and q=1/24 at the following crossing. No idle count becomes
  a song offset. Assert compensated local q=0 alignment, one owner per boundary tick, +/-port
  delays and multiple ports joining one grid. Repeat Stop-to-idle, Continue with retained
  fractional position, Song/Pattern modes and Clock-only output. At 1000 BPM assert the 5 ms
  preparation gap suppresses intermediate slots without changing grid phase or bursting later.
- Hand over D-to-H and H-to-a-new-D with injected clock keys, origins, fit observations and
  readouts, including before/at/after a due tick and while a tempo update is pending. A normal
  handover keeps the same next ordinal/deadline within one destination unit, applies delay
  once and emits it once; cover a changed device rate. An invalid bridge or overdue/bunched
  tick faults and stays silent until acknowledged Resync. Closing the MIDI port, disabling
  Clock/Idle clock, reconnecting, feedback suppression and fault cancellation must invalidate
  queued idle entries; delayed old entries cannot escape. Stopped Resync sends no FA/FB and
  resumes idle only after the sticky FC. A playing locate/resync admits no idle F8 inside its
  Stop/SPP/Continue or Stop/Start sequence. Test default-true and persisted-false preferences.
- Script the compensation reversal at Fs=Fd=48000 and 120 BPM: tick k has uncompensated
  frame 48000 and A=4800, so it is queued for frame 52800; the next span has A=0 and
  tick k+1 at frame 49000. Present both spans before either deadline. Expect a port fault
  on the A change, both ticks canceled/accounted for (k+1 refused before admission), no F8
  at 49000 or 52800, and Stop when Transport is enabled. A fresh acknowledged Resync must
  send the ordered position sequence in a new generation; a delayed old span cannot escape.
  Repeat with k already successfully sent: keep exactly that one send, suppress k+1 and
  require the same fault/resync. Also inject an A increase, unchanged-A map reversal,
  equal/too-close future deadlines, and a map update that preserves the spacing bound;
  only the last continues without a fault. Exercise Transport off and Song Position off.
  Check canceled/refused ordinals, last successful ordinal, fault visibility, and the new
  phase reset; no reordered/bunched F8s or silent loss of clock count is an acceptable result.
- Count allocations/frees and MIDI sends over the actual callback path, including adoption,
  loss, overflow, wrap, recovery and idle handoff. Idle ticks and device handovers perform no
  tick generation or sends on the callback. Assert coherent tempo/anchor snapshots under a scripted
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
and ramp, both with an unloaded and with the reference loaded session. Measure playing and
transport-stopped Idle clock separately under both loads; the page shows idle-tick jitter,
late/lost counts and drift alongside running-tick results using the same calculations and bound.
Log tick sequence, idle/running classification, anchor domain/generation, intended device
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

Without an audio device, run the stopped diagnostic in H: substitute intended deadline `h_k`
and returned monotonic arrival `a_k` for d_k/r_k, and compute one median latency and residual
`j_k = (a_k-h_k) - median(a_k-h_k)` in ns. The target is `max(abs(j_k)) <= 500000 ns`, zero
lost/extra ticks and first/last minute median difference <=500000 ns; report percentiles and
drift in ns/hour. This is a separate monotonic-domain measurement, not device-sample evidence.
Split results at anchor handovers and report their gap/rounding separately. The deterministic
deadline bound (one sample or one ns) is distinct from these measured USB/driver jitter targets.

## Consequences

- A single selected controller can drive transit while notes on that port keep working. Ports
  can independently receive regenerated clock or transport. Clock and Transport remain off by
  default; enabling Clock also enables stopped tempo output unless Idle clock is explicitly off.
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
- A compensation change interrupts outgoing sync and needs explicit resynchronization; notes
  retain their existing safety policy. A one-beat interval window permits the specified brief
  BPM excursion on a delayed arrival while phase remains continuously controlled.

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

## Judging round 1: responses

- C1 accepted: compensation changes and nonmonotonic/too-close map deadlines now fault per-port sync, cancel old-generation pending events without the note clamp, preserve already-sent counts, expose losses, and require ordered resync; the 52800-to-49000-frame regression specifies outcomes for pending and already-sent ticks.
- C2 accepted: retained the estimator and exact alternating-jitter/2 ms fixture, specified a 0.5 BPM transient envelope at ticks 120..144 and recovery at 145, kept the 0.2 BPM steady-state/ramp bound, and required independent source-phase and controller-error checks within 96 samples without a transient exception.
- M1 accepted: SPP deduplication is scoped to the current position/epoch or equivalent pending operation; returning to an earlier SPP target locates again, and count uncertainty always permits an explicit same-position phase reset.
- M2 accepted: tempo-map suppression, restoration and offline-render restrictions explicitly name Transport and Clock mode in this ADR and the clocks knowledge page; Transport-only mode retains the saved map.

## Amendment 2026-11-03: idle clock while stopped

Approved 2026-11-03. Replace the stopped-clock deferral in section 5 with per-output **Idle
clock**, on by default when Clock is on. The requested Live-style behavior is continuous tempo
for a hardware display or arpeggiator even when transit is stopped. Song and Pattern receivers
get the same idle F8 stream; it conveys neither song position nor a transport command.

Section 5 now specifies the active-tempo source, off-callback generation, exact fractional D
grid with an open stream and H grid without one, and phase-preserving device open/close handover.
Tempo changes latch at the next tick. Stop sends FC and releases external destinations while
idle ticks continue; Clock off or MIDI-port closure cancels them. The existing resync fault
cancels idle ticks too and requires explicit recovery before they resume.

Start resets receiver position on the next Clock. A prepared Start therefore precedes the next
emitted tick on the existing time grid, whose musical position becomes zero regardless of N
prior idle ticks. The preparation lead, ordered locate/resync sequences, already-sent accounting
and exclusion from the note-deadline clamp remain in force. A required preparation gap suppresses
grid slots without moving the grid. The gate now covers idle trains, next-tick tempo edits,
Start after N idle ticks and both device handovers in injected samples or scheduler ns, plus
stopped jitter diagnostics with the same 0.5 ms hardware target expressed in the active domain.
Section 6 and the clocks knowledge page carry the preference, lifetime and clock-domain rules.
Earlier judging responses remain intact; this amendment changes no follower or recording gate.

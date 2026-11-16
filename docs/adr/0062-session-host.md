<!-- transit-index
title: The session host: transport and recording orchestration move out of the interface crate into `transit-host` (layer 5), with one typed transport phase as the only truth (a real-time render included), explicit capabilities for every safety reader, status text arbitrated per transition, an explicit count-in stage from the audio thread, time as a clock capability, a restricted service facade so the interface can no longer name the Player, headless playback, and blocking work staged or moved off the interface thread
-->
# ADR 0062: The session host: one typed transport phase, outside the interface

- **Status:** proposed 2026-11-16 (revised after judging round 1, same day)
- **Date:** 2026-11-16
- **Implementation:** not started. Slices S1, S2a to S2d, S3a to S3f, S4, S5a and S5b below; tracking issue #739 (topic B of #736). S1 and S2a to S2d land before hardware sync slice 1 (#733, ADR 0043); S3 onward runs beside sync and does not block it. The transition matrix, the simulation table, the reader assignments and the display examples are in the appendix [0062/tables.md](0062/tables.md), which is part of this record.
- **Reopen when:** a record changes the Player so that it may live on another thread (alternative iii); sync slice 1 cannot express its machine as phase variants and matrix rows of this record (section 15); a measured device reopen on the reference machine holds the interface thread for more than one second after S5b; the display arbitration of section 6 fails the maintainer's feel-right check; a host file nears the 1,500-line budget and the split by concern does not fit section 11; headless recording is asked for (section 13).

## Context

The read-only inventory for #739 was taken at 3b12412e; every fact below was re-read at `main` 1a56b895. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

- **The host is the interface struct.** Transport and recording are methods on `StudioApp` (`lib.rs:214`), which holds the Player (`:218`), the request tracker (`:219`), the recorder (`:221`), the status line (`:267`) and egui types (`:319`, `:342`). `host/` is 4,241 lines with tests. 57 non-test interface files name the Player or `transit_playback`.
- **The state is flags and text.** No type names the transport's states; each is a combination of `player_requests.*` (`host.rs:263-271`), `take_plan`, `Player::count_in()` (`P/player/device.rs:897-900`), `sim_count_in`, `engine.transport.{playing,recording}` and the status string. "Play pending" (`host/transport.rs:23`), "Stop pending" (`:41`), "Count-in" and "Record pending" (`host/recording.rs:153, 161`) are compared at `host.rs:343, 354, 380` and `host/transport.rs:59, 165`, and `status.rs:26-28` keeps them from ageing. Section 10 lists the defects this causes.
- **Transport is started outside the host code too.** The real-time render starts Play inside `Player::start_realtime_render` (`P/player/device/realtime_render.rs:106`, called from `finish/realtime.rs:240`) and stops it on completion, failure and cancel (`:137`, `:165`, `:188` via `stop_and_release`). No host request tracks those calls.
- **Where the step runs.** `logic()` (`frame_logic.rs:309`) drains control (`:325`), polls the Player (`:329`), follows a pass (`:335-337`), watches devices (`:355-357`), runs `handle_transport(dt)` (`:363`), the automation write pass (`:364`), session sync (`:365-369`), autosave (`:370-372`) and input (`:373-375`). MIDI transport buttons run in the drawing pass (`:467`) and call `handle_transport(0.0)` per event (`midi_transport.rs:270`).
- **The Player stays on its thread.** "Main-thread facade. RuntimeControl deliberately makes this type !Send/!Sync." (`P/player/device.rs:69`); "Creating-thread owner" (`P/player/runtime.rs:20-23`). Whatever holds it runs on the thread that made it.
- **Nothing headless plays.** `Headless` clears every transport request (`crates/automation/src/lib.rs:53-54`). The production null Player has "no input and no MIDI ports" (`P/player/device/null_open.rs:11-12`).
- **Who needs it next.** ADR 0043 slice 1 needs "a deterministic state matrix" without a window (ADR 0043, Implementation); section 15 maps its ten requirements.
- **Conventions.** None from Ableton or Logic applies to internal structure. Ableton Live "will not begin recording until the count-in is complete" and Logic's count-in "switches to record when it reaches" the playhead (both as quoted in ADR 0052, Context): both treat the count-in as not recording, which is this record's meaning of the flag. Transit still captures MIDI during the count-in on the audio thread (ADR 0052 decision 8); that capture is not the flag. No convention bears on which button pulses (B6) or when half speed ends (B7).

## Decision

### 1. Settled

By the maintainer (2026-11-16): M1 orchestration moves into a host with no interface dependency and a typed transport state; M2 the flag disagreements are fixed in slice 2, each a named change with its own test; M3 S1 and S2 land before sync slice 1. Standing rules: no migrations while alpha; no interim user-facing surface a recorded plan replaces; exact where a professional could null-test; nothing audible changes that the user did not ask for; 1,500-line files; related defects become task lines on #739. Coordinator rulings after round 1 (R-A to R-K) are applied in sections 3 to 15 and listed under Judging round 1.

Nothing here changes capture, the latch or any saved file. One playback change is made: the record-start stage publication of section 5 (S2c).

### 2. The crate and the facade

- `transit-host` at `crates/host`, `("transit-host", 5)` in `xtask/src/main.rs` `LAYERS`. Normal dependencies: `transit-engine`, `transit-playback` (L4), `transit-model`, `transit-mix`, `transit-time`, `serde_json`, `log`. A `native-jack` feature forwards to `transit-playback/native-jack`. `check_layers` bars UI crates (`:211`) and upward edges (`:219`; it rejects only `dl > layer`, so L5 to L4 and L5 to L5 pass); dev dependencies are skipped (`:208-209`). Unsafe stays forbidden.
- `Host` owns the Player, the recorder, `PlayerRequests`, the phase and its facts, the simulation, starts, stops, locates, loops, recording, the MIDI journal lifetime, the autosaves that depend on transport, device reopen, and **every call that starts or stops the Player** (section 3, R-A). `StudioApp` holds `host`.
- `Host::step(&mut self, engine: &mut Engine, status: &mut StatusLine, clock: &dyn HostClock, input: &StepInput, out: &mut Vec<HostEvent>)`. The host borrows the engine and the status line; it owns neither.
- **The facade (R-B).** The interface never receives `&Player` or `&mut Player`. `Host` hands out narrow handles, each a struct borrowing the host that exposes only its permitted methods:

| Handle | Permitted (Player methods behind it) | Interface users today |
|---|---|---|
| `PluginServices` | editors (`want_editor`, `drive_editors`, `close_editor`, `idle` edits as events), preset and state jobs, slot load and stuck state, retry, instrument and insert parameter echo | `editors.rs`, `editor_group.rs`, `plugin_header.rs`, `host_hang.rs`, `unresponsive.rs`, `devices.rs`, `mixer/inserts.rs`, `mixer/actions.rs`, `external_instrument.rs`, `sampler_panel/behavior.rs`, `scope_strip.rs` |
| `Readouts` | meters snapshot, telemetry, MIDI input and output status, monitor activity, scope hub, plug-in timings, the playing and position pair of the step | `telemetry.rs`, `debug_log.rs`, `midi_monitor.rs`, `midi_output.rs`, `midi_input.rs`, `midi_learn.rs`, `mixer/channel.rs`, `arrange.rs`, `arrange/tracks.rs`, `about.rs`, `diagnostics.rs`, `render_measurement.rs` |
| `LiveInput` | audition notes, step input, MIDI Learn capture | `keys.rs`, `keys/step.rs` |
| `DeviceServices` | device listing, settings panel, low latency, I/O map, monitor controls, click level, calibration, measurement start, poll and cancel, preview player | `audio_devices.rs` and `audio_devices/*.rs`, `audio_devices_system.rs`, `hardware_measure.rs`, `hardware_ui.rs`, `external_measure.rs`, `io_settings.rs`, `low_latency.rs`, `main_monitor.rs`, `ui_state.rs`, `browser/preview.rs` |
| none (moves into the host) | transport, recording, render transport, reopen, Player installation | `host.rs`, `host/*.rs`, `transport.rs` (to `:134`), `metronome.rs` (to `:96`), `midi_transport.rs` (to `:304`), `arrange/recording_preview.rs`, `frame_logic.rs`, `lib.rs`, `menus.rs`, `splash.rs:129-136`, `audio_connection.rs`, `finish/realtime.rs` |

- **The edge.** At S3f `transit-ui-studio` loses its normal dependency on `transit-playback`; `transit-host` re-exports, item by item in `transit_host::data`, the plain data types the interface still names (meter snapshot, count-in plan, audio configuration and device listing, monitor settings, slot load, telemetry, MIDI status types). It never re-exports `Player`, `Recorder`, `RuntimeControl`, `Control` or `action_lane`. The dev dependency stays for test fakes; production interface code cannot name the Player. The opener closure in `Services` (today returning a `Player`) becomes `transit_host::OpenAudio`, opaque to the interface.
- **The guard** becomes the type system plus one manifest rule (section 14). The source scan is a transitional lint only.

### 3. The transport phase

```rust
pub enum TransportPhase {
    Stopped,
    StartPending { seq: Sequence, from: Samples, prev: Prev },
    Playing,
    RecordPending { seq: Sequence, ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    CountIn { ticket: Ticket, from: Samples, plan: CountIn },
    WaitingForClock { ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    Recording { latch: Option<Point>, count_in: Option<CountIn> },
    StopPending { stop: Option<Sequence>, record_off: Option<Sequence>, then: AfterStop },
    Rendering { finishing: bool },
}
pub enum Prev { Stopped, Playing }
pub enum AfterStop { Park, Resume(Samples), Reset }
```

| Variant | Data and why |
|---|---|
| `StartPending` | the Play request, its start, and `prev`: where a refused Play returns. A Play is validated before the audio thread changes anything (`P/player/audio.rs:445-453`), so a refused replacement Play leaves the old playback running |
| `RecordPending` | the Record request, the record-start ticket (`P/record/start.rs:17-20`) to match the stage observation, the start (`None` for a punch), the count-in sent |
| `CountIn`, `WaitingForClock` | the ticket; entered on the receipt or on the stage observation, whichever comes first (section 5) |
| `Recording` | the latch (`None` only simulated); the count-in plan, kept for the take's plan fallback (`host/recording.rs:418`) |
| `StopPending` | the Stop request and the record-off request (`finish_recording` sends `Recording(false)`, `host/recording.rs:378-381`), each `None` once acknowledged; Park leaves when `stop` is `None`, Resume when both are; `Reset` when a session replaced the document. Reset supersedes Resume (section 9) |
| `Rendering` | a real-time render owns the transport (R-A); `finishing` once its result is taken or it is cancelled; it leaves for Stopped only when the step observes the Player not playing, so the phase never reads Stopped while the Player plays |

**Facts beside the phase** (not phases): speed `Normal | Half` (invariant: `Half` only in StartPending or Playing; every other transition sets `Normal` and `Player::set_speed(1.0)`, B7); the wanted loop, the loop last sent and the loop a pass records under; release, locate and session requests outstanding; observed playing and position (sampled together, B9); the working cursor and retained position (section 8); reopen state (waiting, gave up, `ReopenPending` while staged, S5b); `InputOpening` (S5b); `faulted`, set by `PlayerError::Faulted` or a Player fault and cleared only by replacing the Player; held commands (section 9); the MIDI transport `recovering` flag; the interlocks that stay in the interface (`measuring`, External Instrument measure; refused at `run`, `frame_logic.rs:47-66`); the backend, Player or simulated (simulated data in [tables.md](0062/tables.md), section B); ADR 0043's later facts (section 15).

**The sweep for calls that start or stop the Player (R-A).** In the host already: `host/transport.rs` and `host/recording.rs` (play, stop, locate, set_loop, set_speed, record, set_recording, all_notes_off), and every `stop_play` caller (`host/reconnect.rs`, `host/autosave.rs:114-116`, `menus.rs:18-19`, `midi_transport.rs:162-173`, `frame_logic.rs:148-151`, `host/recording.rs:565`). Moving into the host: the real-time render (`finish/realtime.rs:190-330` around `start_realtime_render`, `realtime_render_result`, `cancel_realtime_render`), Player installation (`splash.rs:129-136`) and replacement (`audio_connection.rs:62-86`, `host/reconnect.rs:212-255`), which drop the old Player. Checked and not transport: the hardware probe, loop calibration and External Instrument measure refuse while playing and start no transport (`P/player/device.rs:265`, `P/player/device/calibrate.rs:25`, `P/player/device/external_measure.rs:31`); the browser preview is a separate player outside the plan (`Player::preview`), and the interface stops it on a playing edge (`browser/preview.rs:272`); audition notes (`Player::note`) start nothing.

### 4. The state table

| Phase | Entered by | Audible | State text | Record button | Commands with an effect |
|---|---|---|---|---|---|
| Stopped | launch; refusals from a stop, stop completion, failures, device events, render end | no transport sound (live instruments sound, E3) | none | off | all |
| StartPending | a Play-family request accepted | prev's sound (nothing, or the old playback) | Play pending | off | replacement starts, Toggle, Stop, Pause, Record, Locate (cursor only) |
| Playing | Play applied | playback | none (outcome "Playing") | off | all but Play |
| RecordPending | Record accepted | from stopped nothing; punch playback | Count-in when a count-in was sent, else Record pending | off | Toggle, Stop, Pause, Record (cancel), Locate (take-safe) |
| CountIn | admission or stage | the click | Count-in | pulses while the published counter runs (as today, `transport.rs:193-197`) | the starts, Toggle, Stop, Pause, Record and HalfSpeed cancel; Locate take-safe |
| WaitingForClock | admission without count-in, or stage | from stopped nothing (parked); punch playback | Record pending (B1) | lit | Toggle, Stop, Pause, Record (cancel), Locate (take-safe) |
| Recording | latch | playback and capture | none (outcome "Recording") | lit | Toggle, Stop, Pause, Record (take finalized), Locate (take-safe) |
| StopPending Park / Reset | a Stop-family request; device trouble; lap limit / ResetSession | playback until the audio thread applies Stop | Stop pending / none | off | Stop, Pause, Toggle re-send; Locate (cursor); starts and Record held (D4) |
| StopPending Resume | Locate during a pass | as Park | Stop pending | off | Stop, Pause, Record drop the resume; Locate retargets; others deferred |
| Rendering | a render started by the host | the render's playback | none (the render dialog shows progress) | off | all refused with the render's text; All Notes Off cancels the render first (today `finish/realtime.rs:164-171`) |

### 5. Transitions

The total matrix is [tables.md](0062/tables.md), section A: every `TransportRequest` (`E/lib.rs:87-101`), every receipt kind (Play, Record start, record-off, Locate, Stop frontier, release frontier, Session rejection, untracked refusal, fault), the stage and playing observations, the render, device events, the live loop change, MIDI loss, the lap limit, held-command release. Each cell gives the phase entered, the effects, or the refusal; each cell that differs from today names its B or D item. The simulated transport has its own table (section B). The rules the matrix applies:

- **A command's target phase is entered when its Player request is accepted** (`Sent(kind, seq)`). A local refusal keeps the phase and posts its message; a Record refused after arming runs `finish_recording` (disarm, discard the journal), as `host/recording.rs:129-147` does. A Player error keeps the phase and posts the error; `PlayerError::Faulted` sets the `faulted` fact.
- **While `faulted`**, the starts (Play family, HalfSpeed, Record) refuse with the fault's text until the Player is replaced; finalization runs with Player errors ignored (the recorder's take and the MIDI pass are finished from what the host already holds).
- **Untracked refusals** still reach the status line (`host.rs:362-364`, pinned by `player_tests.rs:99-116`); stale receipts of a tracked kind clear nothing.
- **Record-start stage (R-H).** The audio thread publishes, with one release store per change, `shared.metro.record_stage: AtomicU64` holding the record-start ticket's sequence (56 bits) and a stage code: 1 counting in, 2 waiting for the clock, 3 started, 4 failed, 5 cancelled. Writers: `count_in` (`P/player/audio.rs:372-413`), `record_within` when it waits (`:296-299`, `:332-340`) or starts (`:366`), the count-in end and the waiting retry in `render_next` (`:942-958`), every failure and cancel path. Reader: `Player::record_stage() -> Option<(Ticket, RecordStage)>`. A stage for another ticket is ignored. CountIn leaves only on stage 2, 3 or 4 (or the latch); nothing is inferred from elapsed time or from `count_in()` reading `None`. A step that misses the whole count-in sees stage 2 or 3 directly. The published counter (`count_in()`) stays the display's source for the countdown and the pulse.

### 6. Status text (R-E)

- `StatusLine` (plain data, no egui; in the interface crate in S2c, in `transit-host` from S3b): `message: Option<Message { text, kind: Outcome | Note, rev, since }>`, `state: Option<State { text, rev }>`, `next_rev`. `status.set(text)` replaces `app.status = ...` at its 333 writers (a mechanical change in S2c); it takes a new revision when the text differs, and `say_again` (today's `StatusAge::restart`, #216) takes one for equal text. `StatusAge` ages only `message`.
- `state_text(phase, facts)` is the one producer of state texts: ReopenPending "Reopening the audio device" over the phase's text ("Play pending", "Record pending", "Count-in", "Stop pending") over InputOpening "Opening the audio input" over release pending "All MIDI notes off pending". `is_state` is deleted.
- **Shown:** `message` when it is newer (`rev`) than `state`, else `state`, else `message`, else empty.
- **Arbitration, once per transition**, with the shown line before the transition as input, in the executor, in transition order: (1) the new state text takes a fresh revision if the line before showed a state text, nothing, or an Outcome; it keeps the old state revision if the line before showed a Note, so a recent failure stays visible; (2) an outcome ("Playing" on Play applied, "Recording" on the latch, "Stopped" on a Park completion, "All MIDI notes off" on the release frontier) is written as an Outcome message only if the line before showed a state text or nothing. The decision is carried in `HostEvent::PhaseChanged { from, to, shown_before, outcome_written }`.
- The six sequences of round 1 are worked in [tables.md](0062/tables.md), section D, and each is an S2c test.
- `ui.inspect`'s `status`, the status bar and the diagnostics report read the shown line; `StatusAge::last` keeps messages only.

### 7. Capabilities (R-D)

The host publishes, after every transition (not only at step end), into `engine.transport`:

| Field | Meaning, over the phase and facts |
|---|---|
| `playing` | playback observed: the sampled `Player::is_playing()` of the step, or the simulated state (B9) |
| `position` | the observed position while not Stopped; the working cursor while Stopped (section 8); always published with `playing` |
| `recording` | `Recording` |
| `record_engaged` (new) | `RecordPending`, `CountIn`, `WaitingForClock`, `Recording`, `StopPending Resume`; later ADR 0043's armed external start with Record |
| `finalizing` (new) | resources still need finalization: a take plan, a recorder take pending, or a MIDI pass prepared |
| `work_outstanding` (new) | a Play, Locate, Stop, Record start, record-off, release or render is outstanding, or a command is held |
| `exclusive_ok` (new) | `Stopped`, not `work_outstanding`, not `finalizing`, not `faulted`: safe to start a measurement, a render, a sync mode change |

Host-only predicates: `running()` (not Stopped, not StopPending Park or Reset: a pending start counts as running, ADR 0043 amendment), `transport_active()` (not Stopped), `sync_allowed()` (no Play, Locate, Record start or record-off outstanding; Stop and release do not block, exactly `frame_logic.rs:365`).

Every reader is assigned to exactly one field or predicate in [tables.md](0062/tables.md), section C, including the judge's list. `recording_active()` is deleted. Readers that change what they block are tagged B12. Backend-local readers (`crates/mix/src/strip.rs:418`, `P/player/audio.rs:775, 871`, `P/player/device.rs:265, 966`, the measurement and render services) keep reading the audio side. `TransportStatus` derives `Serialize` (`E/lib.rs:105`), so the new fields appear in `transport.status` (`E/cmd/transport.rs:95`) and `session.inspect`'s `transport` (`E/inspect.rs:100`); `ui.inspect` (`lib.rs:664-665`) adds `record_engaged` explicitly. `docs/control-protocol.md` is updated in S2b, keeping its `recording_pass` contract (`:122-123`). No session file changes.

### 8. Positions and synchronous effects (R-C)

Within a step the host keeps a **working cursor** (initialized from `StepInput.cursor`) and the working positions; every command sees the effects of the commands before it in the same step.

| Effect | Side | Today |
|---|---|---|
| cursor move by a locate or a take-locate | synchronous (working cursor; `CursorTo` at step end for the interface's copy) | `set_cursor` before the next request, `host/transport.rs:145, 268` |
| transport position, retained position | synchronous, published at once | `host/transport.rs:52, 66, 146, 223, 260, 270` |
| a stopped cursor move from the interface | synchronous: `set_cursor` calls `host.cursor_moved(at)`, which publishes the position at once while Stopped | `lib.rs:537-539` |
| selection after a placed take | synchronous: the host runs `edit.select` itself | `host/recording.rs:640, 712` |
| Return to Zero on a second Stop | synchronous: `engine.execute("transport.rtz")` and its Locate handled before later requests | `host/transport.rs:238-241` |
| undo squash of a Record operation | synchronous | `host/recording.rs:466-478` |
| ResetSession's cursor, cursor track and focus clear | cursor synchronous (working cursor := session start); the rest deferred | `host/transport.rs:199-201` |
| scroll back to pickups | deferred (`PickupsBefore`) | `host/recording.rs:493-503` |
| focus track, take-lane visibility | deferred (`TakePlaced { focus, take_lanes }`) | `host/recording.rs:631, 642, 653, 714` |
| gestures, copies, automation selection, meter displays on reset | deferred (`SessionReset`) | `host/transport.rs:191-198, 202-204` |
| Record hint | deferred (`RecordHint`), stamped by the interface's frame clock | `host/recording.rs:27`, `midi_transport.rs:290` |
| editor windows re-wanted after a replacement; plug-in editor edits applied to automation, Learn and merges | deferred (`EditorsReopened`, `EditorEdits`) | `host/reconnect.rs:231-235`; `host/plugin_state.rs:193-214` |

Retained position: written on Park completion from the applied stop position read before parking (`host.rs:96-97`), by PlayExact and Locate (the target), by ResetSession (0); read by Continue.

### 9. The step

**Clock (R-F).** `trait HostClock { fn frame(&self) -> f64; fn monotonic(&self) -> Duration; fn calendar(&self) -> SystemTime; fn sleep(&self, d: Duration); }`. The host never calls the system clock. The interface implements it from egui's frame time and the system clocks; tests and the headless runner pass a stepped clock.

| Timed thing | Clock |
|---|---|
| `dt` of the simulation and the simulated count-in; meter poll every 50 ms (`host/transport.rs:324`) | frame |
| reopen attempts (`host/reconnect.rs:34, 86, 151`); time while the window is hidden does not count, as today | frame |
| stream verification after a reopen (`audio_devices.rs:299-320, 340`) | frame |
| Record hint lifetime (stamp and expiry, `lib.rs:653`) | frame, both on the interface side |
| timed state read and autosave every two minutes (`host/timed_read.rs:42, 47, 57`) | monotonic |
| MIDI journal flush interval (`E/midi_journal.rs:134, 140, 212`) | monotonic, passed into `Writer` (an engine signature change in S3b) |
| MIDI journal file name (`E/midi_journal.rs:114`) | calendar, passed into `journal_path` (S3b) |
| plan wait before captures (`host/state_guard.rs:215-241`) | monotonic and `sleep` |
| hardware measurement deadlines (`hardware_measure.rs:21, 47`) when stepped (S5a) | monotonic |
| exempt, inside a blocking Player service: the state read budget (`P/player/handles.rs:897-913`), the save deadline (`:31`) | the Player's own |

**Input.** `StepInput { dt, cursor, prefs: HostPrefs, pointer_down, touches_open, ingress_cancel_generation }`; `HostPrefs` copies count-in bars and switch, click level, monitor settings, audio configuration, low-latency limit and MIDI transport profiles. The timed read's gate is `pointer_down || touches_open || engine.merge_held()` (`host/timed_read.rs:75-76`).

**Order.** Today's `logic()` order is kept; only the MIDI buttons move.

| # | Block | Today |
|---|---|---|
| H1 | receipts in `poll_notices` order, then the stop and release frontiers; Park completion reads the applied position first | `frame_logic.rs:329`, `host.rs:82-114` |
| H2 | recording clock observation and journal; lap limit | `:335-337` |
| H3 | output trouble, input trouble, reopen | `:355-357` |
| H4 | click and monitor pushes; the Resume check (Reset supersedes it, below); observations: start failure, record stage, latch | `host/transport.rs:139-167` |
| H5 | queued commands, first in first out | `:168-308` |
| H6 | MIDI buttons in arrival order, each decided against the current phase and applied at once; MIDI loss | drawing pass today (`frame_logic.rs:467`); tagged D7 |
| H7 | loop retarget; readback, natural end; simulation timers; meters | `host/transport.rs:309-386` |
| after | automation write pass; session sync when `sync_allowed()`; autosave; input | `frame_logic.rs:364-375` |

- **Held and deferred commands.** A start or Record during StopPending Park is held and applied after completion (D4); during StopPending Resume, commands other than Stop, Pause, Record and ResetSession are deferred until the resumed pass starts or fails. A Stop, Pause or ResetSession drops every held start queued before it.
- **Reset supersedes Resume.** Before H4 starts a resumed pass it scans the queued commands; a ResetSession, Stop, Pause or Record there turns Resume into Park (Reset for ResetSession). So a session opened during a take-locate never resumes recording (ADR 0043: "Opening a session always starts stopped").
- **Resume uses the full start path.** It runs the Record start with all its refusals; a refusal leaves Stopped with the finished take preserved and the reason visible (ADR 0043:143-145), then the deferred commands run against Stopped.
- **Ingress cancellation.** The host drops any held or deferred start whose source generation is not newer than `ingress_cancel_generation`; the generation itself is ADR 0043's (section 15).
- **Device-free step (R-I).** S2b extracts the transport part of `logic()` into `StudioApp::step_host(&mut self, clock, input)` with no egui argument; `logic()` calls it, and `host::testing::device_free_app()` (test only) builds a `StudioApp` on a device-free Player and a stepped clock. This is the window-free adapter sync slice 1's gate uses before the crate move.

**Events out** (`HostEvent`, in transition order): `PhaseChanged`, `Message`, `CursorTo`, `SessionReset`, `TakePlaced`, `PickupsBefore`, `RecordHint`, `EditorsReopened`, `EditorEdits`. Outcome text is written by the executor into `StatusLine` (section 6); the event records it.

**The pure core.** `next(phase, event, facts) -> Next { to, effects }` takes no strings and no Player; the executor performs the effects and feeds `Sent` or `NotSent` back in the same step.

### 10. Behavior changes

"Read" = confirmed by reading `main` 1a56b895; round 1 confirmed each by reading too. Every test runs its negative control on the commit before its slice.

| # | Today | Instead | Affected | Slice, test |
|---|---|---|---|---|
| B1 | Count-in admission and a start waiting for the clock answer `Applied` (`P/player/audio.rs:299, 340, 412`); any applied Record receipt sets the flag (`host.rs:349-355`) and the wait shows "Recording" | the flag only from the latch; the wait shows "Record pending" | `transport.status`, `session.inspect`, `ui.inspect` `recording`; wait text | S2b `recording_flag_is_false_through_the_count_in_and_true_from_the_latch`, `an_admitted_record_without_a_latch_is_waiting_not_recording` |
| B2 | With a Player the flag stays until record-off or completion (`host/recording.rs:375-381`, `host/transport.rs:63`), so `recording_active()` (`host/recording.rs:281`) gates device changes, undo, recovery and the timed autosave while a Park or Reset stop is pending | StopPending Park and Reset are not `record_engaged`; Resume stays engaged | readers on `record_engaged` (section 7) | S2b `stop_ends_the_pass_at_once_and_device_changes_are_allowed_while_the_stop_is_pending` |
| B3 | "Count-in" is written on sending (`host/recording.rs:149-154`) and replaced by "Recording" when `count_in_state()` reads `None` (`host/transport.rs:165-166`), even before admission | CountIn leaves only on the record stage | status text | S2c `count_in_text_stays_until_the_record_stage_moves_on`, `a_step_that_misses_the_whole_count_in_sees_waiting_or_recording` |
| B4 | Only the displayed promotion is gated by text (`host.rs:343, 354, 380`; `host/transport.rs:59-60, 165-166`); requests and the flag change regardless (`host.rs:342, 348, 351-353, 388`) | section 6 | status line | S2c, the six display tests |
| B5 | `reset()` keeps the text (`host.rs:280-282`); Reconnect while playing sends Stop (`host/reconnect.rs:227-228`), resets (`:245`) and writes nothing when capture succeeded (`:107-112`); `is_state` keeps "Stop pending" | replacement and reopen enter Stopped; text derived | status line | S2c `reconnect_while_playing_leaves_no_pending_text` |
| B6 | A Record from stopped stores its sequence as `play` (`host/recording.rs:150-152`); Play pulses "Play pending" (`transport.rs:120, 127`) | `play_pending()` is StartPending only; the slow and stuck plug-in override stays (`:125-126`) | Play pulse, tooltip, click guard | S2c `a_record_start_does_not_pulse_play` |
| B7 | Speed set before a fallible start (`host/transport.rs:289-292`, returns at `:12-21`, receipt `host.rs:362-366`) outlives a refused start; the simulation ignores speed (`host/transport.rs:361`) | the speed invariant; the simulation advances by `dt` times speed. The refusal of Record while playing at half speed stays (pinned by `host/live_cycle_tests.rs:366`) | Record after a refused half-speed start; `--no-audio` position | S2d `a_refused_half_speed_start_leaves_normal_speed_and_record_works`, `the_simulated_transport_plays_at_half_speed` |
| B8 | A take-locate returns at `host/transport.rs:139-142`, skipping the pushes, readback and natural end | Resume is a phase; the block runs. Fresh meters stay suppressed while a Stop is pending (`:324`, unchanged) | playhead, click level, monitor during a take-locate | S2d `a_take_locate_keeps_reading_back_position_and_pushing_the_click_level` |
| B9 | `is_playing()` reads the atomic (`lib.rs:637`); the copy (`host/transport.rs:312-317`) is read by `lib.rs:537`, `host.rs:47`, `host/reconnect.rs:29, 138`; `midi_monitor.rs:674` reads the Player directly | one playing and position pair per step; backend admission checks stay on the audio side | section 7's `playing` readers | S2d `every_reader_sees_the_playing_value_of_the_last_step`, `the_punch_decision_uses_the_step_sample` |
| B10 | The count-in plan outlives the count-in (`host/recording.rs:154`, `host/transport.rs:58`); displays also check the counter (`metronome.rs:50-51, 61`) | the plan lives in phase data | none visible | S2c `the_count_in_plan_leaves_with_the_pass` |
| B11 | Three positions (`midi_transport.rs:59`, `engine.transport.position`, `ui.cursor`; `host/transport.rs:52, 66`; `lib.rs:537-539`) | one owner (section 8); no timing change: a stopped cursor move still publishes at once | none visible | S2d `a_stopped_cursor_move_publishes_the_position_at_once`, `locate_then_play_in_one_step_plays_from_the_locate` |
| B12 | Raw-flag readers do not block during RecordPending before admission (`actions.rs:11, 15`, `menus.rs:18, 402`, `E/lib.rs:770`); measurement admission checks only playing (`audio_devices.rs:481`, `hardware_ui.rs:105`, `hardware_measure.rs:187`, `external_measure.rs:227`, `finish/realtime.rs:139`, `E/lib.rs:414`, `E/cmd/external_instrument.rs:45`, `E/cmd/realtime_render.rs:115`) | `record_engaged` blocks from RecordPending on; exclusive operations need `exclusive_ok` (also refused during a pending start or stop, a finalizing take, an outstanding locate or release) | undo, width, replace, Reconnect, measurements, renders | S2b `undo_is_refused_from_the_record_request_on`, `a_measurement_is_refused_while_a_start_is_pending` |

**Related defects** (same code path; proposed task lines on #739, fixed in S2d):

| # | Today | Instead | Test |
|---|---|---|---|
| D1 | Play or Toggle while a start waits for the clock from stopped reaches `play_from_cursor` (`host/transport.rs:218-219, 246-249`); its Play calls `end_count_in` (`P/player/audio.rs:452`), cancelling the waiting ticket (`:417-418`) while the host keeps the plan | Play ignored; Toggle cancels like Stop | `play_while_waiting_for_the_clock_does_not_drop_the_record_start` |
| D2 | PlayExact always calls `start_play` (`host/transport.rs:222-224`); PlaySelection cancels an observed count-in through Stop (`:273-275`) but otherwise starts Play (`:278-279`), during RecordPending before admission, WaitingForClock and Recording | ignored during RecordPending, WaitingForClock and Recording; PlayExact during CountIn cancels like Play | `play_from_a_position_during_a_pass_leaves_the_pass_alone` |
| D3 | ResetSession during a take-locate waits behind the early return (`host/transport.rs:139-142`), so `start_recording` is attempted on the new document (`:148`) and refused for want of an armed track (`host/recording.rs:20-28`) | Reset supersedes Resume | `a_session_replaced_during_a_take_locate_does_not_attempt_a_resume` |
| D4 | During Stop pending, starts and Record depend on the observations of the moment (`host/transport.rs:218, 246-249, 253-256`) and a Play can be sent with the Stop outstanding (`:21-23`) | held until the stop completes, then applied | `a_start_during_stop_pending_is_held_and_applied_when_stopped` |
| D5 | Locate during Stop pending sends a Player Locate while the Player still plays (`host/transport.rs:268-270`, `:85-92`), and a loop change sends `set_loop` (`:128-133`) | cursor and retained position move; no Player request to a stopping Player (the stop parks at the cursor, the same end state) | `locate_during_stop_pending_parks_at_the_target` |
| D6 | Pause while stopped sends a Stop (`host/transport.rs:244`) | no effect | `pause_while_stopped_does_nothing` |
| D7 | MIDI buttons apply after the write pass, session sync and autosave, in the drawing pass (`frame_logic.rs:363-372, 467`) | applied in H6, before them | full `logic()` tests `a_midi_start_is_seen_by_the_same_frames_write_pass`, `midi_stop_ordering_survives_the_move` |
| D8 | ResetSession during a real-time render sends Stop under the running render | the render is cancelled first, as a reopen does (`finish/realtime.rs:26`) | `a_session_replaced_during_a_render_cancels_it_first` |

### 11. The crate boundary and tests

| Moves to `transit-host` | Slice |
|---|---|
| `host/phase.rs` (phase, facts, `next`, `state_text`, capabilities), `PlayerRequests` and receipts, the pure texts, `midi_transport::decode` and the button table, the count-in arithmetic | S3a |
| `Host` with the Player and recorder; `host/transport.rs`, `host/recording.rs` (view effects as events), `host/autosave.rs`, `host/reconnect.rs`, `host/state_guard.rs`, `host/timed_read.rs`, `host/plugin_state.rs` minus `apply_edits` (an `EditorEdits` event, `:193-214`), the non-drawing parts of `midi_transport.rs`, `metronome.rs`, `transport.rs`; `StatusLine`; the render transport; `Connection for Player` (`audio_connection.rs:62-86`); Player installation (`splash.rs:129-136`) | S3b |
| the four facades and their files (section 2) | S3c to S3e |

Stays in the interface: view state, `StatusAge` and drawing, meter ballistics (`host.rs:146`), `run` and its interlocks, window features, preferences.

**Tests.** No window-free test is required to move. In S3b, test files that reach into moved fields change paths (`app.player` to the host's test accessors, `app.player_requests` to `app.host.requests_mut()`, and `take_plan`, `midi_transport`, `sim_count_in`, `record_start`, `take_cycle`, `recorder` likewise); tests that assign moved fields use test setters (`host::testing`), and time fields convert to the stepped clock. The reviewer checks the test diff holds only those kinds of edits. Tests needing a window stay; window-free ones may be ported to `Host` after S4.

### 12. Blocking calls

| # | Call | Target | Slice |
|---|---|---|---|
| 1 | device reopen (`audio_devices.rs:527`, Apply `:509`, `ui.set {audio}` `frame_logic.rs:291`, Reconnect `host/reconnect.rs:104`, automatic `:173`, a needed reopen in `host/plugin_state.rs:40-41`) | staged with `ReopenPending`: the reopen runs only in a pass after one that drew the text, the splash's precedent (`splash.rs:115-124`); a held Play is cancelled by Stop. The open and `preserve_audio_state` hold the Player: out of scope | S5b |
| 1a | ui.json write after verification (`audio_devices.rs:325-356`) | worker | S5a |
| 2 | `io.measure_hardware_latency` sleep loop up to 8 s (`hardware_measure.rs:86-94, 21`) | stepped (`start_measure` `:98`, `step_measure` `:136`); the control request is parked and answered when done | S5a |
| 3 | `wait_for_plan`, up to 150 ms (`host/state_guard.rs:215-241`) | out of scope, bounded; uses the clock capability | none |
| 4 | plug-in state capture (`host/plugin_state.rs:105`; 500 ms, `P/player/handles.rs:31`) | out of scope; reopen with an ADR 0018 amendment | none |
| 5 | `Autosaver::flush` (`E/autosave.rs:83-86`) via `autosave_to` | timed write by ticket; panic path, `close_autosave`, `recover` keep the flush | S5a |
| 6 | saves inside `engine.execute` | out of scope | none |
| 7 | input open (`host.rs:118-141`) | staged with `InputOpening`; the open holds the Player: out of scope | S5b |
| 8 | `machine_listing` (`audio_devices.rs:716`) | worker, as the ASIO listing (`:731`) | S5a |
| 9 | quit, up to 10 s (`host.rs:75-78`) | out of scope | none |
| 10 | file pickers | #39 | none |

Reopen condition for the out-of-scope parts: a record that lets a Player be created or dropped on a worker.

### 13. Headless (S4, R-J)

- `Headless` holds a `Host`; it stops clearing `transport_requests` (`crates/automation/src/lib.rs:53-54`).
- **Stepping.** `mcp::Server::serve` blocks on `input.lines()` (`crates/automation/src/mcp.rs:239-241`). S4 moves line reading to a reader thread that sends lines over a channel; the thread that created the Player loops on `recv_timeout(10 ms)`: a line is handled, a timeout steps the host. So natural ends, journal progress and autosave proceed while the client is idle. The loop owns a monotonic clock implementing `HostClock`; tests inject a stepped one.
- **What it can do.** Production Headless opens `Player::with_null_device`: real-time paced playback, no input, no MIDI ports. So headless supports playback, every transport command, offline work and inspection. It cannot record: Record refuses with the existing "no audio input" and "no audio device is open to record MIDI" paths. Recording headless would need a virtual MIDI input on the null Player (the `transit-midi-io` `fake` backend made a production option) or a null audio input; that is a later record.
- **Build.** `transit-automation`, and `transit-cli` through it, link `transit-playback`: cpal, the native backends on Windows, ALSA development files on Linux (`libasound2-dev`, already a gate prerequisite); `native-jack` is forwarded as an optional feature. `transit-engine` alone still builds without them. `Backend` has no `Send` bound (`crates/automation/src/lib.rs:22`).
- Open point: none; the parked replies of S5a answer pending control requests.

### 14. The guard

- **Final (S3f):** the layer rule (`LAYERS`) and one forbidden normal edge in `check_layers`: `transit-ui-studio` may not depend on `transit-playback` (dev dependencies exempt, as `:208-209` already skips them). With the edge gone the interface cannot name the Player; a fixture test in `xtask/src/tests.rs` asserts the edge fails.
- **Transitional lint (S3a to S3e):** `xtask/src/host_guard.rs` counts, per non-test interface file, calls of `Host::legacy_player()` (the temporary accessor S3b adds) and textual Player transport calls, on a ratchet table like `file_size::OVER_BUDGET` (`xtask/src/file_size.rs:11-38`). It is a review aid, not ownership: it misses aliases, paths through traits and multiline types and has known false positives (`hardware_measure.rs:276`, `keys.rs:915`). Each facade slice removes its rows; S3f deletes the lint and the accessor.

### 15. Relation to ADR 0043

| Requirement | Here |
|---|---|
| 1. sync mode changed only stopped and finalized | `exclusive_ok`, published by S2b; the mode is ADR 0043's |
| 2. Start/Continue/Stop/SPP with its own start pending | `StartPending`, `next()`, the matrix (S2a, S2b). An external start waiting for its first clock tick has no Player sequence: its shape is `TransportPhase::StartArmed { source: SourceGeneration, from: Samples, record: bool }`, entered on the source's Start or Continue, left for StartPending when the start is submitted, for Stopped on Stop, a device event, faulted or ResetSession; `record_engaged` when `record`. ADR 0043 adds it; S2a's reducer has no assumption that every pending state holds a sequence |
| 3. duplicate suppression against a pending locate in the epoch | ADR 0043's; S1's `Receipt::Locate { seq, target, outcome }` keeps the target; ADR 0043 adds the applied epoch (`P/player/audio.rs:361, 411`) and the source generation |
| 4. record engaged, "Waiting for external start" | ADR 0043's, as `StartArmed { record: true }` with the state text from `state_text()`; B1 is its precondition |
| 5. sticky local Stop | ADR 0043's mechanism at ingress admission (ADR 0043:534-537). The host guarantees it can honor it: a Stop is never held behind a start; a Stop drops held starts queued before it; held and deferred starts older than `ingress_cancel_generation` are dropped |
| 6. half speed and scrub refused in clock follow | ADR 0043's, as a condition on the HalfSpeed and Scrub rows; a refusal leaves phase, speed and position unchanged |
| 7. readout through `TransportStatus` | ADR 0043's; S2b adds four fields to the same struct |
| 8. applied transport moments for sending | ADR 0043's callback-published spans with generations and converter phase (ADR 0043:319); `PhaseChanged` is a control-thread observation and is not used for sending |
| 9. slice 3 refusals, tempo, journal segments | unchanged; ADR 0043's |
| 10. deterministic matrix without a window | the pure matrix test (S2a) and the device-free step (S2b), both before the crate move and before sync slice 1 |

**What sync slice 1 extends:** `TransportPhase` (`StartArmed`), `TransportEvent` (a selected-source message with source generation and sequence), `next()` and its matrix test, `state_text()`, `TransportStatus`, `StepInput.ingress_cancel_generation`. Before S3a in `crates/ui-studio/src/host/phase.rs`, after in `crates/host/src/phase.rs`.

**Stale statements.** No ADR 0043 decision becomes false. The path statements "`crates/ui-studio/src/host/transport.rs` (exact-position starts and independent retained position)" (Implementation) and "Slice 0 must adapt `crates/ui-studio/src/host/transport.rs`" (2026-11-04 amendment) need a pointer amendment at S3b. Its Context sentence on `TransportStatus` describes its base and stays.

### 16. Relation to ADR 0052

ADR 0052's count-in and latch statements rely on the audio thread (decision 8 and the 2026-11-16 amendment: "The count-in bar keeps its full length in output frames" and the take starts where the record start is heard), on `count_in_for` (decision 3), on `plan_recording` writing `count_in` (decision 8) and on `recording_pass` (decision 12). None changes: the stage publication of section 5 only reports what the audio thread does and never schedules it. Path statements become stale at S3a and S3b (decisions 3, 4, 5, 7, 8) and need a pointer amendment.

## Slices

Every commit builds; every slice runs the full local Windows and Linux gates and is shippable alone. A writer of a meaning and every safety reader of that meaning change in the same slice (R-G). "Unchanged" = the test file is not edited.

**S1, derive the phase (no behavior change).** `host/phase.rs` with the types and a read-only `transport_phase()` derived from today's data plus metadata S1 adds to `PlayerRequests`: `play_from`, `play_prev`, the record request's `from`, count-in and ticket, and a `recording_kind: Start | Off` beside the shared `recording` sequence, so a completed Stop still awaiting record-off derives as StopPending Resume with `stop: None`. `notice` and `recovery` split into typed `classify` and `frontiers` behind wrappers with today's exact text and flag rules. Files: `host.rs`, `host/phase.rs`, `host/phase_tests.rs`, `host/recording.rs` and `host/transport.rs` (metadata writes only). About 200 lines and 250 of tests. Unchanged: every existing test. New: six derived-phase tests (Play to Playing, Stop to Stopped, Record without and with count-in, take-locate as Resume including the record-off wait, the simulated count-in) and `state_text_names_the_waits`. Feel-right: none.

**S2a, the pure reducer (unused).** `next()`, `TransportEvent` with full payloads, facts, effects; [tables.md](0062/tables.md) section A as a literal table; `transport_phase_matrix` (every phase and event), `the_matrix_has_no_empty_cell`, `recording_implies_record_engaged`. Files: `host/phase.rs`, `host/phase_tests.rs`. About 450 lines and 400 of tests. Unchanged: every existing test. Feel-right: none.

**S2b, the authoritative lifecycle.** The phase drives every start, stop, record and render call (render orchestration moves from `finish/realtime.rs` into the host executor); cleanup (`finish_recording`, record-off, faulted); `record_engaged`, `finalizing`, `work_outstanding`, `exclusive_ok` and every safety reader of section 7 with them, engine guards included; published after every transition; B1, B2, B12; the device-free step and `host::testing`. Files: `host.rs`, `host/*.rs`, `finish/realtime.rs`, `lib.rs`, `actions.rs`, `menus.rs`, `frame_logic.rs`, `audio_devices.rs`, `audio_devices/window.rs`, `hardware_measure.rs`, `hardware_ui.rs`, `external_measure.rs`, `midi_transport.rs`, `E/lib.rs`, `E/cmd/external_instrument.rs`, `E/cmd/realtime_render.rs`, `docs/control-protocol.md`. About 600 lines and 500 of tests. Edited tests: `player_tests.rs` to `classify` and `frontiers` (same cases); the 23 sites in 10 interface test files and `E/tests/recording.rs:8`, the `realtime_render.rs` test (`:211`) that set the recording flag as a stand-in for a pass now set the phase or `record_engaged`; `E/tests/session_transport.rs:9` gains `..Default::default()`; `midi_transport/tests.rs:138` sets the simulated count-in through `host::testing`; `state_guard_round3_tests.rs:31-34` sets a pending Play through `host::testing`. Unchanged: the rest of S1's list, `live_cycle_tests.rs` included. New: the B1, B2, B12 tests; render ownership (`the_phase_is_rendering_while_a_render_plays`, `the_phase_never_reads_stopped_while_the_render_stops`); Resume with audio and MIDI, deferred commands and an intervening edit (undo squash, `host/recording.rs:471-477`); journal handoff interrupted at each step (`host/autosave.rs:72-83`); host steps that miss a whole count-in. Feel-right: at Stop after a take the Record light goes off at once.

**S2c, display and count-in.** `StatusLine`, revisions and arbitration; the 333 writers to `status.set`; `state_text` with reopen, input and release; the record-stage publication in playback (section 5); B3 to B6, B10. Files: `status.rs`, `lib.rs`, `transport.rs`, `metronome.rs`, `menus.rs`, the 333 writer sites (mechanical), `host/*.rs`, `P/player/audio.rs`, `P/player/device.rs`. About 400 lines plus the mechanical change, 400 of tests. Edited tests: `status.rs` `transport_waits_stay_until_the_transport_code_clears_them` (`:147`) becomes `a_state_text_stays_while_its_phase_holds_and_leaves_with_it`; state-text assertions read the shown line in `slice6_tests.rs` (`:404, :422, :455, :462-463`); `transport_tests.rs:317` and `plugin_retry_tests.rs:330` set StartPending through `host::testing`. Unchanged: the playback count-in tests (`P/player/audio/tests/count_in.rs:63, 118, 161`) and `a_pickup_on_a_count_in_click_lands_on_that_click_at_a_converted_rate_too`. New: the six display sequences, B3 to B6 and B10 tests, `the_record_stage_follows_count_in_wait_and_start` (playback). Feel-right: Record from stopped with a one-bar count-in: Play does not pulse; Count-in, then Recording; Stop pending, then Stopped; Reconnect while playing leaves no Stop pending.

**S2d, speed, positions, ordering.** The speed invariant, working cursor and positions, held and deferred commands, Reset over Resume, H6; B7 to B9, B11, D1 to D8. Files: `host/*.rs`, `lib.rs`, `midi_transport.rs`, `frame_logic.rs`, `midi_monitor.rs`, `browser/preview.rs`, `automation/pass.rs` (reads the step's pair). About 400 lines and 500 of tests. Unchanged: S2c's list; `live_cycle_tests.rs` (the half-speed refusal at `:366` and the stalled-frame and later-lap tests at `:304, :326, :342`), `midi_transport/tests.rs` (`:211`, `:257`). New: B7 to B9, B11 and D tests; full-`logic()` ordering tests for H6. Any other moved expectation stops the slice for a ruling. Feel-right: Space twice quickly (stop, then play) restarts playback once the stop completes.

**S3a, the pure machine into `transit-host`.** Crate, `LAYERS`, S3a row moved, the transitional lint. About 1,000 lines moved, 250 new. All S2d tests pass; the pure tests move unchanged.

**S3b, the Player into `Host`.** The S3b row of section 11, `HostClock` and `StepInput` in the crate, the journal clock parameters (`E/midi_journal.rs`), `Host::legacy_player()` for the facade slices, the pointer amendments of ADR 0043 and ADR 0052. About 3,000 lines moved. Tests: section 11's mechanical edits only. Open point: split transport and recording from autosave, reconnect and state if one sitting cannot hold it.

**S3c, `PluginServices`; S3d, `Readouts` and `LiveInput`; S3e, `DeviceServices`.** Each moves its files of section 2 off `legacy_player()` and removes their lint rows; 10 to 15 files and 300 to 600 lines each; their existing tests pass with path edits only.

**S3f, the edge.** `transit_host::data` re-exports; `transit-ui-studio` drops its normal `transit-playback` dependency; the forbidden-edge rule; `legacy_player()` and the lint deleted. Small.

**S4, headless.** Section 13. New: headless `transport.play` reaches Playing while the client is idle (stepped clock); a natural end finalizes with no client line; Record refuses with the no-input reason; the matrix's command rows through `Headless`.

**S5a, S5b.** Section 12. S5a gate: the parked measurement reply arrives after stepping with a fake probe; the timed autosave never calls `flush`. S5b gate: the reopen runs only after a pass that drew `ReopenPending` (fake connection); a held Play starts after the reopen and is cancelled by Stop. That a desktop frame was presented is the feel-right check: Apply shows "Reopening the audio device" before the window waits.

Order: S1, S2a, S2b, S2c, S2d (before sync slice 1); S3a to S3f, S4, S5a, S5b in order beside sync.

## Consequences

- One value answers what the transport is doing, a real-time render included; status strings are display; a stray message can neither strand nor hide a state.
- `recording` means capture; the engine's guards read named capabilities, and exclusive operations wait for a fully settled transport (B12).
- The interface cannot name the Player after S3f; window code reaches it only through four facades.
- Headless plays and steps itself; it does not record; `transit-cli` links the audio backends.
- The host still runs on the interface thread; a slow driver open still holds the window, now announced first.
- About 4,000 lines move; ADR 0043 and ADR 0052 need pointer amendments at S3b.

## Alternatives rejected

- **(i) Type the phase in place and never move**: nothing headless, the window types stay beside the machine, nothing to guard.
- **(ii) The host in `transit-engine`**: every engine user would link the device backends and plug-in hosts, and the document crate would gain the Player's thread rule.
- **(iii) A host actor on its own thread**: the Player is neither Send nor Sync by design.
- **(iv) The host in `transit-playback`**: the device crate would depend on the document crate.
- **(v) Strings as constants**: the text would still be the state.
- **(vi) Phases per pass kind**: punch, cycle and pre-roll are plan data.
- **(vii) A regex guard as the boundary** (round 1's draft): bypassed by aliases, traits and the render; kept only as a transitional lint.
- **(viii) The render as an owned operation beside the phase**: the phase could then read Stopped while the render plays.
- **(ix) Inferring the count-in end from the published counter or elapsed time**: misses whole count-ins (round 1, finding 9.5).

## Choices made in this record

1. Variants and data (section 3), `Prev`, the two acknowledgements of StopPending, `Rendering` as a phase.
2. WaitingForClock shows "Record pending"; InputOpening "Opening the audio input"; ReopenPending "Reopening the audio device".
3. Arbitration: a Note keeps priority across a state change; an Outcome never does.
4. Capability names and the reader assignment.
5. Held starts during Stop pending (D4), Pause while stopped a no-op (D6).
6. MIDI buttons in H6 (D7) rather than after autosave.
7. The record-stage atomic, 56-bit ticket plus code.
8. Facades in four groups, removal of the playback edge, data re-exports.
9. Headless steps on a 10 ms receive timeout; no headless recording.

## Judging round 1: responses

Round 1 (verdict REJECT) findings, numbered by its sections. Each was checked against `main` 1a56b895 before acceptance.

| Finding | Disposition |
|---|---|
| 1. B1, B3, B5, B7, B9, B10, D1 confirmed | Kept. |
| 1. B2: restrict to Park and Reset | Accepted: section 10 B2; Resume stays `record_engaged`. |
| 1. B4 confirmed as restated | Kept with its corrected citations. |
| 1. B6: the plug-in loading override | Accepted: kept (`transport.rs:125-126`), section 10. |
| 1. B8: meters suppressed while Stop pending | Accepted: suppression unchanged (`host/transport.rs:324`); the test asserts readback and pushes, not fresh meters. |
| 1. B11: delayed publication is a change | Accepted: no delay; `cursor_moved` publishes at once (section 8). |
| 1. D2 wrong as generalized | Accepted: PlaySelection's count-in cancel is today's (`host/transport.rs:273-275`); D2 restated. |
| 1. D3 an attempted resume | Accepted: restated. |
| 2. Render drives transport outside the owner | Accepted: section 3 sweep, `Rendering`, S2b. |
| 2. Replacement Play needs the previous state | Accepted: `StartPending.prev`; a refused Play returns to it (tables A). |
| 2. Stop and Play coexist | Accepted: starts held during Stop pending (D4), so they no longer coexist. |
| 2. S1 cannot derive the data | Accepted: S1 adds the metadata; StopPending holds optional acknowledgements. |
| 2. Fault must persist | Accepted: `faulted` fact (sections 3, 5). |
| 2. Missing rows (record-off, Locate, release, session, untracked, punch refusal, Resume results, NotSent cleanup, loop change, MIDI loss, simulation) | Accepted: all in tables A and B. |
| 2. "Stale receipts dropped as today" incorrect | Accepted: section 5, untracked refusals post. |
| 2. Untagged Stop-pending cells, output trouble in StartPending, CountIn light, half-speed Record refusal | Accepted: D4 to D6; output trouble in StartPending matches today (message only); the light follows the counter as today; the refusal is a row (tables A). |
| 2. Deferred cursor breaks Locate then Play | Accepted: working cursor (section 8), test in S2d. |
| 3. `!(playing \|\| pass)` does not implement `stopped()` | Accepted: explicit capabilities (section 7). |
| 3. Engaged vs finalizing vs observed vs outstanding | Accepted: four fields; cleanup reads `finalizing`. |
| 3. Missing readers | Accepted: all in tables C. |
| 3. RecordPending newly blocked | Accepted: B12. |
| 3. `session.inspect`, `ui.inspect`, protocol page | Accepted: section 7. |
| 3. Step-end publication stale for command guards | Accepted: published after every transition. |
| 4. Display rule unsound (six sequences) | Accepted: section 6; tables D; S2c tests. |
| 5. Order differs from today | Accepted: section 9 keeps today's blocks; only H6 moves (D7); Park reads the applied position first. |
| 5. Session sync gate excludes Stop and release | Accepted: `sync_allowed()`. |
| 5. Hidden clocks | Accepted: `HostClock` and the clock table; the Player's budgets exempt. |
| 6. Layer placement valid | Kept. |
| 6. Regex guard not ownership | Accepted: facade and forbidden edge; the lint transitional (section 14). |
| 6. Splash installs the Player | Accepted: moves in S3b. |
| 6. `apply_edits` not pure | Accepted: `EditorEdits` event. |
| 6. Gesture gate uses touches | Accepted: `touches_open` in the input. |
| 6. Accessors cannot replace setters | Accepted: `host::testing` setters. |
| 6. Build prerequisites; stepping; null Player capture | Accepted: section 13. |
| 7. S1 metadata | Accepted. |
| 7. S2 split and unsafe intermediate commits | Accepted: S2a to S2d, writers and readers together. |
| 7. Four tests | Accepted: `session_transport.rs:9`, `midi_transport/tests.rs:138`, `state_guard_round3_tests.rs:31-34` edited in S2b; `live_cycle_tests.rs:366` unchanged. |
| 7. Integration coverage | Accepted: S2b to S2d gates. |
| 7. S3 time conversions, S5 presentation | Accepted: section 11, S5b. |
| 8. Mappings 1 to 10 and the three accepted ADR 0043 statements | Accepted: section 15 rewritten; Resume full start path, ingress guarantee, Reset over Resume (section 9). |
| 9. Rulings 1 to 8 | Accepted, each through the rows above. |
| 10. Automation merge, preview, repaint, splash, input-loss messages, `edit.select` | Accepted: tables C (write pass, preview, repaint on `work_outstanding`), message-only input-loss rows (tables A), section 8 (`edit.select` synchronous). |

No finding was rejected.

## Judging

## Related

[ADR 0043](0043-hardware-sync-transport-and-clock.md), [ADR 0052](0052-pre-count-zone.md), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md), [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md), [ADR 0018](0018-plugin-host-thread.md), [ADR 0026](0026-cycle-recording-stacks-takes.md), [ADR 0027](0027-external-instrument-real-time-render.md). Issues #739, #733, #736, #39.

<!-- transit-index
title: The session host: transport and recording orchestration move out of the interface crate into `transit-host` (layer 5), with one typed transport phase as the only truth (a real-time render included), transport commands admitted synchronously, explicit capabilities for every safety reader, status text arbitrated per transition, the record start's stage published through its ticket, time as a clock capability, a restricted service facade so the interface can no longer name the Player, headless playback, and blocking work staged or moved off the interface thread
-->
# ADR 0062: The session host: one typed transport phase, outside the interface

- **Status:** proposed 2026-11-16 (revised after judging rounds 1 and 2, same day)
- **Date:** 2026-11-16
- **Implementation:** not started. Slices S1, S2a to S2e, S3a to S3f, S4, S5a and S5b below; tracking issue #739 (topic B of #736). S1 and S2a to S2e land before hardware sync slice 1 (#733, ADR 0043); S3 onward runs beside sync. The appendix [0062/tables.md](0062/tables.md) is part of this record: the transition matrix (A), the simulation (B), the reader assignments and clocks (C), the display examples (D), the status writers that are not mechanical (E), the facade assignments (F).
- **Reopen when:** a record changes the Player so that it may live on another thread (alternative iii); ADR 0043 cannot add its states under the obligations of section 15; a measured device reopen on the reference machine holds the interface thread for more than one second after S5b; the display arbitration fails the maintainer's feel-right check; a host file nears the 1,500-line budget and the split by concern does not fit section 11; headless recording is asked for (section 13).

## Context

The inventory for #739 was taken at 3b12412e; every fact below was re-read at `main` 1a56b895. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

- **The host is the interface struct.** Transport and recording are methods on `StudioApp` (`lib.rs:214`), which holds the Player (`:218`), the request tracker (`:219`), the recorder (`:221`), the status line (`:267`) and egui types (`:319`, `:342`). `host/` is 4,241 lines with tests; 57 non-test interface files name the Player or `transit_playback`.
- **The state is flags and text.** No type names the transport's states: they are combinations of `player_requests.*` (`host.rs:263-271`), `take_plan`, `Player::count_in()`, `sim_count_in`, `engine.transport.{playing,recording}` and the status string, whose four waits are compared at `host.rs:343, 354, 380` and `host/transport.rs:59, 165` and never age (`status.rs:26-28`).
- **Commands are queued, not admitted.** `transport.record` only pushes a request (`E/cmd/transport.rs:21`); the control channel drains up to 64 calls (`control.rs:53-56`, `app.run` at `:102`) before the frame polls the Player (`frame_logic.rs:325, 329`), so Record followed by Undo or a measurement evaluates both guards before anything knows a pass is starting.
- **Transport is started outside the host code too.** `Player::start_realtime_render` calls `self.play` (`P/player/device/realtime_render.rs:106`) and its result and cancel paths call `self.stop` (`:137, :165, :188`); no host request tracks them.
- **The Player stays on its thread.** "Main-thread facade. RuntimeControl deliberately makes this type !Send/!Sync." (`P/player/device.rs:69`); "Creating-thread owner" (`P/player/runtime.rs:20-23`).
- **Nothing headless plays.** `Headless` clears every transport request (`crates/automation/src/lib.rs:53-54`). The null Player has "no input and no MIDI ports" (`P/player/device/null_open.rs:11-12`).
- **Recording without a Player is not reachable outside test fixtures.** `arm_recorder` arms audio takes through a recorder that only a Player opens (`host/recording.rs:202-203, 265`; `host.rs:118-141`) and refuses MIDI without a Player (`host/recording.rs:212`); the simulated count-in is set only by the fixture at `midi_transport/tests.rs:138`.
- **Who needs it next.** ADR 0043 slice 1 needs "a deterministic state matrix" without a window.
- **Conventions.** None from Ableton or Logic applies to internal structure. Ableton Live "will not begin recording until the count-in is complete" and Logic's count-in "switches to record when it reaches" the playhead (as quoted in ADR 0052, Context): the count-in is not recording, which is this record's meaning of the flag. Transit still captures MIDI during the count-in on the audio thread (ADR 0052 decision 8); that capture is not the flag.

## Decision

### 1. Settled

By the maintainer (2026-11-16): M1 orchestration moves into a host with no interface dependency and a typed transport state; M2 the flag disagreements are fixed in slice 2, each named with its own test; M3 S1 and S2 land before sync slice 1. Standing rules: no migrations while alpha; no interim surface a recorded plan replaces; exact where a professional could null-test; nothing audible changes unasked; 1,500-line files; related defects become task lines on #739. Coordinator rulings R-A to R-K (after round 1) and S-1 to S-16 (after round 2) are applied below and listed under the judging responses.

Nothing here changes capture, the latch or any saved file. Two playback changes are made, both in S2b: the record start publishes its stage through its ticket (section 5), and the real-time render returns its transport receipts (section 3).

### 2. The crate and the facade

- `transit-host` at `crates/host`, `("transit-host", 5)` in `xtask/src/main.rs` `LAYERS`. Normal dependencies: `transit-engine`, `transit-playback` (L4), `transit-model`, `transit-mix`, `transit-time`, `serde_json`, `log`; a `native-jack` feature forwards to `transit-playback/native-jack`. `check_layers` bars UI crates (`:211`) and upward edges (`:219`, `dl > layer` only); dev dependencies are skipped (`:208-209`). Unsafe stays forbidden.
- `Host` owns the Player, the recorder, the request tracker, the phase and its facts, the simulation, every call that starts or stops the Player (section 3), recording, the MIDI journal lifetime, the autosaves that depend on transport, device reopen, and the connection machinery of `audio_connection.rs` (the `Connection` and `Editors` traits and their implementations become private to the host). `StudioApp` holds `host`.
- `Host::admit(&mut self, engine, status, clock, input)` (section 9) and `Host::step(&mut self, engine, status, clock, input, out)`. The host borrows the engine and the status line.
- **The facade.** The interface never receives `&Player` or `&mut Player`. `Host` hands out four narrow handles that expose only permitted methods: `PluginServices`, `Readouts`, `LiveInput`, `DeviceServices`. Their methods and the interface files that use them are in [tables.md](0062/tables.md), section F.
- **The edge.** At S3f `transit-ui-studio` loses its normal dependency on `transit-playback`; `transit-host` re-exports, item by item in `transit_host::data`, the plain data types the interface still names. It never re-exports `Player`, `Recorder`, `RuntimeControl`, `Control` or `action_lane`, and no public host item returns, derefs to, aliases or names one in an associated type. The interface's dev dependency stays for test fakes; production interface code cannot name the Player. The opener closure in `Services` becomes `transit_host::OpenAudio`.

### 3. The transport phase

```rust
pub enum TransportPhase {
    Stopped,
    StartPending { applied: bool },                    // the pending starts are a fact (below)
    Playing,
    RecordPending { seq: Sequence, ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    CountIn { ticket: Ticket, from: Samples, plan: CountIn },
    WaitingForClock { ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    Recording { latch: Point, count_in: Option<CountIn> },
    StopPending { stop: Option<Sequence>, then: AfterStop },
    Rendering { play: Option<Sequence>, stop: Option<Sequence>, job: RenderJob },
}
pub enum AfterStop { Park { target: Option<Samples>, rtz: bool }, Resume(Samples), Reset }
pub enum RenderJob { Live, Cancelling(CancelReason), Retired }
```

| Variant | Data and why |
|---|---|
| `StartPending` | entered while the set of pending starts is not empty; `applied` once any of them applied. When the set empties: Playing if any applied or playback is observed, else Stopped (S-2) |
| `RecordPending` | the Record request, its full ticket (`P/record/start.rs:17-20`: engine epoch and sequence), the start (`None` for a punch), the count-in sent |
| `CountIn`, `WaitingForClock` | the ticket; entered from the record start's stage (section 5) or the admission receipt, whichever is seen first |
| `Recording` | the latch from the stage `Started(point)`; the count-in plan for the take's plan fallback (`host/recording.rs:418`). Only a Player records, so the latch is required |
| `StopPending` | the Stop request (`None` once its frontier is reached). `Park.target`: a Locate during Park (S-6); `Park.rtz`: a second Stop during Park (D9). `Resume(at)`: a locate during a pass. `Reset`: a session replaced. Reset supersedes Resume |
| `Rendering` | the render's Play and Stop sequences and its job (S-8); left only when the Play has settled (applied or refused), any Stop frontier is reached, the job is `Retired`, and playback is not observed |

**Facts beside the phase:** pending starts (`(Sequence, Samples)` per Play-family request until its receipt, bounded at 16; a seventeenth is refused with the default refusal text); held commands (Park) and deferred commands (Resume); speed `Normal | Half` (invariant: `Half` only in StartPending or Playing; every other transition sets `Normal` and `Player::set_speed(1.0)`, B7); the wanted loop, the loop last sent and the loop a pass records under; locate, release and record-off requests outstanding (record-off is independent, S-7: it survives leaving StopPending until acknowledged or the Player is replaced); observed playing and position (sampled together, B9); the working cursor and retained position (section 8); reopen state, `ReopenPending`, `InputOpening` (S5b); `faulted` (set by `PlayerError::Faulted` or a Player fault, cleared only by replacing the Player); the MIDI transport `recovering` flag; the ingress cutoff (section 15); the interface-owned interlocks (measuring, External Instrument measure; refused at `run`, `frame_logic.rs:47-66`); the backend, Player or simulated.

**Invariant (S-2), pinned by a test after every admission and every step:** Stopped implies no accepted transport request (start, Record, Stop, render Play or Stop) is outstanding and playback is not observed. A step that observes playback while Stopped and nothing outstanding adopts it (row O7: Playing, logged), so a late or untracked receipt can never leave the phase Stopped while the Player plays.

**Calls that start or stop the Player (R-A).** Already host code: `host/transport.rs`, `host/recording.rs` and every `stop_play` caller (`host/reconnect.rs`, `host/autosave.rs:114-116`, `menus.rs:18-19`, `midi_transport.rs:162-173`, `frame_logic.rs:148-151`, `host/recording.rs:565`). Moving into the host: the render (`finish/realtime.rs:190-330`), Player installation (`splash.rs:129-136`) and replacement (`audio_connection.rs:26-86`, `host/reconnect.rs:212-255`). Checked, not transport: the hardware probe, loop calibration and External Instrument measure refuse while playing (`P/player/device.rs:265`, `P/player/device/calibrate.rs:25`, `P/player/device/external_measure.rs:31`); the browser preview is a separate player outside the plan; audition notes start nothing.

**The render's receipts (S-8, a playback change in S2b).** `start_realtime_render` returns the Play's `Sequence` (a synchronous `play()` error stays its `Err`); the job keeps the `Sequence` of every Stop it submits, read with `realtime_render_stop()`; `cancel_realtime_render()` returns the Stop it submitted, if any. The host:
- polls the render where `finish::step_bounce` calls `step_realtime` today (`finish.rs:809-811`, `frame_logic.rs:360`), block H3r of section 9;
- applies, in that block, any cancel requested since the last poll before it collects a result, so a cancel requested in a step wins over a completion first seen in the same step; a result already collected in an earlier step stands and a later cancel is a no-op (`P/player/device/realtime_render.rs:184` finds no job);
- on a refused Play receipt, cancels the job (no Stop needed) and leaves when the job is retired;
- on a fault (no acknowledgment can arrive), cancels the job, which abandons the writer and removes the part files (`:182-190`), treats the transport requests as settled, and leaves for Stopped with `faulted` set;
- on a cancel caused by a session replacement, shows "Render stopped: the session was replaced. Nothing was written."; by a reopen, today's `CANCELLED_REOPEN` (`finish/realtime.rs:26`).

### 4. The state table

| Phase | Audible | State text | Record button | Commands with an effect |
|---|---|---|---|---|
| Stopped | no transport sound (live instruments sound, E3) | none | off | all |
| StartPending | the sound before the first start (nothing, or the old playback) | Play pending | off | starts (added to the set), Toggle, Stop, Pause, Record, Locate (cursor only) |
| Playing | playback | none (outcome "Playing") | off | all but Play |
| RecordPending | from stopped nothing; punch playback | Count-in when a count-in was sent, else Record pending | off | Toggle, Stop, Pause, Record (cancel), Locate (take-safe) |
| CountIn | the click | Count-in | pulses while the published counter runs (as today, `transport.rs:193-197`) | the starts, Toggle, Stop, Pause, Record, HalfSpeed cancel; Locate take-safe |
| WaitingForClock | from stopped nothing (parked); punch playback | Record pending (B1) | lit | Toggle, Stop, Pause, Record (cancel), Locate (take-safe) |
| Recording | playback and capture | none (outcome "Recording") | lit | Toggle, Stop, Pause, Record (take finalized), Locate (take-safe) |
| StopPending Park | playback until the audio thread applies Stop | Stop pending | off | starts, Toggle and Record held; Stop sets `rtz`; Locate sets the target |
| StopPending Resume | as Park | Stop pending | off | Stop, Pause, Record drop the resume; Locate retargets; AllNotesOff at once; HalfSpeed refused; the rest deferred |
| StopPending Reset | as Park | none | off | Stop, Pause re-send; everything else has no effect (S-5: discarded) |
| Rendering | the render's playback | none (the render dialog shows progress) | off | all refused with the render's text, except All Notes Off (cancels the render first, `finish/realtime.rs:164-171`) and Scrub |

### 5. Transitions and the record start's stage

The matrix is [tables.md](0062/tables.md), section A. It is total for this record's phases: every `TransportRequest` (`E/lib.rs:87-101`), every receipt kind, the stage observations, the render, the device events (input loss and input trouble separate, S-4), the live loop change, MIDI loss, the lap limit, held and deferred release. Cells that differ from today carry their B or D item. The simulation has its own table (section B). Rules:

- **Admission (S-1).** A command's cell is applied when the command executes (section 9), and the Player request is submitted then; the phase and capabilities reflect it before the next command runs.
- **Player errors, in this order:** `PlayerError::Faulted` sets `faulted` and applies row R13; an error from a call that cancels the record ticket before it fails (`Player::stop`, `locate`, `set_recording(false)` cancel first: `P/player/device.rs:319, 469-470, 475-476`) ends the pass like a cancelled stage (row O5) and leaves for the phase the transport is in; any other error keeps the phase and posts the error.
- **While `faulted`**, every start refuses with the fault's text until the Player is replaced; finalization runs with Player errors ignored.
- **Untracked refusals** still reach the status line (`host.rs:362-364`, `player_tests.rs:99-116`); stale receipts of a tracked kind clear nothing.

**The stage (S-11, playback change in S2b).** The record start already publishes through its ticket-owned structure: one state word per runtime holding the ticket's sequence and flags, issued with `checked_add` and refusing exhaustion (`P/record/start.rs:10-13, 117-125`), validated against the full ticket by `Reader::status()` (`:232-250`). S2b widens its flags from two bits to three (`FLAGS = 7`, sequences step by 8; exhaustion still refused, never wrapped) and adds stages: `Pending`, `CountingIn`, `Waiting`, `Started(Point)`, `Failed(StartFailure)`, `Cancelled`. Writers, all on the audio thread except Main's cancel: `count_in` admission (`P/player/audio.rs:372-413`), `record_within` waiting (`:296-299, 332-340`), start (`start_placed`, `:349`), failure (the paths that store `start_failure` today, `:949, :967`), cancel (compare-and-exchange from any non-terminal stage, so a late audio-side write never overwrites `Cancelled`). The failure reason and the latch are payload words stored with release before the state word; the reader loads the state word with acquire, reads the payload, and revalidates the state word, the pattern `status()` already uses.
- **Precedence:** `Reader::status()` is authoritative for the pass's lifecycle; `Started(point)` is the latch (`Recorder::applied_start` and `Player::midi_recording_latch` read the same reader); `Failed(reason)` gives the message. Action receipts only clear the request and give the refusal text when the receipt arrives first; the host records which ticket it reported, so one failure is said once. `count_in()` is display only. `take_start_failure()` (`P/player/device.rs:903-904`, a destructive untagged read) is deleted in S2c, when its consumer moves to the stage.
- **Terminal stages (S-3):** `Cancelled` and `Failed` end the pass in RecordPending, CountIn and WaitingForClock: the prepared resources are finalized (`finish_recording`: disarm, discard the journal) and the phase becomes Playing if playback is observed, else Stopped. No phase waits for a latch that cannot come.

### 6. Status text

- `StatusLine` (plain data): `message: Option<Message { text, kind: Outcome | Note, rev, since }>`, `state: Option<State { text, rev }>`, `next_rev`. `set(text)` takes a new revision when the text differs; `say_again` (today's `StatusAge::restart`, #216) takes one for equal text; `last_text()` returns the last written message, which is what `Err(self.status.clone())` returns today. `StatusAge` ages only `message`.
- `state_text(phase, facts)` is the one producer: ReopenPending "Reopening the audio device" over the phase's text over InputOpening "Opening the audio input" over release pending "All MIDI notes off pending".
- **Shown:** `message` when its `rev` exceeds `state.rev`, else `state`, else `message`, else empty.
- **Arbitration, once per transition, with the shown line before it as input:** (1) the new state text takes a fresh revision when the line before showed a state text, nothing, or an Outcome; when it showed a Note, the new state keeps the old state's revision, or 0 when there was no state (below every message), so the Note stays visible. (2) An outcome ("Playing", "Recording", "Stopped", "All MIDI notes off") is written as an Outcome only when the line before showed a state text or nothing **and** no state text is live after the transition (S-12): release completing during Stop pending keeps "Stop pending"; Stop completing with a release outstanding keeps "All MIDI notes off pending". The decision is carried in `HostEvent::PhaseChanged { from, to, shown_before, outcome_written }`.
- **Writers that are not mechanical (S-12):** sites that append to, read back, compare or return the status get their own treatment, listed in [tables.md](0062/tables.md), section E; the rest change `app.status = x` to `app.status.set(x)` and `self.status.clone()` to `self.status.last_text()`.
- Nine worked sequences are in section D, each an S2d test. `ui.inspect`'s `status`, the status bar and the diagnostics report read the shown line.

### 7. Capabilities

Published into `engine.transport` at every admission and every transition:

| Field | Meaning |
|---|---|
| `playing` | playback observed: the sampled `Player::is_playing()`, or the simulated state (B9) |
| `position` | the observed position while not Stopped; the working cursor while Stopped; published with `playing` |
| `recording` | `Recording` |
| `record_engaged` (new) | RecordPending, CountIn, WaitingForClock, Recording, StopPending Resume |
| `finalizing` (new) | a take plan, a recorder take pending, a MIDI pass prepared, or a record-off outstanding (S-7) |
| `work_outstanding` (new) | a pending start, Record, Stop, Locate, record-off, release or render request, or a held or deferred command |
| `exclusive_ok` (new) | Stopped, not `work_outstanding`, not `finalizing`, not `faulted` |

Host-only: `running()` (not Stopped and not StopPending Park or Reset), `transport_active()` (not Stopped), `sync_allowed()` (no pending start, Locate, Record or record-off; Stop and release do not block, as `frame_logic.rs:365`). Expected truth table: `record_engaged` true exactly in RecordPending, CountIn, WaitingForClock, Recording and Resume; `exclusive_ok` false in every phase but a settled, healthy Stopped; `finalizing` and `work_outstanding` depend on resources and requests and may overlap `record_engaged`.

Every reader is assigned to exactly one field or predicate in [tables.md](0062/tables.md), section C. `recording_active()` stays only as a test helper (`#[cfg(test)]`, `record_engaged || finalizing`) so `host/live_cycle_tests.rs:311, 336, 373` and `midi_transport/tests.rs:170` keep compiling unchanged. Backend-local readers keep reading the audio side. The new fields appear in `transport.status` (`E/cmd/transport.rs:95`) and `session.inspect`'s `transport` (`E/inspect.rs:100`); `ui.inspect` adds `record_engaged`; `docs/control-protocol.md` is updated in S2c, keeping its `recording_pass` contract (`:122-123`).

### 8. Positions and synchronous effects

Within an admission or a step the host keeps a working cursor and working positions; each command sees the effects of the commands before it.

| Effect | Side |
|---|---|
| cursor move by a locate or take-locate (`host/transport.rs:145, 268`) | synchronous (working cursor); `CursorTo` for the interface's copy |
| transport and retained positions (`host/transport.rs:52, 66, 146, 223, 260, 270`) | synchronous, published at once |
| a stopped cursor move from the interface (`lib.rs:537-539`) | synchronous: `set_cursor` calls `host.cursor_moved(at)` |
| selection after a placed take (`host/recording.rs:640, 712`) | synchronous: the host runs `edit.select` |
| Return to Zero on a second Stop (`host/transport.rs:238-241`) | synchronous: `engine.execute("transport.rtz")` and its Locate admitted before the next command |
| undo squash of a Record operation (`host/recording.rs:466-478`) | synchronous |
| ResetSession's cursor clear (`host/transport.rs:199`) | synchronous (working cursor := session start) |
| scroll to pickups, focus, take lanes, reset view clears, Record hint, editor re-want, editor edits | deferred events (`PickupsBefore`, `TakePlaced`, `SessionReset`, `RecordHint`, `EditorsReopened`, `EditorEdits`) |

**Retained position (S-6):** on Park completion, `Park.target` when a Locate set it, else the applied stop position read before parking (`host.rs:96-97`); written by PlayExact and Locate (the target) and ResetSession (0); read by Continue. So Continue resumes from where the user put the cursor during Stop pending.

### 9. Admission and the step

**Admission (S-1).** The engine's `TransportRequest` queue stays as the engine's hand-off buffer and is never carried across a command boundary when a host exists:
- Every interface path that executes an engine command goes through one function, `StudioApp::execute` (today `run`, `execute_ui_merged`, `execute_with_mode` and the 21 direct `engine.execute` sites in the interface), which calls `host.admit` right after `engine.execute` returns. Admission first drains receipts (block H1 below) so the command sees every acknowledgment published so far, then applies each queued request through the matrix in order, submits the Player requests, and publishes the capabilities. The next command's guard therefore sees the new phase: Record then Undo refuses the Undo; Record then a measurement refuses the measurement.
- The control channel's batch (`control.rs:53-56`) and MIDI transport buttons call `run`, so each call is admitted before the next is evaluated. MIDI buttons run right after the control drain in `logic()` (D7) and are decided against the admitted phase; buttons received during StopPending stay in the button queue, as today (`midi_transport.rs:224-227`), and are dispatched after the stop completes.
- **Replies.** A transport command whose admission refuses returns `Err` with the refusal text (which also goes to the status line, as today); `Ok` means admitted: the phase moved and the Player request was submitted, not that it applied (B13). The MIDI dispatch's device-named Record refusal keeps its text: it treats an admission `Err` for an unarmed Record as its `refused` case (`midi_transport.rs:260-266, 288-292`).
- **Engine only** (engine tests, a build with no host): requests stay in the queue as today; `Headless` clears them until S4 and admits them after.
- A step that finds the queue not empty logs a defect and admits it first (a missed `execute` path).

**Clock (R-F).** `trait HostClock { fn frame(&self) -> f64; fn monotonic(&self) -> Duration; fn calendar(&self) -> SystemTime; fn sleep(&self, d: Duration); }`; the host never calls the system clock. Timed things and their clocks are in [tables.md](0062/tables.md), section C, last part; the Player's own budgets inside its service calls are exempt (`P/player/handles.rs:31, 897-913`).

**Input.** `StepInput { dt, cursor, prefs: HostPrefs, pointer_down, touches_open, ingress: IngressCutoff }`; `HostPrefs` copies count-in, click level, monitor settings, audio configuration, low-latency limit, MIDI transport profiles.

**The step** handles receipts, observations, timers and device events only:

| # | Block | Today |
|---|---|---|
| H1 | receipts in `poll_notices` order, then the stop and release frontiers; Park completion reads the applied position, then releases held commands (they are admitted) unless the ingress cutoff or a superseding event cancelled them | `frame_logic.rs:329`, `host.rs:82-114` |
| H2 | recording clock observation and journal; lap limit | `:335-337` |
| H3 | output trouble, input trouble, input loss, reopen | `:355-357` |
| H3r | render: pending cancels, then result poll | `:360` (`finish.rs:809-811`) |
| H4 | click and monitor pushes; the record stage; the Resume check | `host/transport.rs:139-167` |
| H5 | loop retarget; readback, natural end, adoption (O7); simulation timers; meters | `host/transport.rs:309-386` |
| after | automation write pass; session sync when `sync_allowed()`; autosave; input | `frame_logic.rs:364-375` |

**Held and deferred commands (S-5).**
- Park holds starts, Toggle (a start, so Space twice restarts) and Record; they are admitted in order at completion. A held start is cancelled by a later Stop, by ResetSession, by a device reopen or replacement, by a fault, and by the ingress cutoff.
- A second Stop during Park sets `rtz`; Return to Zero runs at completion (D9: today it depends on the observations of the moment, `host/transport.rs:228-241`).
- Reset discards held and deferred commands: opening a session always starts stopped.
- Resume defers every command except Stop, Pause, Record (they drop the resume), Locate (retargets), AllNotesOff (at once) and HalfSpeed (refused). When `stop` and record-off are both acknowledged, H4 checks for a superseding command already admitted (Reset, Stop, Pause, Record: then Park or Reset), else submits the full Record start at the target; the deferred commands are released when that Record's admission receipt or stage arrives (CountIn, WaitingForClock, Recording) or when it is refused (receipt or stage `Failed`/`Cancelled`, or a local refusal, which leaves Stopped with the finished take preserved and the reason visible). Not on submission, not only on the latch.

**Device-free step (R-I).** S2b extracts the transport part of `logic()` into `StudioApp::step_host(&mut self, clock, input)` with no egui argument, and adds `host::testing::device_free_app()`; this is the window-free adapter sync slice 1's gate uses before the crate move.

**The pure core.** `next(phase, event, facts) -> Next { to, effects }` takes no strings and no Player; the executor performs the effects and feeds `Sent` or `NotSent` back in the same admission or step.

### 10. Behavior changes

Each is confirmed by reading `main` 1a56b895 (rounds 1 and 2 confirmed them too); every test runs its negative control on the commit before its slice. Tests are named in the Slices gates.

| # | Change | Slice |
|---|---|---|
| B1 | the recording flag only from the latch; the wait for the clock shows "Record pending" (`P/player/audio.rs:299, 340, 412`; `host.rs:349-355`) | S2c |
| B2 | StopPending Park and Reset are not a record operation; Resume is (`host/recording.rs:375-381`, `host/transport.rs:63`, `host/recording.rs:281`) | S2c |
| B3 | Count-in leaves only on the record start's stage (`host/recording.rs:149-154`, `host/transport.rs:165-166`) | S2d |
| B4 | the displayed state is derived and arbitrated (`host.rs:343, 354, 380`) | S2d |
| B5 | replacement and reopen leave no pending text (`host.rs:280-282`, `host/reconnect.rs:107-112, 227-228, 245`) | S2d |
| B6 | a Record start no longer pulses Play; the plug-in loading override stays (`host/recording.rs:150-152`, `transport.rs:120-127`) | S2d |
| B7 | the speed invariant; the simulation honors speed; the half-speed Record refusal stays (`host/transport.rs:12-21, 289-292, 361`; `host/live_cycle_tests.rs:366`) | S2e |
| B8 | a take-locate keeps readback and pushes; meters still suppressed while Stop is pending (`host/transport.rs:139-142, 324`) | S2c (the early return goes with Resume as a phase) |
| B9 | one playing and position pair per step, `midi_monitor.rs:674` included (`lib.rs:637`, `host/transport.rs:312-317`) | S2e |
| B10 | the count-in plan lives in phase data (`host/recording.rs:154`, `host/transport.rs:58`) | S2d |
| B11 | one owner for the positions, no timing change (`midi_transport.rs:59`, `lib.rs:537-539`) | S2e |
| B12 | blocking changes, only for readers that newly block: the raw-flag readers block from RecordPending on (`actions.rs:11, 15`, `menus.rs:18, 402`, `E/lib.rs:770`); exclusive operations need `exclusive_ok`; the save, timed-read and plan-wait guards include the pending-start set and the render (tables C) | S2c |
| B13 | a refused transport command replies `Err` at once (today `Ok`, then a status message) | S2c |

**Related defects** (proposed task lines on #739):

| # | Today | Instead | Slice |
|---|---|---|---|
| D1 | Play or Toggle during a clock wait from stopped restarts playback and drops the record start (`host/transport.rs:218-219, 246-249`; `P/player/audio.rs:452, 417-418`) | Play ignored; Toggle cancels | S2c |
| D2 | PlayExact and PlaySelection have no pass guard outside an observed count-in (`host/transport.rs:222-224, 273-279`) | ignored during RecordPending, WaitingForClock, Recording; PlayExact in CountIn cancels | S2c |
| D3 | a session replaced during a take-locate attempts the resume on the new document (`host/transport.rs:139-148`) | Reset supersedes Resume | S2c |
| D4 | starts during Stop pending depend on the moment's observations and can coexist with the Stop (`host/transport.rs:21-23, 218, 246-256`) | held, released at completion; Toggle counts as a start | S2c |
| D5 | Locate or a loop change during Stop pending sends a request to a stopping Player (`host/transport.rs:85-92, 128-133, 268-270`) | sets the park target; nothing sent | S2c |
| D6 | Pause while stopped sends a Stop (`host/transport.rs:244`) | no effect | S2e |
| D7 | MIDI buttons run in the drawing pass after the write pass and autosave (`frame_logic.rs:363-372, 467`) | run after the control drain, admitted like any command | S2c |
| D8 | ResetSession during a render sends Stop under it (`host/transport.rs:176-184`) | the render is cancelled first, with its own message | S2c |
| D9 | a second Stop while a Stop is pending returns to zero only when the observations of the moment say stopped (`host/transport.rs:228-241`) | always, at completion | S2c |

### 11. The crate boundary and tests

| Moves to `transit-host` | Slice |
|---|---|
| `host/phase.rs` (phase, facts, `next`, `state_text`, capabilities), the request tracker and receipts, the pure texts, `midi_transport::decode` and the button table, the count-in arithmetic | S3a |
| `Host` with the Player and recorder; `host/transport.rs`, `host/recording.rs` (view effects as events), `host/autosave.rs`, `host/reconnect.rs`, `host/state_guard.rs`, `host/timed_read.rs`, `host/plugin_state.rs` minus `apply_edits` (`:193-214`, an `EditorEdits` event), the non-drawing parts of `midi_transport.rs`, `metronome.rs`, `transport.rs`; `StatusLine`; the render; `audio_connection.rs`; Player installation (`splash.rs:129-136`); the journal's clock parameters (`E/midi_journal.rs:114, 134-140, 212`) | S3b |
| the four facades and their files | S3c to S3e |

Stays: view state, `StatusAge` and drawing, meter ballistics, `run` and its interlocks, window features, preferences. Tests in S3b: test files reaching into moved fields change paths to the host's test accessors and setters (`host::testing`), and time fields to the stepped clock; the reviewer checks the test diff holds only those kinds of edits.

### 12. Blocking calls

| # | Call | Target | Slice |
|---|---|---|---|
| 1 | device reopen (`audio_devices.rs:509, 527`, `frame_logic.rs:291`, `host/reconnect.rs:104, 173`, `host/plugin_state.rs:40-41`) | staged: `ReopenPending` is shown, the reopen runs in a pass after one that drew it (the splash's precedent, `splash.rs:115-124`); a held Play is cancelled by Stop. The open itself holds the Player: out of scope | S5b |
| 1a | ui.json write after verification (`audio_devices.rs:325-356`) | worker | S5a |
| 2 | `io.measure_hardware_latency` sleep loop up to 8 s (`hardware_measure.rs:21, 86-94`) | stepped (`start_measure` `:98`, `step_measure` `:136`); the control request is parked and answered when done, in order, and cancelled on disconnect | S5a |
| 3 | `wait_for_plan` up to 150 ms (`host/state_guard.rs:215-241`) | out of scope, bounded; the clock capability | none |
| 4 | plug-in state capture (500 ms, `P/player/handles.rs:31`) | out of scope; ADR 0018 amendment to reopen | none |
| 5 | `Autosaver::flush` (`E/autosave.rs:83-86`) | timed write by ticket; panic, close and recovery keep the flush | S5a |
| 6 | saves inside `engine.execute` | out of scope | none |
| 7 | input open (`host.rs:118-141`) | staged with `InputOpening`; the open itself out of scope | S5b |
| 8 | `machine_listing` (`audio_devices.rs:716`) | worker, as the ASIO listing (`:731`) | S5a |
| 9 | quit (`host.rs:75-78`) | out of scope | none |
| 10 | file pickers | #39 | none |

Reopen condition for the out-of-scope parts: a record that lets a Player be created or dropped on a worker.

### 13. Headless (S4)

- `Headless` holds a `Host` and admits each command's requests inside `answer` (section 9).
- **Replies.** `Ok` for Play means admitted (StartPending entered and the request submitted); `Err` with the refusal text otherwise. Record is refused explicitly, before arming or journaling, with "Recording needs an audio or MIDI input; this headless session has none.": the null Player is `Some`, so today's `None` refusal (`host/recording.rs:212`) would not fire and `prepare_midi_recording` would prepare a pass with no ports (`P/player/device.rs:390-403`).
- **Stepping.** A reader thread reads lines and sends them over a channel; the thread that owns the Player handles at most a bounded amount of message work (one line, or up to 16 entries of a JSON-RPC batch) and then steps the host whenever the monotonic deadline (10 ms after the last step) has passed, and otherwise waits on the channel until that deadline. Continuous input cannot starve the step.
- **Confinement.** Deferred host work (steps, natural ends, journal writes, autosave) runs inside the same `file_access::confine` scope as `answer` (`crates/automation/src/lib.rs:70-73`).
- **Shutdown.** On EOF or disconnect: parked replies are cancelled with an error, the host stops the transport and runs its quit order (close autosave, close input, drop the Player), then the loop ends.
- **Scope.** Playback through the null device, every transport command, offline work and inspection. Real-time measurements and renders refuse for want of a live device, as their guards already do. Recording would need a virtual MIDI input on the null Player or a null audio input: a later record.
- **Build.** `transit-automation` and `transit-cli` link `transit-playback` (cpal, the native backends on Windows, ALSA development files on Linux); `native-jack` forwarded. `transit-engine` alone builds without them.

### 14. The guard

- **Final (S3f):** the layer rule; one forbidden normal edge in `check_layers` (`transit-ui-studio` may not depend on `transit-playback`); an audit of the host's whole public surface (no item returns, derefs to, re-exports, aliases or exposes through an associated type or `impl Trait` bound a Player, Recorder or their control types), recorded in the S3f landing; a compile-fail fixture (a production-consumer crate depending only on `transit-host` that tries to name `Player` through every public item, expected to fail).
- **Transitional lint (S3a to S3e):** `xtask/src/host_guard.rs` counts calls of `Host::legacy_player()` and textual transport calls per non-test interface file on a ratchet table (as `xtask/src/file_size.rs:11-38`). A review aid with known misses and false positives (`hardware_measure.rs:276`, `keys.rs:915`), not ownership. S3f deletes it and `legacy_player()`.

### 15. Relation to ADR 0043

| Requirement | Here |
|---|---|
| 1. sync mode changed only stopped and finalized | `exclusive_ok` (S2c) |
| 2. Start/Continue/Stop/SPP with its own start pending | the pending-start set, `next()`, the matrix; ADR 0043 adds its armed states by amendment (below) |
| 3. duplicate suppression against a pending locate | the Locate receipt keeps sequence and target; ADR 0043 adds the applied epoch and source generation |
| 4. record engaged, "Waiting for external start" | ADR 0043's state, under the obligations below; B1 is its precondition |
| 5. sticky local Stop | ADR 0043's mechanism at ingress admission (ADR 0043:534-537). The host applies `StepInput.ingress` (`IngressCutoff { generation, offered_sequence }`, kept distinct as ADR 0043 defines them) at the start of every admission and before releasing any held or deferred command, so no queued or held start older than the cutoff is admitted after the Stop that set it |
| 6. half speed and scrub refused in clock follow | ADR 0043's condition on those rows; a refusal leaves phase, speed and position unchanged |
| 7. readout through `TransportStatus` | ADR 0043's; S2c adds four fields |
| 8. applied moments for sending | ADR 0043's callback-published spans (ADR 0043:319); `PhaseChanged` is not used for sending |
| 9. slice 3 refusals, tempo, journal | unchanged |
| 10. a deterministic matrix without a window | the pure matrix (S2a) and the device-free step (S2b), before sync slice 1 |

**The matrix is total for this record's phases only (S-10).** ADR 0043 adds its states by amendment. Any added state must define: its row for every existing event and its column for every existing phase; its capabilities (`record_engaged`, `work_outstanding`, `exclusive_ok`); its state text; and its cancellation on local Stop, selected-source Stop, ingress overflow or loss, source replacement, session replacement, device failure and preparation failure. Input to that amendment, from round 2: three sub-states differ: (1) Record engaged locally, waiting for an external Start (it holds a prepared ticket); (2) an external Start or Continue received, waiting for its first following clock tick (it holds the start message's boundary, to reject an earlier tick); (3) a submitted recording action (this record's RecordPending).

**Stale statements.** No ADR 0043 decision becomes false. Its path statements naming `crates/ui-studio/src/host/transport.rs` (Implementation; 2026-11-04 amendment) need a pointer amendment at S3b.

### 16. Relation to ADR 0052

ADR 0052 relies on the audio thread (decision 8 and its 2026-11-16 amendment: "The count-in bar keeps its full length in output frames"), on `count_in_for`, on `plan_recording` writing `count_in` and on `recording_pass`. None changes: the stage only reports what the audio thread does. Path statements become stale at S3a and S3b and need a pointer amendment.

## Slices

Every commit builds; every slice runs the full local gates and is shippable alone. Nothing consumes what a later slice supplies; a writer of a meaning and all its safety readers change together. "Unchanged" means the file is not edited. Sizes are changed lines including tests.

| Slice | Scope | Not in it | Size |
|---|---|---|---|
| S1 | read-only `transport_phase()` with request metadata (play start, record from, count-in, ticket, record-off kind); derivation precedence below; `classify`/`frontiers` behind wrappers | any behavior | about 500 |
| S2a | the pure reducer, complete payloads, section A as a literal table; `transport_phase_matrix`, `the_matrix_has_no_empty_cell`, `stopped_implies_nothing_outstanding` (over the reducer), unused | any caller | about 900 |
| S2b | groundwork, behavior unchanged: the device-free step adapter; the receipt-bearing render API with the render orchestrated through one function; the stage publication in the ticket structure (nothing reads the new stages yet; `take_start_failure` unchanged) | any consumer | about 1,000 |
| S2c | the lifecycle becomes authoritative: synchronous admission, the pending-start set, cleanup and finalization, capabilities and every safety reader, engine guards, the render as a phase, the stage consumed, S-3 to S-7; B1, B2, B8, B12, B13, D1 to D5, D7 to D9 | display, speed | about 2,400 |
| S2d | display arbitration and count-in reporting; the status writers (section E by hand, the rest mechanical); B3 to B6, B10 | lifecycle | about 1,400, of which about 400 mechanical |
| S2e | speed, positions, the remaining readouts; B7, B9, B11, D6 | | about 700 |
| S3a | the pure machine into `transit-host`; the transitional lint | Player | about 1,250 |
| S3b | the Player into `Host`; clock and input types; journal clock parameters; `legacy_player()`; pointer amendments | facades | about 3,500 |
| S3c / S3d / S3e | `PluginServices` / `Readouts` and `LiveInput` / `DeviceServices` | | about 500 each |
| S3f | data re-exports; the edge removed; public-surface audit; compile-fail fixture; forbidden edge; `legacy_player()` and the lint deleted | | about 600 |
| S4 | headless (section 13) | recording | about 500 |
| S5a / S5b | section 12 | | about 400 each |

**S2c is not one review sitting.** Its one safe cut: S2c-1 (admission, pending starts, lifecycle, capabilities with every reader, engine guards, S-3 to S-7, B1, B2, B8, B12, B13, D3 to D5, D7, D9; about 1,800) and S2c-2 (the render as a phase with D8, and the pass guards D1 and D2; about 600). Between them every guard is right: until S2c-2 the render stays an interlock refused at `run` (`finish/realtime.rs:174-177`) and counted in `work_outstanding` through a `render_running` fact, and D1 and D2 keep today's cells, whose ticket cancellation S-3 already finalizes. S2c-1 cannot be cut further: admission, the phase and the capabilities change the meaning of "a pass is under way" at once, and any split leaves a guard reading a meaning its writer does not produce yet.

**S1 derivation precedence** (first match wins): a render running (`finish.realtime.running`) -> Rendering; `player_requests.stop` Some -> StopPending (Resume when `take_locate` is Some, Reset when a `ResetSession` was processed this step, else Park); `take_locate` Some with a record-off outstanding -> StopPending Resume with `stop: None`; `take_plan` Some and a record request outstanding -> RecordPending; `take_plan` Some and `count_in_state()` Some -> CountIn; `take_plan` Some and a latch -> Recording; `take_plan` Some -> WaitingForClock; `player_requests.play` Some with play metadata -> StartPending; `is_playing()` -> Playing; else Stopped. Without a Player: `sim_count_in` Some -> CountIn (fixture only), `sim` Some -> Playing, else Stopped.

**Gates, per slice.**
- **S1:** every existing test unchanged; new: six derived-phase tests and `state_text_names_the_waits`.
- **S2a:** every existing test unchanged; the three reducer tests.
- **S2b:** every existing test unchanged (the render test at `E/cmd/realtime_render.rs:208-213` included); new: `the_render_reports_its_play_and_stop_sequences`, `a_refused_render_start_returns_no_job`, `the_record_stage_follows_count_in_wait_start_fail_and_cancel` (playback, with exhaustion refused and a late audio write never overwriting `Cancelled`), `step_host_matches_logic_for_the_transport_part`.
- **S2c:** edited: `player_tests.rs` to `classify`/`frontiers`; the 23 sites in 10 interface test files and `E/tests/recording.rs:8` that set the recording flag as a stand-in for a pass now set the phase or capabilities through `host::testing`; `E/cmd/realtime_render.rs:208-213` sets `playing` and `recording` capability-aware; `E/tests/session_transport.rs:9` gains `..Default::default()`; `midi_transport/tests.rs:138` uses a device-free count-in (the simulation no longer counts in); `state_guard_round3_tests.rs:31-34` sets a pending start through `host::testing`. Unchanged: `host/live_cycle_tests.rs` (the `recording_active()` test helper stays), `midi_transport/tests.rs:211, 257`, the playback count-in tests (`P/player/audio/tests/count_in.rs:63, 118, 161, 240-251`), `P/player/audio/midi/tests.rs:387`. New: every B and D test of the slice; `record_then_undo_in_one_drain_refuses_the_undo`, `record_then_measure_in_one_drain_refuses_the_measurement`; `an_older_play_applying_after_a_newer_refusal_leaves_playing`; `stopped_implies_nothing_outstanding` after every admission and step in the integration tests; `a_cancelled_ticket_ends_the_pass`, `a_failed_stop_that_cancelled_the_ticket_ends_the_pass`; `input_loss_keeps_recording`; `reset_discards_held_commands`; `continue_after_a_locate_during_stop_pending_resumes_at_the_target`; `record_off_outstanding_keeps_exclusive_ok_false`; render fault, cancel-versus-completion and session-replacement tests; Resume with audio and MIDI, deferred commands and an intervening edit; journal handoff interrupted at each step (`host/autosave.rs:72-83`).
- **S2d:** edited: `status.rs:147` becomes `a_state_text_stays_while_its_phase_holds_and_leaves_with_it`; state-text assertions read the shown line in `slice6_tests.rs:404, 422, 455, 462-463`; `transport_tests.rs:317` and `plugin_retry_tests.rs:330` set StartPending through `host::testing`; tests asserting composed messages from section E sites keep their expected strings. New: the nine display sequences; B3 to B6 and B10 tests; `the_lap_limit_note_never_copies_a_state_text`.
- **S2e:** unchanged: `host/live_cycle_tests.rs:304, 326, 342, 366`; new: B7, B9, B11, D6 tests.
- **S3a to S5b:** as in their rows; S3b's test edits are mechanical paths only; S3f's completion gate is the audit and the compile-fail fixture.

**Feel-right items:** S2c: at Stop after a take the Record light goes off at once; Space twice quickly restarts playback once the stop completes. S2d: Record from stopped with a one-bar count-in: Play does not pulse; Count-in, then Recording; Stop pending, then Stopped; Reconnect while playing leaves no Stop pending. S5b: Apply shows "Reopening the audio device" before the window waits.

## Consequences

- One value answers what the transport is doing, a render included; commands see each other's effects at once; status strings are display only.
- `recording` means capture; the engine's guards read named capabilities; exclusive operations wait for a settled transport.
- A refused transport command fails its call; `Ok` means admitted, not playing.
- The interface cannot name the Player after S3f.
- Headless plays and steps itself, does not record; `transit-cli` links the audio backends.
- The host still runs on the interface thread; a slow driver open still holds the window, announced first.
- About 4,000 lines move; ADR 0043 and ADR 0052 need pointer amendments at S3b.

## Alternatives rejected

- **Type the phase in place and never move**: nothing headless, nothing to guard.
- **The host in `transit-engine`**: every engine user would link the device backends, and the document crate would gain the Player's thread rule.
- **A host actor on its own thread**: the Player is neither Send nor Sync by design.
- **The host in `transit-playback`**: the device crate would depend on the document crate.
- **Strings as constants**: the text would still be the state.
- **A regex guard as the boundary**: bypassed by aliases, traits and the render; a transitional lint only.
- **The render beside the phase, or left on the observed flag**: the phase could read Stopped while the render's requests or job are unsettled (round 2).
- **Inferring the count-in end from the counter or elapsed time**: misses whole count-ins.
- **Queued admission with publication after each transition** (round 1's revision): commands executed in one drain still evaluate their guards before the earlier command's transition (round 2).
- **A truncated ticket in a new atomic** (round 1's revision): loses the engine epoch and the exhaustion refusal the ticket structure already has.

## Judging round 1: responses

Round 1 (REJECT): every finding was verified at `main` 1a56b895 and accepted; none rejected. Round 2 re-audited them; the ones it found not resolved are answered in the next section. Accepted and applied: B2 limited to Park and Reset; B4 restated; the B6 override kept; the B8 meter suppression kept; B11 without delay; D2 and D3 restated; the render brought under the host; replacement Play (now the pending-start set); Stop and Play no longer coexisting (D4); S1 metadata; the persistent fault; the missing matrix rows; untracked refusals posting; the untagged cells (D4 to D6 and matching cells); the working cursor; explicit capabilities; the missing readers; B12; the inspection and protocol surfaces; command-time freshness (now synchronous admission); the display rule; today's step order; the session-sync gate; the clock capability; the layer placement; a facade instead of a regex; the splash; `apply_edits`; the touches gate; test setters; headless build, stepping and capture; the S2 split; the four tests; integration coverage; S3 time conversions; S5 presentation; the ADR 0043 mappings; the round-1 rulings; the omissions list.

## Judging round 2: responses

| Finding | Disposition |
|---|---|
| 1. Round-1 audit, items not resolved | Answered by the rows below. |
| 2. Replacement Play loses history (A applies after B is refused) | Accepted. Verified at `P/player/audio.rs:445-453`. The pending starts are a set (section 3); test `an_older_play_applying_after_a_newer_refusal_leaves_playing`; invariant and adoption row O7. |
| 2. Cancellation strands a pass | Accepted. Verified: `Player::stop` cancels at `P/player/device.rs:475` before `owner()` at `:476`; `count_in.rs:240-251` exercises a cancelled ticket. Terminal stages end the pass (section 5, row O5). |
| 2. Input loss stops recording | Accepted. Verified at `host/reconnect.rs:57-66`. Rows V2 (trouble) and V6 (loss) split. |
| 2. Reset and Park share holding | Accepted. Reset discards; Park holds (section 9). |
| 2. Deferred release contradicts the table | Accepted. One release point: the resumed Record's admission receipt, stage or refusal (section 9, row H2). |
| 2. D5 loses the retained target | Accepted. `Park.target`; retained := target (section 8). |
| 2. Record-off cannot survive Park completion | Accepted. Record-off is an independent fact (sections 3, 7). |
| 2. Simulation cannot construct CountIn | Accepted, scoped: simulated recording is unreachable outside a fixture (Context; `host/recording.rs:202-212, 265`); the simulation has Stopped and Playing only; `Recording.latch` is required. |
| 2. Simulation events incomplete | Accepted. Section B is total, with wrap before end (`host/transport.rs:363-377`). |
| 2. StartArmed not in the matrix | Accepted. Removed from the matrix; ADR 0043's obligations listed (section 15). |
| 2. Rendering substates | Accepted. `play`, `stop`, `job` (section 3); rows E1 to E5. |
| 2. E1 in Stopped lacks its guard | Accepted. E1 requires `exclusive_ok` (row E1). |
| 2. Walks (a) to (h) | (a) matches the new rules (a Note before any state gives the state revision 0, section 6); (b) accepted, pending-start set; (c) confirmed preserved; (d) accepted: MIDI buttons during Stop pending stay in the button queue as today, so the same-step MIDI Locate does not move the cursor; (e) accepted, held and deferred cleared by Reset and reopen; (f) accepted, cancel message for a replacement (section 3); (g) accepted, Toggle held, second Stop tagged D9; (h) accepted, the simulation no longer counts in. |
| 3. Capabilities overlap | Agreed; section 7 says they overlap. |
| 3. Missing save and capture readers | Accepted. `host/plugin_state.rs:69-73`, `host/timed_read.rs:84-86`, `host/state_guard.rs:216-220` in tables C, with the changed-document qualification; B12. |
| 3. Locate and release in `exclusive_ok` | Kept, as the judge recommends. |
| 3. Freshness not solved | Accepted. Synchronous admission (section 9). |
| 4. First state after a Note | Accepted. Revision 0 (section 6); sequence 7. |
| 4. Outcomes hide live states | Accepted. No outcome while a state text is live (section 6); sequences 8 and 9. |
| 4. Writers that append, read, compare or return | Accepted. Verified the ten sites and found more; tables E. |
| 5. `record_stage` contract | Accepted. Published through the ticket structure with full identity and exhaustion refused; terminal payloads; precedence and deduplication; acquire and revalidation (section 5). |
| 6. Rendering completion | Accepted. Receipt-bearing render API, fault path, cancel-versus-completion order, polling block H3r (section 3). |
| 7. Public API leakage | Accepted. S3f audit and compile-fail fixture (section 14). |
| 7. Facade assignments wrong | Accepted. Corrected in tables F. |
| 7. `audio_connection.rs` traits | Accepted. Private to the host (section 2). |
| 7. `legacy_player()` removal | Accepted. S3f's completion gate; S3f is not small. |
| 8. Slices consume later slices | Accepted. Re-cut S2b to S2e (Slices). |
| 8. Named tests | Accepted. `recording_active()` kept as a test helper; `midi_transport/tests.rs:211` preserved by the button queue; `:257` by `recovering`; render test capability-aware. |
| 9. ADR 0043 mapping 5 | Accepted. Cutoff applied before held work; generation and offered sequence distinct (section 15). |
| 9. Three sub-states | Recorded as input to ADR 0043's amendment (section 15). |
| 10. B12, D4 to D8 | B12 narrowed to readers that newly block; D4 Toggle held; D5 target kept; D6 removed from the MIDI-loss row (that row keeps today's Stop); D7 and D8 kept with tests. |
| 11. Headless | Accepted. Explicit refusal, bounded work and deadline stepping, reply meaning, confinement, shutdown (section 13). |
| 12. Verdict classes 1 to 6 | Answered by the rows above. |

## Judging

## Related

[ADR 0043](0043-hardware-sync-transport-and-clock.md), [ADR 0052](0052-pre-count-zone.md), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md), [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md), [ADR 0018](0018-plugin-host-thread.md), [ADR 0026](0026-cycle-recording-stacks-takes.md), [ADR 0027](0027-external-instrument-real-time-render.md). Issues #739, #733, #736, #39.

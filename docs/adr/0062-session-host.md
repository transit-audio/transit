<!-- transit-index
title: The typed transport lifecycle: one transport phase is the truth for starts, stops, recording and the real-time render; every Player request is a tracked fact until retired; transport changes are admitted synchronously; engine guards read named capabilities; the record start publishes its stage through its ticket; status text is arbitrated per transition; the code stays in the interface crate with no window types, ready to move; lands before hardware sync slice 1
-->
# ADR 0062: The typed transport lifecycle

- **Status:** accepted 2026-11-17 (proposed 2026-11-16; six judging rounds; narrowed to the lifecycle after round 3); Amended 2026-11-17 for #739 (slice S2a): the matrix made exact, see the amendment at the end
- **Date:** 2026-11-16
- **Implementation:** slice S1 landed at 8f9bf93c (the derived transport phase, read only); slice S2a is built and in review (the pure reducer over the literal matrix, unused by the app); S2b to S2g not started. Slices S1 and S2a to S2g below, all before hardware sync slice 1 (#733, ADR 0043). Tracking issue #739 (topic B of #736). The appendix [0062/tables.md](0062/tables.md) is part of this record: the transition matrix (A), the simulation (B), the reader assignments (C), the display examples (D), the status writers that are not mechanical (E).
- **Reopen when:** ADR 0043 cannot add its states under the checklist of section 14; the display arbitration fails the maintainer's feel-right check; a lifecycle file nears the 1,500-line budget; one of the deferred records needs a lifecycle change this record does not allow; the owed-Stop deadline (section 4) is reached in practice.

## Context

Facts re-read at `main` 1a56b895. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

- **The state is flags and text.** No type names the transport's states: they are combinations of `player_requests.*` (`host.rs:263-271`), `take_plan`, `Player::count_in()`, `sim_count_in`, `engine.transport.{playing,recording}` and the status string, whose four waits are compared at `host.rs:343, 354, 380` and `host/transport.rs:59, 165` and never age (`status.rs:26-28`). Section 13 lists the defects this causes.
- **Commands are queued, not admitted.** `transport.record` only pushes a request (`E/cmd/transport.rs:21`); the control channel runs up to 64 calls (`control.rs:53-56, 102`) before the frame polls the Player (`frame_logic.rs:325, 329`). Some transport changes do not pass through a command at all (section 7).
- **Transport is started outside the transport code.** `Player::start_realtime_render` calls `self.play` (`P/player/device/realtime_render.rs:106`); its result and cancel paths call `self.stop` and discard its result (`:137, 165, 188`).
- **Stop can fail without a fault.** `Player::stop` returns `Busy` when the owner is borrowed (`P/player/device.rs:168, 475-481`) and an error when urgent sequences are exhausted (`P/action_lane.rs:277-279`).
- **Recording without a Player is not reachable outside a fixture.** Audio takes need a recorder only a Player opens (`host/recording.rs:265`, `host.rs:118-141`); MIDI refuses without a Player (`host/recording.rs:212`); the simulated count-in is set only at `midi_transport/tests.rs:138`.
- **ADR 0043 slice 1** needs "a deterministic state matrix" without a window.
- **Conventions.** None from Ableton or Logic applies to internal structure. Ableton Live "will not begin recording until the count-in is complete" and Logic's count-in "switches to record when it reaches" the playhead (as quoted in ADR 0052): the count-in is not recording, this record's meaning of the flag. Capture during the count-in (ADR 0052 decision 8) is the audio thread's and not the flag.

## Decision

### 1. Settled

Maintainer (2026-11-16): M1 the transport gets a typed state and its orchestration leaves the interface crate (this record does the first half; the move is deferred, below); M2 the flag disagreements are fixed in slice 2, each named and tested; M3 slice 1 and slice 2 land before sync slice 1. Standing rules: no migrations while alpha; no interim surface a recorded plan replaces; exact where a professional could null-test; nothing audible changes unasked; 1,500-line files; related defects become task lines on #739. Coordinator rulings R-A to R-K, S-1 to S-16, T-1 to T-9 and U-1 to U-3 are applied and listed under the judging responses.

Nothing here changes capture, the latch or any saved file. Playback groundwork comprises record-stage publication and reader access (section 6), render transport-obligation reporting (section 11), and a safe host-triggered runtime-fault entry point, `Player::fault_runtime()` (section 4). The MIDI record-start reader accessor lands in S1; the remaining groundwork lands in S2b.

### 2. Where the code lives

The lifecycle lives in the interface crate, in its own module `crates/ui-studio/src/host/lifecycle/` (phase, requests, admission, capabilities, status line). Rule: the module names no egui type and reads no `StudioApp` field of an egui type; it takes the frame time, `dt` and a monotonic time as parameters and calls no system clock. It can then move into its own crate unchanged (deferred, below). S2b adds the device-free step: `StudioApp::step_host(&mut self, frame_now: f64, dt: f64, mono_now: Duration)`, the transport part of `logic()` with no egui argument, and `host::testing::device_free_app()` (test only), the window-free adapter sync slice 1's gate uses.

### 3. The transport phase

```rust
pub enum TransportPhase {
    Stopped,
    StartPending,                                  // the current start group is not empty
    Playing,
    RecordPending { ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    CountIn { ticket: Ticket, from: Samples, plan: CountIn },
    WaitingForClock { ticket: Ticket, from: Option<Samples>, count_in: Option<CountIn> },
    Recording { latch: Point, count_in: Option<CountIn> },
    StopPending { then: AfterStop },
    Rendering { job: RenderJob },
}
pub enum AfterStop { Park { end: ParkEnd }, Resume(Samples), Reset }
pub enum ParkEnd { AtCursor, Target(Samples), ReturnToZero }
pub enum RenderJob { Live, Retired { result: Option<RenderOutcome>, reason: Option<CancelReason>, then_reset: bool } }
```

- `RecordPending`, `CountIn`, `WaitingForClock` carry the full record-start ticket (`P/record/start.rs:17-20`: engine epoch and sequence). `Recording.latch` comes from the stage `Started(point)`; only a Player records, so it is required. The count-in plan rides in the phase from S2d (for the take's plan fallback, `host/recording.rs:418`); its display arrives in S2f.
- `from=None` denotes a punch into existing playback; `from=Some(at)` denotes an explicit start. Prepared `count_in=Some(plan)` requires `from=Some(at)`. The host must preserve this pairing. CountingIn observations require both payloads; inconsistent payloads are outside the valid preparation contract and the reducer defensively leaves the phase and effects unchanged.
- `Park.end`: **the last command wins.** A Locate during Park sets `Target(at)` (clearing a return to zero); a second Stop sets `ReturnToZero` (clearing a target). Exactly one applies at completion (section 9). `Resume(at)` is a locate during a pass; `Reset` a session replaced. Reset supersedes Resume.
- `Rendering`: the host owns the render; it leaves only by the rule of section 11. `Retired` keeps the collected result, the cancellation reason and a pending "perform Reset after the render settles" continuation. `RenderOutcome` may be a Completed/Failed summary; the host retains the full result and diagnostic data through settlement. `CancelReason` includes `Fault` for R13 and owed-Stop deadline cleanup.
- Request identities live in section 4's facts, not in the phase.

**Facts beside the phase:** the tracked requests and owed Stops (section 4); at most one held start (Park) and at most 32 deferred commands (Resume), each stamped with the reset generation current when it was held; the reset generation (section 7); speed `Normal | Half` (invariant: `Half` only in StartPending or Playing; every other transition sets `Normal` and `Player::set_speed(1.0)`, B7); the wanted loop, the loop last sent, the loop a pass records under; observed playing and position, sampled together; the working cursor and retained position (section 9); reopen state; `faulted` (set by `PlayerError::Faulted`, a Player fault or an owed-Stop deadline; cleared only when the old Player is retired and a new one installed); the MIDI transport `recovering` flag; the ingress cutoff (section 14); the backend, Player or simulated.

**Invariant, tested after every admission and every step:** Unless faulted, Stopped implies no outstanding request or record-start continuation still capable of moving the transport, no owed Stop, and no observed playback. Requests neutralized by an acknowledged later Stop may remain tracked until their own receipts arrive; they continue to block capabilities wherever section 8 requires nothing tracked. A step that observes playback while Stopped with nothing tracked adopts it (row O7: Playing, logged); adoption is off while `faulted`.

**Invariant (D-5):** Playing never holds an outstanding current-group start: every entry into Playing is R1 or R2 with an empty group, the destination of section 4 with no outstanding member, or O7 adoption with nothing tracked, and a Play-family command that submits a start in Playing opens a new group in StartPending.

### 4. Request ownership

A request becomes a tracked fact when its submission returns a sequence; preparation before that (arming, a journal, a plan) is cleaned up on a local refusal and is not a fact. Facts: the start groups; Record; record-off; Stop; Locate (each one tracked separately); release; `SetLoop` (`P/player/device.rs:315-316`); render Play; render Stop.

- **Retirement.** A fact is retired by its own receipt (Stop and release: their frontier reaching its sequence, `P/action_lane.rs:435-437, 447`), by an early terminal stage (Record only: `Failed` or `Cancelled`; `Started` does not retire Record, its later receipt does and never moves the phase back), by a fault, or when the old Player is retired. Retirement is independent of the phase: a receipt retires its fact in every phase, and the phase rule then says what, if anything, changes. An acknowledged Stop neutralizes earlier ordinary actions and pending record-start continuations; it does not retire their request facts.
- **Late receipts.** A receipt whose fact was superseded or already retired has no lifecycle effect; a refusal's text is still shown once, deduplicated by request identity (`host.rs:362-364`, `player_tests.rs:99-116`).
- **Start groups.** Play-family requests (Play, PlayExact, PlaySelection, HalfSpeed, a Toggle that starts) join the current group, which has an `applied` bit. A Stop closes the current group: its members stay tracked until their receipts retire them, but nothing from a closed group, its `applied` bit included, carries into the next. When the current group's last member retires, the phase becomes Playing if any member applied or playback is observed, else Stopped. The bound is the action lane (256 ordinary actions, `P/action_lane.rs:12`); exceeding it is the lane's existing refusal.
- **Play A, then Record.** Record is admitted with A still in the current group; the phase is RecordPending; A's receipt later retires A with no phase change (its refusal text, if any, is shown). A Record from stopped no longer reuses the Play request (B6).
- **Stage before receipt.** The stage moves the phase (CountIn, WaitingForClock, Recording); the later action receipt retires Record with no phase change. A refusal receipt after a `Failed` or `Cancelled` stage has already retired Record and shows nothing new for that ticket.
- **A failed or cancelled start's destination** is decided when the terminal observation is handled (a Record that fails, is refused or is cancelled), in this order: StartPending whenever the current start group still has an outstanding member (a start that has not been processed: neither applied nor terminally refused), regardless of its `applied` bit; Playing when the group has no outstanding member and playback is observed; else Stopped. The host cannot tell a processed member whose receipt is still on its way from an unprocessed one, so a tracked member counts as outstanding until its receipt retires it.
- **A fault, or the old Player's retirement,** retires every fact and owed Stop, closes every group, discards held and deferred work, and finalizes the prepared resources; cleanup calls that fail create no facts.
- **Shutdown** closes autosave, finalizes resources and returns input before dropping the Player (`host.rs:70-78`). Actual shutdown retirement discharges all requests and owed Stops without waiting for further steps (`Player::drop` quiesces admission even if the stream Stop fails, `P/player/device.rs:1112-1121`); it does not install a new Player or clear the fault for reuse.

**Owed Stops (U-1).** Wherever the host must stop the Player (an ordinary Stop, Pause or Toggle, the stop before a replacement, the render's stop on cancel or completion, a device-trouble stop) and `Player::stop()` returns an error without the runtime faulting (`Busy`, sequence exhaustion), the host records an owed Stop, keeps the phase that requires it (StopPending stays StopPending, Rendering stays Rendering), keeps `exclusive_ok` false and `work_outstanding` true, and retries the submission in every step. A retry that succeeds turns the owed Stop into a tracked Stop. If it is still unsent 2 seconds after it was first owed (by `mono_now`), the host sets `faulted` and takes the fault path (row R13). At the deadline, the host calls `Player::fault_runtime()` before R13 cleanup. This safe method publishes the existing runtime failure signal and device-failed flag (`RuntimeIdentity::fault()`, `P/plan_bridge.rs:277-278`, as device failure does at `P/player/device.rs:1307-1310`) without borrowing the owner or audio holder. Cleanup submits no new tracked work. Repeated stop requirements preserve the earliest unsent deadline; a successful submission satisfies the coalesced unsent requirement. An expired deadline is checked before retrying. The Player may still be playing then; `faulted` refuses every start and the user is told to reconnect the device (the existing fault message). Changing StopPending kind in C5, C6, C7, C12, V1, V2 or V5 submits a new Stop even when an earlier Stop is tracked or owed; failed submissions retain the earliest owed deadline.

**Timing.** Retries run on host logic ticks, including hidden or minimized-window ticks (eframe 0.36.2 runs `logic` while the window is hidden); outstanding work keeps requesting ticks. `mono_now` comes from an actual monotonic clock outside the lifecycle, never egui frame time. The deadline is enforced on the first tick at or after expiry; a blocked host call can delay that tick.

**Bounded held and deferred work (U-3).** Park holds at most one start: a newer start replaces the held one; the newer caller gets `Ok(Held)`, the replaced one was already answered, and the status line is unaffected. Resume defers at most 32 commands in order; one more is refused with `Err` and "The transport is finishing a take; try again in a moment." Every command insertion, including the remainder of a drain after a take-locate (today's unbounded `deferred.extend(reqs)`, `host/transport.rs:264`), uses the same checked 32-entry insertion and that refusal text. The wanted loop during a pass or Resume is a fact holding only the latest value, not a deferred command. A Stop, Reset, fault or the old Player's retirement empties both.

### 5. The state table

| Phase | Audible | State text | Record button | Commands with an effect |
|---|---|---|---|---|
| Stopped | no transport sound unless faulted (live instruments sound, E3) | none | off | all |
| StartPending | the sound before the group (nothing, or the old playback) | Play pending | off | starts join the group; Toggle, Stop, Pause close it; Record; Locate (cursor only) |
| Playing | playback | none (outcome "Playing") | off | all but Play |
| RecordPending | from stopped nothing; punch playback | Count-in when a count-in was sent, else Record pending | off | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| CountIn | the click | Count-in | pulses while the published counter runs (`transport.rs:193-197`) | the starts, Toggle, Stop, Pause, Record, HalfSpeed cancel; Locate take-safe |
| WaitingForClock | from stopped nothing; punch playback | Record pending (B1) | lit | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| Recording | playback and capture | none (outcome "Recording") | lit | Toggle, Stop, Pause, Record (take finalized); Locate (take-safe) |
| StopPending Park | playback until the audio thread applies Stop | Stop pending | off | one held start (a newer one, Toggle or Record replaces it); Stop sets return to zero; Locate sets the target; Pause drops the held start |
| StopPending Resume | as Park | Stop pending | off | Stop, Pause, Record drop the resume; ResetSession turns it into Reset; Locate retargets; AllNotesOff and Scrub at once; HalfSpeed refused; the rest deferred (up to 32) |
| StopPending Reset | as Park | none | off | Stop re-sends Stop; AllNotesOff is sent; Scrub applies; everything else has no effect |
| Rendering | the render's playback | none (the render dialog shows progress) | off | all refused with the render's text, except All Notes Off (cancels the render first, `finish/realtime.rs:164-171`) and Scrub |

### 6. Transitions and the record start's stage

The matrix is [tables.md](0062/tables.md), section A; it is total for this record's phases. Cells that differ from today carry their B or D item. Rules:

- **Player errors, in this order:** `PlayerError::Faulted` applies row R13. A failed Stop that did not fault becomes an owed Stop (section 4). After a call that cancels the ticket before it can fail (`Player::stop`, `locate`, `set_recording(false)` cancel first, `P/player/device.rs:319, 469-470, 475-476`) fails, the host reads the ticket's stage: `Cancelled` ends the pass (row O5); `Started` keeps it (a cancel cannot undo a started pass, `P/record/start.rs:133-134, 193-195`). Any other error keeps the phase and shows the error.
- **While `faulted`** every start refuses with the fault's text; finalization runs with Player errors ignored.

**The stage (S2b).** The record start publishes through its ticket-owned structure: one state word per runtime, the ticket's sequence plus flag bits, issued with `checked_add` (`P/record/start.rs:10-13, 117-125`), validated against the full ticket by `Reader::status()` (`:232-250`). S2b widens the flags from two bits to three (`FLAGS = 7`, sequences step by 8; the identity space halves from about 2^62 to 2^61, exhaustion still refused) and adds stages: `Pending`, `CountingIn`, `Waiting`, `Started(Point)`, `Failed(StartFailure)`, `Cancelled`.
- **Audio side:** each stage is published by one strong `compare_exchange` from the matching ticket's non-terminal state, never a plain store, never retried; a failed publication commits no transport effect. Advances are monotonic: Pending, CountingIn, Waiting, then a terminal stage. A terminal stage is its payload stores (release) followed by that compare-and-swap, as `Started` is today (`P/record/start.rs:158-165`).
- **Main's cancel** retries its strong compare-and-swap when it loses to a stage advance, at most three attempts (the longest advancing chain), and stops on a terminal stage.
- **Failure at its source:** the cancellations at `P/player/audio.rs:301` (speed), `:342` (the other `record_within` errors), `:386` and `:395` (`count_in`'s refusals) publish `Failed(reason)` when `StartFailure::of` gives a reason, else `Cancelled`. The `start_failure` atomic stores at `:949, 967` stay in S2b for today's consumer and are deleted in S2d with `take_start_failure()` (`P/player/device.rs:903-904`).
- **Readers:** acquire on the state word, read the payload, revalidate the state word. `Reader::status()` is authoritative for the pass; `Started(point)` is the latch for `Recorder::applied_start` and `Player::midi_recording_latch`; `Failed(reason)` gives the message once per ticket; `count_in()` is display only.
- **Consumers updated in the same commit:** `P/record/start/mapping.rs:68-74` and `P/record.rs:758-769` treat `CountingIn` and `Waiting` as `Pending` and `Failed` as `Cancelled`; `WriterStart::finished` (`P/record.rs:745-748`) finishes on `Failed` as on `Cancelled`, without waiting for an input mapping; the Started-only readers (`P/record.rs:606`, `P/player/device.rs:408, 424, 434, 446`) keep their latch-only behavior. Tests: S2b's row of the tests-that-change table.
- **Terminal stages** `Cancelled` and `Failed` end the pass in RecordPending, CountIn and WaitingForClock: resources finalized (`finish_recording`), destination by section 4.

### 7. Admission

**One synchronous entry.** Everything that changes the transport reaches the host through `Host::admit(cause)`, inside the change, before its caller's next guard:

| Path | How it reaches the host |
|---|---|
| engine commands: `run`, `execute_ui_merged`, `execute_with_mode` and the direct `engine.execute` sites (`actions.rs:446-449`, `groups.rs:412-423`, `mixer/actions.rs:344-353`, `import.rs:307`, `sampler_panel/instrument/state.rs:178`, `pad_actions.rs:11`, `sampler_panel/slices/{detect_panel.rs:199, state.rs:228, use_panel.rs:99, 104}`, `arrange/track_height.rs:66, 83-88`, keeping its nonrecursive height commit) | one `StudioApp::execute` wrapper calls `admit(Commands)` after `engine.execute` |
| control channel (`control.rs:102`), MIDI transport buttons (`midi_transport.rs:262`; the extra `handle_transport(0.0)` at `:270` is removed) | through `run` |
| ruler and empty-space locate (`actions.rs:180, 187`, today `locate_playing` directly) | `admit(Locate(at))`, keeping their own cursor and selection handling; during a pass this is now a take-safe locate (B14) |
| crash recovery (`host/autosave.rs:160` calls `E/autosave.rs:400`, which replaces the session and queues `ResetSession`, `E/lib.rs:488-490`) | `recover` calls `admit(Commands)` after the engine call |
| `run_or_ask`'s stop before a replace (`menus.rs:18-19`), the replace preparation (`frame_logic.rs:148-151`), `close_autosave`'s stop (`host/autosave.rs:114-116`) | `admit(Stop)` |
| render start (`finish/realtime.rs:240`) and cancel (`finish/realtime.rs:328`, from `finish.rs:300, 692`) | `admit(RenderStart)`, `admit(RenderCancel)` |
| Player installation (`splash.rs:129-136`) | `admit(Install)` |
| Player replacement (`host/reconnect.rs:212-255`, `audio_connection.rs:75-86`) | `admit(ReplaceRequested)`: preflight first; a refusal (`host/reconnect.rs:217-218`) keeps the phase and every fact (Td in RecordPending, CountIn, WaitingForClock, Recording and Resume). If stopping is required, the phase stays Park, Reset or Rendering with submitted or owed Stops; nothing becomes Stopped merely because replacement was requested. A failure before the old Player's retirement (`audio_connection.rs:77-81` can fail before dropping it) preserves all remaining facts and obligations. `admit(OldPlayerRetired)` when the old Player is actually dropped: finalize, retire everything, install Stopped |
| test setup that injects requests (`browser_tests.rs:819`, `builtin_editor_tests.rs:565-566`, `slice6_tests.rs:442`, `transport_tests.rs`, `audio_devices_tests.rs`) | edited to call the entry (S2c) |
| shutdown (`host.rs:70-78`, on exit) | `admit(Shutdown)`: closes autosave, finalizes resources, returns input; dropping the Player discharges every request and owed Stop |
| device trouble, lap limit, natural end, owed-Stop retries | inside the step (section 10) |

**Order inside one admission:**
1. **Supersede.** If the cause or the queue holds a `ResetSession`, `Stop` or `Pause`, or a `Record` while the phase is Resume: the held start and the deferred commands it cancels are discarded, and a pending Resume restart is disabled (Resume becomes Park, or Reset for `ResetSession`). A `ResetSession` also increments the reset generation. The ingress cutoff (section 14) is applied here. The Record request that cancels Resume is handled once and consumed in step 1; step 5 must not reinterpret it as a Park-held or Stopped Record. Preserve the pre-supersession operation and its Stop requirement. Execute each D-1 requirement exactly once despite the phase change before draining; a consumed Record must neither lose that Stop nor execute again in step 5.
2. **Drain.** Call `poll_notices` until it returns fewer notices than its buffer (64), then read the stop and release frontiers. With no new admissions during the drain, at most 576 notices can precede the short poll (256 ordinary actions, 256 notes, 64 session requests: `P/action_lane.rs:12-13`, `P/player/mod.rs:79`); a frontier may precede older action receipts (`P/action_lane.rs:435, 505-507`). Each receipt retires its fact; release effects (Park completion, held-start release, a Resume restart, deferred-command release) and all phase decisions requiring observed playback (a refused start's destination, a failed or cancelled start's destination) are queued, not run. Phase changes and deferred release require the current pass, group or render identity; older live facts retire and report diagnostics without changing the current operation.
3. **Refresh** playing and position; read the current ticket's stage.
4. **Run the queued effects** with the fresh observation: a Park completion parks at the fresh position, a released Record chooses punch or stopped start from fresh playback (`host/recording.rs:54`). Held work of an older reset generation is dropped.
5. **Apply** the remaining requests in order through the matrix; submit the Player requests; publish the capabilities. A nested request made by a transition (Return to Zero at Park completion) is applied inside that transition, without draining again.

**Multi-request commands** (`edit.play_timeline` queues Locate then Play, `E/cmd/edit_more.rs:132-133`) are applied in order with no rollback, as today: if the Play is refused or held, the Locate stays applied.

**Results.** A command returns `Ok(disposition)` or `Err(refusal)`. From its requests in order: `Err` if any was refused (the first refusal's text; earlier requests stay applied); else `Held` or `Deferred` if any was; else `Submitted` if a Player request was submitted; else `NoEffect`. The refusal text also goes to the status line, as today. Transport commands' replies gain `"transport": "submitted" | "held" | "deferred" | "no_effect"`. Held and Deferred describe admission at reply time, not guaranteed eventual execution; supersession or cancellation produces no second reply.

| Caller that depended on a queued `Ok` | Edit |
|---|---|
| control channel (`control.rs:102-104`) and MCP connected to the desktop app | none in code: `Err` already becomes `{"ok": false, "error": ...}`; the MCP `batch` tool (`crates/automation/src/mcp.rs:135-155`) stops at the first `Err` unless `keep_going`, so a refused transport command now stops a batch; `docs/control-protocol.md` says so. The engine-only `Headless` backend still clears transport requests (`crates/automation/src/lib.rs:52-54`) until the deferred headless record |
| MIDI button dispatch (`midi_transport.rs:260-266`) | an `Err` for an unarmed Record is its `refused` case, keeping "Record from <device>: no armed track"; other `Err`s continue as today |
| tests | `host/live_cycle_tests.rs:371, 387, 392` expect `Err`; tests that observed a queued request are in S2c's row |

**Reset generation.** The three readers that look for a queued `ResetSession` read the generation instead:
- `copied` is stamped with the generation when `copy_selected` writes it (`actions.rs:522-529`); `paste_at_cursor` (`actions.rs:538`) clears `copied` only when its stamp is older than the current generation.
- `sync_player_session` (`host/plugin_state.rs:16`) keeps today's `restore_pending` boundary (set when a Reset is admitted, cleared only when the reset's session is published, so a failed Stop admission does not clear it) and refuses while the newest reset's Stop is tracked or owed.
- `apply_editor_edits` (`host/plugin_state.rs:174`): when the generation changes, the host drains `Player::idle()` and drops every edit it returns before it accepts edits for the new document (`p.idle()` returns untagged edits, `:176, 193`); `restore_pending` keeps rejecting meanwhile.
- Deferred reset view clears carry their reset generation and cannot clear newer copied data. The old-editor drain barrier is satisfied only by a successful drain; a `Busy` or error result keeps it. In S2c, an explicit reset-preparation-pending marker preserves the publication barrier when Stop submission fails; successful reset preparation or the old Player's actual retirement resolves it.

### 8. Capabilities and the engine

The engine gains `capabilities: Option<Capabilities>` and `Engine::capabilities()`, which returns it or, with no host attached, computes it from `TransportStatus`. `StudioApp` attaches `Some(..)` at construction (the simulation included) and never clears it on reset or replacement, so no interface guard ever reads the fallback.

| Field | With a host | With no host (`from_status`) |
|---|---|---|
| `record_engaged` | RecordPending, CountIn, WaitingForClock, Recording, StopPending Resume | `recording` |
| `finalizing` | a take plan, a recorder take pending, a MIDI pass prepared, or record-off tracked | `recording` |
| `work_outstanding` | any tracked request, owed Stop, held or deferred command | the request queue is not empty |
| `exclusive_ok` | Stopped, nothing tracked, owed, held or deferred, not `finalizing`, not `faulted`, no render | not (`playing` or `recording`) |

`TransportStatus` is unchanged; the host writes `playing` as observed playback and `recording` as the Recording phase, published with `position` at every admission and transition. Engine guards read `capabilities()`: `blocked_while_recording` (`E/lib.rs:770`) reads `record_engaged`; `hardware_measurement` (`:414`), `measure_blocker` (`E/cmd/external_instrument.rs:45`) and `realtime_plan` (`E/cmd/realtime_render.rs:115`) read `exclusive_ok`. With no host the fallback gives today's answers, so the engine-only CLI and the engine tests that set the flags directly keep their expectations (`E/tests/recording.rs:8, 19`, `E/tests/hardware_insert.rs:165-167`, `E/cmd/external_instrument.rs:311-313`, `E/cmd/realtime_render.rs:208-213`, `E/tests/session_transport.rs:9, 23`). `transport.status` (`E/cmd/transport.rs:95`) adds a `capabilities` object; `ui.inspect` adds `record_engaged`; `docs/control-protocol.md` follows, keeping its `recording_pass` contract.

Host-only predicates: `running()` (not Stopped and not StopPending Park or Reset), `transport_active()` (not Stopped), `sync_allowed()` (no start, Locate, Record or record-off tracked, no owed Stop, no render in progress: the Rendering phase, or until S2e the `finish.realtime.running` interlock; Stop and release do not block, as `frame_logic.rs:365`). Every interface reader is assigned in [tables.md](0062/tables.md), section C. `recording_active()` is removed; each of its uses moves to the capability it means. Cleanup driven by `finalizing` is idempotent: a record-off that is only tracked never causes another `finish_recording()` submission.

### 9. Positions and synchronous effects

Inside an admission or a step the host keeps a working cursor and working positions; each request sees the effects of those before it.

| Effect | Side |
|---|---|
| cursor move by a locate or take-locate (`host/transport.rs:145, 268`) | synchronous (working cursor); `CursorTo` for the interface's copy |
| transport and retained positions (`host/transport.rs:52, 66, 146, 223, 260, 270`) | synchronous |
| a stopped cursor move from the interface (`lib.rs:537-539`) | synchronous: `set_cursor` calls `host.cursor_moved(at)` |
| selection after a placed take (`host/recording.rs:640, 712`) | synchronous: the host runs `edit.select` |
| Return to Zero at Park completion (`host/transport.rs:238-241`) | synchronous, inside the completion |
| undo squash of a Record operation (`host/recording.rs:466-478`) | synchronous |
| ResetSession's cursor clear (`host/transport.rs:199`) | synchronous |
| scroll to pickups, focus, take lanes, reset view clears, the Record hint, editor re-want, editor edits | deferred events |

**Park completion and the retained position (U-2):** exactly one of `Park.end` applies. `Target(at)`: retained := at, position := at, cursor := at. `ReturnToZero`: Return to Zero runs (the session start), and retained := the applied stop position read before parking (`host.rs:96-97`, as a first Stop records it today). `AtCursor`: retained := the applied stop position; position := the working cursor. PlayExact writes retained when admitted as a start in S, SP or P. Locate writes retained immediately in S, SP, P, RP, CI, W, R and Resume; Park applies its target at completion. Held or deferred commands apply their position effects when admitted, and refused or no-effect cells do not write retained. ResetSession writes 0; Continue reads retained.

### 10. The step

The step handles receipts, observations, timers, owed-Stop retries and device events, in today's `logic()` order:

| # | Block | Today |
|---|---|---|
| H1 | admission steps 1 to 4 (supersede, drain, refresh, queued effects); owed-Stop retries and their deadline | `frame_logic.rs:329`, `host.rs:82-114` |
| H2 | recording clock and journal; lap limit | `:335-337` |
| H3 | output trouble, input trouble, input loss (message only, the pass keeps recording, `host/reconnect.rs:57-64`), reopen | `:355-357` |
| H3r | render: result poll (section 11) | `:360` (`finish.rs:809-811`) |
| H4 | click and monitor pushes; the record stage; the Resume check | `host/transport.rs:139-167` |
| H5 | loop retarget; readback, natural end, adoption (O7); simulation timers; meters | `host/transport.rs:309-386` |
| after | automation write pass; session sync when `sync_allowed()`; autosave; input | `frame_logic.rs:364-375` |

MIDI transport loss (`midi_transport.rs:162-173`) says "MIDI transport input lost: stopping; press Play again after recovery." while its Stop is submitted or owed, replacing today's "stopped safely", which would claim a completion that has not happened. MIDI buttons run right after the control drain, before the step (D7): a MIDI Locate in the same frame as a lap limit is applied first, as a take-safe locate, and the lap-limit check then finds no Recording phase to stop. Buttons received during StopPending stay in the button queue, as today (`midi_transport.rs:224-227`).

**Held and deferred work.** Park's one held start (Play family, Toggle, Record) is admitted at completion; it is cancelled by a later Stop or Pause, by ResetSession, by a device reopen or the old Player's retirement, by a fault, and by the ingress cutoff. Reset discards held and deferred work. Resume's exceptions are Stop, Pause, Record and ResetSession (they drop the resume before the drain and discard its deferred commands; ResetSession makes it Reset), Locate (retargets), AllNotesOff and Scrub (applied at once), HalfSpeed (refused); everything else is deferred, up to 32. When all Stops required by the resume have retired, none is owed, the record-off is retired, fresh playback is false and no supersession disabled it, H4 submits the full Record start at the target; the deferred commands are released once per resumed request, on the first of: its admission receipt, a stage of its ticket, its refusal receipt, or a local refusal (which leaves Stopped with the finished take preserved and the reason shown). A Stop out of the resumed pass before its receipt or stage discards them instead, under U-3; the later receipt or stage then finds nothing to release.

### 11. The real-time render

- **Entry.** `admit(RenderStart)` requires `exclusive_ok`; `start_realtime_render` returns the render Play's `Sequence` (a synchronous `play()` error stays its `Err`). The phase becomes Rendering, job Live; the render Play is a tracked fact.
- **Stops are kept, not discarded (S2b, playback change).** The job keeps every Stop sequence it submits, from the moment of submission (today the capture-complete Stop at `P/player/device/realtime_render.rs:159-165` is sent before the unfinished-writer return at `:168-169`), and records a Stop it could not submit as owed. Every render poll reports newly submitted and unsent Stop obligations even when no result is ready. The host records the first-owed monotonic stamp on that poll and owns subsequent retries (section 4). The job retains previously submitted identities for cancellation reporting; repeated reports are deduplicated. Terminal result collection is independent of obligation reporting: `realtime_render_result()` returns the result when ready, and `cancel_realtime_render()` returns any obligations not yet reported and the writer's own result when the writer had already finished.
- **A refused render Play** disposes of the job through a path that removes the part files and releases the External Instruments without calling Stop (today's `stop_and_release`, `:136-141`, always stops).
- **Ordering, from what the code does.** A cancel admitted before the step's result poll takes the job (`slot.take()`, `:184`); a result collected first stands, and a later cancel finds no job: the job stays Retired with its collected result, and the phase waits for the render Stops' frontiers.
- **Cancellation result.** The host processes the cancel's returned obligations and optional writer result before allowing E5 or another admission. Result feedback may fill a cancelled Retired job's empty result without replacing its reason or Reset continuation. A render-Play refusal preserves any already retired payload and reports its refusal once. In Rendering, R13 and V3 cancel the render first and then retire every fact, including what the cancel returned: its Stops are retired, neither tracked nor owed (a reset, reopened or faulted device cannot acknowledge them), and a writer result is still collected and reported.
- **Leaving.** Rendering becomes Stopped when the job is Retired, the render Play and every render Stop are retired, no Stop is owed, and playback is not observed; then a pending Reset continuation runs.
- **Files, as today.** Cancellation removes unfinished part files (`P/realtime_render/writer.rs:269-271`); anything the writer had already committed (each file is synced and renamed when it finishes, `:261-266`, main then stems, `:319-321`) may remain. The message for a cancel caused by a session replacement is "Render cancelled: the session was replaced." and, only when the cancel returned a successful finished result, "The finished files were kept."; a reopen keeps today's text (`finish/realtime.rs:26`).
- **Fault and the blocking join.** No acknowledgment can arrive: the host cancels the job, which joins the writer (`P/player/device/realtime_render.rs:27-31`); the interface thread waits for that join. This record keeps that limitation (the deferred slow-calls record addresses it); Rendering stays owned and exclusive operations stay refused until the join returns, then the render facts are retired by the fault and the phase becomes Stopped with `faulted`.

### 12. Status text

- `StatusLine` (plain data, in the lifecycle module): `message: Option<Message { text, kind: Outcome | Note, rev, since }>`, `state: Option<State { text, rev }>`, `next_rev`, `message_rev` (bumped only by message writes), and `last_shown: Option<(u64, String)>`, updated from the line actually presented when it is not empty, with the epoch timestamp the caller supplies, and kept after expiry (today's `StatusAge::last`, `status.rs:49-53, 70`, which the diagnostics report reads). `set(text)` takes a new revision when the text differs; `say_again` (today's `StatusAge::restart`, #216; `import.rs:294`) takes one for equal text; `last_text()` returns the last written message. `StatusAge` ages only `message`.
- `state_text(phase, facts)` is the one producer of state texts: the phase's text over "All MIDI notes off pending" (release tracked).
- **Shown:** `message` when its `rev` exceeds `state.rev`, else `state`, else `message`, else empty.
- **Arbitration, once per transition, with the shown line before it as input:** (1) a new state text takes a fresh revision when the line showed a state text, nothing, or an Outcome; when it showed a Note, the state keeps the old state's revision, or 0 when there was none; (2) an outcome ("Playing", "Recording", "Stopped", "All MIDI notes off") is written as an Outcome only when the line showed a state text or nothing and no state text is live after the transition; (3) a transition with no outcome of its own clears an Outcome message, so "Playing" cannot reappear after a Reset.
- Writers that append to, read back, compare or return the status are treated one by one ([tables.md](0062/tables.md), section E); the rest change `app.status = x` to `app.status.set(x)` and `self.status.clone()` to `last_text()`.
- Twelve worked sequences are in section D, each a test.

### 13. Behavior changes and related defects

| # | Change | Slice |
|---|---|---|
| B1 | the recording flag only from the latch; the clock wait shows "Record pending" (`P/player/audio.rs:299, 340, 412`; `host.rs:349-355`) | S2d |
| B2 | StopPending Park and Reset are not a record operation; Resume is (`host/recording.rs:375-381`, `host/transport.rs:63`, `host/recording.rs:281`) | S2d |
| B3 | Count-in leaves only on the record start's stage (`host/recording.rs:149-154`, `host/transport.rs:165-166`): stage-driven phase ownership lands in S2d; stage-derived count-in presentation lands in S2f | S2d, S2f |
| B4 | the shown state is derived and arbitrated (`host.rs:343, 354, 380`) | S2f |
| B5 | replacement and reopen leave no pending text (`host.rs:280-282`, `host/reconnect.rs:107-112, 227-228, 245`) | S2f |
| B6 | a Record start no longer pulses Play; the plug-in loading override stays (`host/recording.rs:150-152`, `transport.rs:120-127`); Record's own request identity arrives in S2d, the display change in S2f | S2f |
| B7 | the speed invariant; the simulation honors speed; the half-speed Record refusal stays (`host/transport.rs:12-21, 289-292, 361`) | S2g |
| B8 | a take-locate keeps readback and pushes; meters still suppressed while Stop is pending (`host/transport.rs:139-142, 324`) | S2d |
| B9 | All public and interface readouts use one published playing/position pair, including `midi_monitor.rs:674`; admission freshness is groundwork supplied by S2c | S2g |
| B10 | the count-in plan lives in phase data (`host/recording.rs:154`, `host/transport.rs:58`) | S2d (payload), S2f (display) |
| B11 | one owner for the positions, no timing change (`midi_transport.rs:59`, `lib.rs:537-539`) | S2g |
| B12 | readers that newly block: the raw-flag readers block from RecordPending on (`actions.rs:11, 15`, `menus.rs:18, 402`, `E/lib.rs:770`, `host/timed_read.rs:44`); exclusive operations need `exclusive_ok`; the save, timed-read and plan-wait guards also wait for a render (tables C) | S2d |
| B13 | a command returns `Ok(disposition)` or `Err(refusal)` at once (today `Ok` and a later status message) | S2c |
| B14 | ruler and empty-space clicks use C8 admission: take-safe during a pass (the pass is finalized and resumed at the click), updating the retained position outside a pass, suppressing the Player Locate while StartPending, and refused during Rendering; today they call `set_cursor` and `locate_playing` directly, which write no retained position (`actions.rs:179-187`, `lib.rs:532-539`, `host/transport.rs:84-99`) | S2c |

| # | Today | Instead | Slice |
|---|---|---|---|
| D1 | Play or Toggle during a clock wait from stopped restarts playback and drops the record start (`host/transport.rs:218-219, 246-249`; `P/player/audio.rs:452, 417-418`) | Play ignored; Toggle cancels | S2e |
| D2 | PlayExact and PlaySelection have no pass guard outside an observed count-in (`host/transport.rs:222-224, 273-279`) | ignored during RecordPending, WaitingForClock, Recording; PlayExact in CountIn cancels | S2e |
| D3 | a session replaced during a take-locate attempts the resume on the new document (`host/transport.rs:139-148`) | Reset supersedes Resume, before any receipt releases work | S2c |
| D4 | starts during Stop pending depend on the moment's observations and can coexist with the Stop (`host/transport.rs:21-23, 218, 246-256`) | one start held, released at completion; Toggle counts as a start | S2d |
| D5 | Locate or a loop change during Stop pending sends a request to a stopping Player (`host/transport.rs:85-92, 128-133, 268-270`) | sets the park target; nothing sent | S2d |
| D6 | Pause while stopped sends a Stop (`host/transport.rs:244`) | no effect | S2g |
| D7 | MIDI buttons run in the drawing pass after the write pass, lap limit and autosave (`frame_logic.rs:337, 363-372, 467`) | after the control drain, before the step, through the same admission | S2c |
| D8 | ResetSession during a render sends Stop under it (`host/transport.rs:176-184`) | the render is cancelled first, with its own message | S2e |
| D9 | a second Stop while stopping returns to zero only if the moment's observations say stopped (`host/transport.rs:228-241`) | always at completion, unless a later Locate set a target (U-2) | S2d |

### 14. Relation to ADR 0043

| Requirement | Here |
|---|---|
| 1. sync mode changed only stopped and finalized | `exclusive_ok` (S2d) |
| 2. Start/Continue/Stop/SPP with its own start pending | start groups, `next()`, the matrix; ADR 0043 adds its armed states by amendment |
| 3. duplicate suppression against a pending locate | each tracked Locate keeps sequence and target; ADR 0043 adds the applied epoch and source generation |
| 4. record engaged, "Waiting for external start" | ADR 0043's state under the checklist below; B1 is its precondition |
| 5. sticky local Stop | ADR 0043's mechanism at ingress admission (ADR 0043:534-537). The host applies the ingress cutoff (`IngressCutoff { generation, offered_sequence }`, kept distinct) in step 1 of every admission, before any held or deferred command is released |
| 6. half speed and scrub refused in clock follow | ADR 0043's condition on those rows; a refusal leaves phase, speed and position unchanged |
| 7. readout | ADR 0043's, through `TransportStatus` and `capabilities()` |
| 8. applied moments for sending | ADR 0043's callback-published spans (ADR 0043:319); the lifecycle's transitions are not used for sending |
| 9. slice 3 refusals, tempo, journal | unchanged |
| 10. a deterministic matrix without a window | the pure matrix (S2a) and the device-free step (S2b) |

**The matrix is total for this record's phases only.** ADR 0043 adds its states by amendment. Each added state must define: its payload and the resources it owns (a prepared ticket, a start boundary); its row for every existing event and column for every existing phase; which requests it tracks and how each retires; its capabilities (`record_engaged`, `finalizing`, `work_outstanding`, `exclusive_ok`); its state text; its observation and error precedence (stage, receipts, ingress); how it uses the one-held-start and 32-deferred bounds; and its cancellation on local Stop, selected-source Stop, ingress overflow or loss, source replacement, session replacement, device failure and preparation failure. Input from round 2: (1) Record engaged locally, waiting for an external Start (holds a prepared ticket); (2) an external Start or Continue received, waiting for its first following clock tick (holds the start message's boundary); (3) a submitted recording action (RecordPending).

**"Opening a session always starts stopped"** (ADR 0043:85) holds because a Reset supersedes in step 1 of every admission, disabling a Resume restart and discarding held and deferred work before any receipt can release it, and because the reset generation keeps the synchronization and editor-edit barriers (section 7). No ADR 0043 decision becomes false.

### 15. Relation to ADR 0052

ADR 0052 relies on the audio thread (decision 8 and its 2026-11-16 amendment: "The count-in bar keeps its full length in output frames"), `count_in_for`, `plan_recording` writing `count_in`, and `recording_pass`. None changes: the stage reports what the audio thread does, and moving failure publication to its source changes no timing.

## Deferred to later records

The settled direction, without detailed promises. Each later record decides its own design and slices.

- **The host crate and the move.** The lifecycle module and the rest of `host/` move into a `transit-host` crate at layer 5 with no interface dependency. That record decides the crate's API, the clock capability that replaces the lifecycle's time parameters, how view effects cross the boundary, how tests reach the host, and the order of the move.
- **Service handles and an unnameable Player.** The interface reaches the Player only through narrow host-owned handles and loses its normal dependency on `transit-playback`. That record decides the handles and their methods, the connection traits' new home, the data re-exports, the public-surface audit and independent compile-fail probes.
- **Headless play.** The automation backend drives the same host with a null Player. That record decides stepping across both batch layers, replies, recording refusal, file confinement and shutdown.
- **Slow calls off the interface thread.** Device reopen, input open, the control-channel latency measurement, the timed autosave flush, device listing, and the render writer's blocking join on a fault (section 11). That record decides, per call, stepping, a worker for work that does not hold the Player, or a visible staged wait.

## Slices

Every commit builds; every slice runs the full local gates and is shippable alone. Assertions about what is heard, where a take lands and the record latch never weaken; a test that observed the old mechanism is edited to observe the new one and is listed. Sizes are changed lines including tests.

| Slice | Changes | Depends on | Size |
|---|---|---|---|
| S1 | read-only `transport_phase()` in `host/lifecycle/` from today's data plus request metadata (play start, record from, count-in, ticket, record-off kind) and a persisted stop reason (`User | Locate | Reset`), so a pending Reset is never read as Park; `classify` and `frontiers` behind wrappers with today's text and flag rules. Derivation precedence: active render; pending Stop with its persisted reason; current prepared pass (Started latch, observed count-in, admission pending, otherwise clock wait); pending Play group; observed playback; Stopped. Record-off alone is not a prepared pass. MIDI-only preparation exposes a clone of its ticket-owned Reader (today private, `P/player/device.rs:369, 402-410`), as audio preparation already does | none | about 550 |
| S2a | the pure reducer `next(phase, event, facts)`, section A as a literal table, unused; `transport_phase_matrix`, `the_matrix_has_no_empty_cell`, `stopped_implies_nothing_tracked` | S1 | about 2,200 (built; the tests keep an independent copy of the table and the code is laid out vertically) |
| S2b | groundwork, no behavior change beyond its tests: `step_host` and `device_free_app`; the render's kept Stops, nonterminal obligation reporting on every poll, refused-Play disposal, one render orchestration function; the stage publication with its consumers; `Player::fault_runtime()` (unused until S2d) | S1 | about 1,550 |
| S2c | the admission boundary with today's meanings: the one entry and every path of section 7, the five-step order, the reset generation and its barriers, the admission refresh, dispositions (B13), B14, D3, D7; and S2d-0, the mechanical capability accessors (`engine.capabilities()` and the interface's readers call accessors that return today's answers; each accessor preserves its reader's original predicate: the raw recording flag and `recording_active()` are not interchangeable) | S2b | about 1,300 |
| S2d | the activation commit, atomic: request ownership and owed Stops, the phase, the capabilities' new answers in every safety reader and the engine guards, terminal stages, owed Stops and `fault_runtime()` at the deadline, Park and Resume rules with their bounds, count-in payload ownership, stage-driven phase changes; B1, B2, B3 (phase), B8, B10 (payload), B12, D4, D5, D9 | S2a, S2c | about 1,700 |
| S2e | the render as a phase; D1, D2, D8 | S2d | about 650 |
| S2f | display arbitration and count-in reporting; the status writers; B3 (presentation), B4 to B6, B10 (display) | S2d | about 1,450, about 400 of it mechanical |
| S2g | speed, positions, readouts; B7, B9, B11, D6 | S2d | about 700 |

**S2d's activation.** The accessors land first (S2c's S2d-0 part), so S2d changes answers, not call sites. The activation commit is atomic: the phase, the request facts and the capabilities together define "a pass is under way". It lands only when: its files are first split by concern (requests, reducer, readers, guards); the request facts, the reducer and the readers are reviewed separately and then together; its tests are adversarial integration tests (interleavings of receipts, stages, Stops, faults and resets on a device-free Player), not copies of the matrix's expectations; every audible, take-placement and latch assertion is preserved; the full gates pass on a quiet machine on both platforms; and an independent refutation review signs it off. The literal reducer contains the final cells; until their slices, S2d keeps today's behavior by four selectors, each false in S2d and set by its slice, with these literal compatibility rules:
- `pass_guards=false` (until S2e): WaitingForClock's Play and Toggle use today's observed-playing and pending-start conditions; a permitted Play is tracked while the pass phase and resources remain owned. PlayExact in RecordPending, CountIn, WaitingForClock and Recording submits a tracked Play while retaining the pass phase; PlaySelection does likewise in RecordPending, WaitingForClock and Recording and cancels in CountIn. The resulting cancellation stages finalize the matching old pass; Play receipts cannot erase it.
- `speed_invariant=false` (until S2g): today's speed changes are preserved, including the normal-speed simulation; the final phase and speed invariant is not enforced.
- `pause_noop=false` (until S2g): stopped Pause enters StopPending Park with `end=AtCursor`, submits Stop or owes it under W1, and completes under R6.
- `render_as_phase=false` (until S2e): the existing render-running interlock is kept.

The reachable compatibility combinations are tested as well as the final cells.

**Tests that change, and why:**

| Slice | Test | Why |
|---|---|---|
| S2b | `P/player/audio/tests/count_in.rs` (`:72` loops while `Pending`; the count-in and failure cases), `P/player/audio/tests.rs`, `P/player/audio/tests/speed.rs`, `P/player/audio/midi/tests.rs`, `P/record/tests.rs` | they observe `Status::Pending` during a count-in or wait, or `Cancelled` after a start failure; they now observe `CountingIn`, `Waiting` or `Failed(reason)`; their latch, timing and audible assertions stay |
| S2b | `P/record/start.rs` tests: the exhaustive match at `:464-467`; the last-valid-ticket fixture at `:489` (`u64::MAX - 7` no longer issues a ticket when sequences step by 8) | new stages; the wider flag field |
| S2b | `P/player/device/realtime_render_tests.rs` | the render calls return sequences and kept Stops |
| S2c | `host/live_cycle_tests.rs:371, 387, 392` | a refused Record or HalfSpeed returns `Err` (B13) |
| S2c | `browser_tests.rs:819`, `builtin_editor_tests.rs:565-566`, `slice6_tests.rs:442`, `transport_tests.rs` (2 sites), `audio_devices_tests.rs` (1 site) | they inject or inspect `engine.transport_requests`; admission is synchronous |
| S2c | any test that asserted nothing happened between `run` and the next frame | the change now happens at `run`; the builder lists each with its reason |
| S2d | `player_tests.rs` (8) | `notice` and `recovery` give way to `classify` and `frontiers`; same cases |
| S2d | the `recording_active()` test call sites: `host/live_cycle_tests.rs` (7), `midi_record_tests.rs` (7), `midi_transport/tests.rs` (5), `slice6_tests.rs` (5), `transport_tests.rs` (3), `audio_devices_tests.rs` (1) | removed helper: "a pass is under way" reads `record_engaged`; "a take is still to place" reads `finalizing`; "nothing records" reads both false |
| S2d | the 23 sites in 10 interface test files that set `engine.transport.recording = true`; `finish/realtime/tests.rs:158-160` (sets `playing` to exercise the render guard) | the interface's guards read the attached capabilities; they set the phase through `host::testing` |
| S2d | `host/state_guard_round3_tests.rs:31-34`; `midi_transport/tests.rs:138`; `host/live_cycle_tests.rs:408` | a pending Play set directly; the simulated count-in (no longer reachable); the untagged failure atomic, now a ticket `Failed` through a test hook |
| S2f | `status.rs:147`; `slice6_tests.rs:404, 422, 455, 462-463`; `transport_tests.rs:317`; `plugin_retry_tests.rs:330` | state texts are no longer in the message channel |
| S2e, S2g | none known | the builder lists any it finds, with its reason |

Engine tests that set the flags directly keep their expectations through the no-host fallback (section 8).

**Gates (new tests).** S1: six derived-phase tests and `state_text_names_the_waits`. S2a: the three reducer tests. S2b: `the_render_returns_every_stop_it_submitted`, `a_render_stop_that_could_not_be_sent_is_owed`, `a_refused_render_play_disposes_without_stop`, `the_record_stage_follows_count_in_wait_start_fail_and_cancel`, `a_late_stage_write_never_overwrites_cancelled`, `main_cancel_wins_over_each_stage_advance` (deterministic interleavings), `stage_exhaustion_is_refused`, `failed_finishes_the_writer_start`, `step_host_matches_logic_for_the_transport_part`. S2c: `admission_is_synchronous_with_todays_meanings`, `ruler_locate_admits_synchronously`, `a_ruler_click_during_a_pass_is_take_safe` (B14), `recovery_admits_its_reset`, `a_reset_disables_a_resume_restart` (legacy Resume cancellation), `admission_refreshes_playing_before_deciding`, `a_ruler_click_outside_a_pass_updates_the_retained_position`, `a_ruler_click_while_start_pending_sends_no_player_locate`, `a_ruler_click_during_a_render_is_refused`, `locate_then_play_keeps_the_locate_when_play_is_refused`, `copies_made_after_a_reset_survive_the_first_paste`, `old_editor_edits_are_dropped_after_a_reset`, `a_midi_locate_before_a_lap_limit_resumes_the_pass` (D7). S2d: every B and D test of the slice; `reset_discards_held_work_before_receipts_release_it`, `released_record_uses_the_fresh_playing_sample` (Park-held), the compatibility-selector tests, `an_owed_stop_coalesces_and_keeps_the_earliest_deadline`, `the_deadline_calls_fault_runtime_before_cleanup`, `a_replacement_request_does_not_stop_the_phase`, `shutdown_discharges_every_request`, `an_old_ticket_receipt_does_not_change_the_resumed_pass`, `record_during_resume_discards_the_deferred_commands_and_is_neither_held_nor_started`, `record_then_undo_in_one_drain_refuses_the_undo`, `record_then_measure_in_one_drain_refuses_the_measurement`, `play_a_then_record_retires_a_without_a_phase_change`, `a_stage_before_the_record_receipt_moves_the_phase`, `started_does_not_retire_record_and_the_receipt_never_regresses`, `a_failed_start_goes_where_the_transport_is`, `a_stopped_group_does_not_lend_its_applied_bit`, `a_fault_retires_every_request`, `an_owed_stop_is_retried_then_faults_after_two_seconds`, `a_replacement_refusal_keeps_every_fact`, `a_newer_start_replaces_the_held_one`, `the_thirty_third_deferred_command_is_refused`, `locate_after_second_stop_parks_at_the_target`, `second_stop_after_locate_returns_to_zero`, `close_autosave_keeps_a_finalizing_take`, `finalizing_cleanup_never_resubmits_record_off`, `record_off_tracked_keeps_exclusive_ok_false`, `input_loss_keeps_recording`, `stopped_implies_nothing_tracked` in the integration tests, an integration gate for section 3's no-observed-playback requirement checked after every complete admission and step, already-Stopped and retirement paths (R13, V3, V4b) included (speed verification is S2g's), Resume with audio and MIDI, deferred commands and an intervening edit, the journal handoff interrupted at each step (`host/autosave.rs:72-83`), host steps that miss a whole count-in. S2e: render ownership, fault, cancel-versus-collected, owed render Stop and session-replacement tests. S2f: the twelve display sequences; `the_lap_limit_note_never_copies_a_state_text`; `last_shown_tracks_the_presented_line_and_survives_expiry`; `plugin_browser_compares_the_message_revision`. S2g: B7, B9, B11, D6 tests.

**Feel-right items:** S2d: at Stop after a take the Record light goes off at once; Space twice quickly restarts playback once the stop completes. S2f: Record from stopped with a one-bar count-in: Play does not pulse; Count-in, then Recording; Stop pending, then Stopped; Reconnect while playing leaves no Stop pending.

## Consequences

- One value answers what the transport is doing, a render included; every request has an owner until it is retired; a Stop that could not be sent is owed and retried, then faults.
- Commands see each other's effects at once; a refused transport command fails its call; `Ok` carries what happened.
- `recording` means capture; engine guards read named capabilities, with today's answers when no host is attached.
- Held and deferred work is bounded and named.
- The lifecycle has no window types and can move into a crate later; the interface still owns the Player, and a fault during a render still waits for the writer's join.

## Alternatives rejected

- **The whole host in one record** (rounds 1 to 3): each round found new depth; the lifecycle alone is what sync needs first.
- **Request identities in the phase** (round 2's revision): receipts arriving in another phase had no owner.
- **Queued admission** (round 1's revision): commands in one drain evaluated their guards before the earlier transition.
- **A truncated ticket in a new atomic** (round 1's revision): loses the engine epoch and the exhaustion refusal.
- **The render left on the observed flag** (round 1's revision): its requests or job could be unsettled while the phase read Stopped.
- **A failed Stop left as a no-op** (round 3's revision): Rendering or StopPending could wait forever with nothing owed; the owed-Stop rule bounds it.
- **Unbounded held and deferred queues**: the lane bounds submitted requests, not commands waiting to be submitted.
- **Inferring the count-in end from the counter or elapsed time**: misses whole count-ins.

## Judging round 1: responses

Every round-1 finding was verified at `main` 1a56b895 and accepted; none was rejected. Rounds 2 to 4 re-audited them; the ones they found unresolved are answered in the later sections.

## Judging round 2: responses

Every round-2 finding was verified and accepted (none rejected): start groups, terminal stages, input loss, Reset versus Park holding, one deferred-release point, the park target, record-off as a fact, the simulation scoped to Stopped and Playing, StartArmed out of the matrix with a checklist, render substates, the E1 guard, the save and capture readers, synchronous admission, display revision 0 and live states over outcomes, the non-mechanical writers, the stage through the ticket, render receipts, the slice re-cut, `recording_active()`, ADR 0043 mapping 5, B12 and D4 to D8, and headless (now deferred).

## Judging round 3: responses

Every round-3 finding was verified and accepted (none rejected): request ownership independent of the phase; failure at its source and the CAS rules; render cancellation against files, without promises beyond today's behavior; one rule for Reset, Resume, Scrub and deferred release; changed tests listed; the render in the synchronization guards; outcome clearing; every admission path; the drain boundary and Reset before release; multi-request commands; dispositions and their callers; the engine fallback; `Scrub` scoped; start groups and fault retirement; errors after cancellation; the 16-start limit removed; stage consumers; reset-generation readers; the diagnostics accessor and the extra writers; S1's reset intent; the earlier admission cut; the ADR 0043 checklist. The nested MCP batch and the render-measurement methods went with their deferred records.

## Judging round 4: responses

| Finding | Disposition |
|---|---|
| 1, 2: Record retirement stated two ways | Accepted. One rule: the action receipt retires Record, or early a `Failed` or `Cancelled` stage; `Started` does not, and the later receipt never regresses the phase (section 4). |
| 2: `SetLoop` missing | Accepted. Verified `P/player/device.rs:315-316`. A tracked fact retired by its receipt, fault or the old Player's retirement (section 4). |
| 2: "no-op" versus untracked refusals | Accepted. "No lifecycle effect", refusal shown once by request identity (section 4). |
| 2: replacement requested versus retired | Accepted. Verified `host/reconnect.rs:217-218`. Split admission causes; rows V4a and V4b (section 7; tables A). |
| 2: multiple Locates | Accepted. Each Locate is tracked separately. |
| 3: `arrange/track_height.rs:66` | Accepted. In the wrapper inventory, keeping its nonrecursive commit. |
| 3: drain bound | Accepted. 256 ordinary, 256 notes, 64 session requests; verified `P/action_lane.rs:12-13`, `P/player/mod.rs:79`. |
| 3: release before refresh | Accepted. Release effects are queued during the drain and run after the refresh (section 7, steps 2 to 4). |
| 3: supersede Resume itself | Accepted. Step 1 disables a pending Resume restart. |
| 3: reset generation (copies, restore boundary, editor edits) | Accepted. Verified `actions.rs:522-529, 538`, `host/plugin_state.rs:16, 174-176, 193`. Section 7. |
| 4: Reset's exceptions | Accepted. Text and table agree: Stop re-sends, AllNotesOff is sent, Scrub applies, the rest has no effect. |
| 4: Park target versus return to zero | Decided by U-2 (the last command wins); section 9 and the R6 cell agree; both orders tested. |
| 4: render `Retired` payload | Accepted. `Retired { result, reason, then_reset }` (section 3). |
| 4: walk (d) under D7 | Accepted. Section 10 states the order; test `a_midi_locate_before_a_lap_limit_resumes_the_pass`. |
| 4: walk (e) driver reset inherits fault retirement | Accepted. Row V3 retires every fact like a fault. |
| 4: walk (g) | Resolved by U-2: with Locate after the second Stop, the target wins. |
| 4: ruler clicks untagged | Accepted. B14 with its test. |
| 5: Failed at source sites | Accepted. `P/player/audio.rs:301, 342, 386, 395`; strong `compare_exchange`, no transport effect on a failed publication (section 6). |
| 5: consumers `record.rs:747`, `start.rs:464-467`, `:489` | Accepted. Verified; in section 6 and S2b's tests. |
| 6: `StudioApp` attaches capabilities; MCP qualified | Accepted (section 8; section 7 caller table). |
| 7: render Stops kept across polls and cancellation; refused Play disposal; "finished files" only for success; blocking join stated | Accepted. Section 11. |
| 7: failed render Stop | Decided by U-1 (owed Stop with a 2-second deadline), referenced from the render rows. |
| 8: autosave close conflates two meanings | Accepted. Requesting Stop reads `record_engaged`; retention reads `record_engaged || finalizing` (tables C). |
| 8: `midi_transport.rs:271, 283`; idempotent cleanup | Accepted (tables C; section 8). |
| 9: plug-in browser revision; `last_shown` from the presented line | Accepted (section 12; tables E). |
| 10: guard tests in S2c | Accepted. Moved to S2d; S2c tests synchronous admission with today's meanings. |
| 10: activation of B6, count-in payload, D1, D2, B7, D6 | Accepted. Selector facts in S2d; payload in S2d, display in S2f. |
| 10: missing tests `P/record/start.rs`, `finish/realtime/tests.rs:158-160` | Accepted (tests-that-change table). |
| 10: 1,800 lines is not uncuttable | Accepted. Accessors land first (S2d-0 in S2c); the activation commit's review conditions stated. |
| 11: ADR 0043 bounds | Accepted (section 14). |
| 12: three decisions | U-1, U-2, U-3 applied (sections 4, 9, 10). |

## Judging round 5: responses

Round 5's verdict was accept with changes. Each of its eighteen edits was checked against `main` 1a56b895 before it was applied; all were right as stated, and all are applied.

| Edit | Applied where |
|---|---|
| 1. playback groundwork inventory | section 1; S1 and S2b rows. Verified: `Player` holds `failure_signal: Option<RuntimeIdentity>` (`P/player/device.rs:75`) and the shared fault flags, so `fault_runtime()` needs no owner borrow |
| 2. owed Stop deadline calls `Player::fault_runtime()`; coalescing; deadline checked before retry | section 4, owed Stops. Verified `P/plan_bridge.rs:277-278`, `P/player/device.rs:1307-1310` |
| 3. render polls report nonterminal obligations | section 11; tables A E3a, E3 |
| 4. C5 in Stopped submits Stop; R6, H2 completion precondition; W1 for every stop continuation | tables A C5, R6, H2, W1; section 10. Verified today's `stop_play` at `host/transport.rs:233` |
| 5. replacement requested keeps Park, Reset or Rendering until retirement | section 7 replacement row; tables A V4a. Verified `audio_connection.rs:77-81` |
| 6. shutdown retirement | section 4 and the section 7 inventory. Verified `host.rs:70-78`, `P/player/device.rs:1112-1121` |
| 7. timing wording; Stopped's audible cell | section 4 (Timing); section 5. Verified `Cargo.lock` selects eframe 0.36.2 |
| 8. ReturnToZero branch restores the saved stop position | tables A R6 |
| 9. Record during Resume cancels before the drain; L1 keeps a fact; checked 32-entry insertion | section 4 (bounds), section 7 step 1, section 10; tables A C7, L1. Verified `host/transport.rs:264` |
| 10. observation-dependent decisions queued; step 3 reads the stage; current-identity rule | section 7 steps 2 and 3; tables A receipt and observation preambles |
| 11. generation on deferred view clears; editor drain barrier; S2c reset-preparation marker | section 7, reset generation |
| 12. autosave retention captured before Stop | tables C |
| 13. S1 derivation precedence; MIDI-only Reader exposed | S1 row. Verified the reader is private (`P/player/device.rs:369, 402-410`) |
| 14. accessors keep each reader's predicate; two Park-held tests moved to S2d | S2c row; gates S2c and S2d |
| 15. literal compatibility rules for the four selectors | S2d's activation; tables A legend |
| 16. B3 split between S2d and S2f | section 13; S2d and S2f rows |
| 17. B14 expanded; C13 tags; tests | section 13; tables A C13; S2c gates. Verified `actions.rs:179-187`, `lib.rs:532-539` |
| 18. Held and Deferred describe admission only; MIDI-loss wording | section 7 results; section 10. Verified today's text at `midi_transport.rs:173`; no test pins it |

Beyond the eighteen: section 2's shutdown inventory and section 6's walks were carried into the tables as stated; the hidden-window observation (eframe runs `logic` while hidden) is recorded in section 4's timing paragraph.

## Judging round 6: responses

Round 6 (confirmation) accepted with six changes; each was checked against `main` 1a56b895 and applied.

| Edit | Applied where |
|---|---|
| 1. R6 in Reset waits for every required Stop, none owed, fresh playback false | tables A R6 |
| 2. V4a keeps the phase and obligations while replacement proceeds; retirement does not wait for Stop acknowledgment | tables A V4a. Verified: today `replace` follows the Stop request without waiting (`host/reconnect.rs:227, 244`); the old Player is dropped at `audio_connection.rs:84` |
| 3. the Record that cancels Resume is consumed in step 1 | section 7 step 1; tables A C7; test renamed to assert it is neither held nor started |
| 4. only an older ticket's Failed or Cancelled stage retires its Record fact | tables A observation preamble |
| 5. stopped Pause in compatibility enters StopPending Park | S2d's compatibility rules; tables A W1 and legend |
| 6. B9 activates once, in S2g | section 13 |

## Judging

## Related

[ADR 0043](0043-hardware-sync-transport-and-clock.md), [ADR 0052](0052-pre-count-zone.md), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md), [ADR 0018](0018-plugin-host-thread.md), [ADR 0026](0026-cycle-recording-stacks-takes.md), [ADR 0027](0027-external-instrument-real-time-render.md). Issues #739, #733, #736.

## Amendment (S2a, 2026-11-17): the matrix made exact

Found while encoding section A as a literal table (slice S2a); a judging round ruled on each reading, and the coordinator settled its open points (D-1 to D-3 keep today's behavior; D-4 came from a second judging round, D-5 and D-6 from the reachability closure).

- **D-1.** A change of StopPending kind (C5, C6 and C7 in STr; C12 in STp and STr; V1, V2 and V5 in STr) always submits a new Stop, even when an earlier Stop is tracked or owed; a failed submission keeps the earliest owed deadline (section 4). Today `stop_play` always submits one, and a Resume whose Stop already retired would otherwise wait for a frontier that never comes.
- **D-2.** Count-in payloads follow a preparation contract: `from=None` is a punch, `count_in=Some` requires `from=Some`, and the host preserves the pairing; the reducer leaves inconsistent input unchanged (section 3; R3 and O1 in RP).
- **D-3.** E1 in RND refuses with today's text, "A real-time render is already running." (`finish/realtime.rs:191-192`).
- **D-4.** A failed, refused or cancelled Record goes to StartPending whenever the current start group still has an outstanding member, regardless of `applied`; Playing only with no outstanding member and playback observed; else Stopped (section 4; the "by 4" rule of R5, O4 and O5 and the legend). Counterexample under the old order: in RP with one Play applied and ended and another still outstanding, the Record fails before its audio action and "by 4" chose Playing from the `applied` bit;
  O6 in Playing then went to Stopped without neutralizing the outstanding Play, which could still execute, against section 3's invariant.
- **D-5.** Playing never holds an outstanding current-group start (section 3). Proven over what the table reaches: the tests' reachability closure from Stopped, over a finite abstraction of the phase, the tracked, owed and group facts and what can still execute, reaches a fixed point of 35,556 states and 7,048,964 transitions (with admission step 1's supersession modeled). On every one it establishes D-5 and Stopped's request side of section 3's invariant (unless faulted, nothing still capable of moving the transport, no Stop tracked or owed, no held start and no deferred commands); playback is checked only on entering Stopped from another phase without a fault or a full retirement. It does not establish a resident Stopped with observed playback, the readback after V3 or V4b (S2d's integration gate) or the speed invariant (S2g). The closure's saturated `starts > members` comparison holds under its reachable-state qualification (capable closed starts coexist with no current group until a Stop neutralizes them).
- **D-6.** In Rendering, R13 and V3 cancel the render before their every-column retirement, which covers what the cancel returned (section 11). Path found by the closure under the old order: Stopped, E1 to Rendering with job Live;
  V3 there retired every fact first, then the cancel's returned render Stops were tracked or owed, in Stopped.
- D-6 depends on section 11's return contract for a render cancel, which S2b must implement: today `cancel_realtime_render()` returns `()`, discarding both the Stops and the writer's result.

Cells and sentences changed, and why:

- C5, C6, C7, C12, V1, V2, V5 (the cells above): "submit Stop or owe it by W1" added (D-1).
- C7 SP and P: "(Ta, Te)" added; every new Record is prepared first, as today (`host/recording.rs:19-42`).
- C2 S and C3 S: "(Tr)" added; both start through `start_play`, which refuses during a reopen (`host/transport.rs:12`).
- C2 SP and P, C8 RP to R and STr: "retained := at" added, as section 9 now states it.
- R3 RP and O1 RP: the count-in payload contract (D-2).
- O6 W: only a punch (`from` absent) ends the pass; a stopped clock wait was never playing.
- E1 RND: the duplicate-render text (D-3).
- W1 S: "—"; the compatibility selectors belong to S2d (legend).
- C12 RND: then_reset is set whether the job is Live or already Retired.
- E2, E3 and R2 in RND: a cancel's returned writer result is fed through E3, which may fill a Retired job's empty result; a render-Play refusal keeps a retired payload and shows its refusal once (section 11).
- R6 STr and R7: the Resume check is scheduled at H4 and rechecks its precondition (section 10).
- R13 and V4b: the common cleanup stated for every column.
- Legend: the cells are the final reducer; leaving StartPending or Playing sets speed Normal (section 3).
- Section 3: the invariant names what can still move the transport (an acknowledged Stop neutralizes earlier actions without retiring their facts, section 4); the count-in contract; the render outcome summary and `CancelReason::Fault`.
- Section 9: where PlayExact and Locate write retained.
- Section 11: the cancellation result.
- Slices: S2a's size is the built figure.
- Section 4 and the legend's "by 4" (used by R5, O4 and O5): the failed-start destination order (D-4).
- Implementation line: S1 landed and S2a built, replacing "not started".
- Section 3: the D-5 invariant beside the Stopped invariant.
- Section 7, admission step 1: the pre-supersession operation and its Stop requirement are preserved (D-1 exactly once).
- Gates: S2d's integration gate for no observed playback in Stopped.
- V1 and V2 in STr: "deferred discarded" added, applying U-3 (section 4: "A Stop, Reset, fault or the old Player's retirement empties both."); the device-trouble Stop out of Resume otherwise left the deferred commands stranded in Stopped. V5 already clears them in its heading.
- Every `STp!` cell of RP, CI, W and R (C1, C2, C3 and C9 in CI; C4 to C7, V1 and V2 in RP to R; T1 in R; V5 already clears them in its heading): "deferred discarded", applying U-3 to every Stop out of a pass. Path found by the closure: Stopped, Record, Locate (STr), Play (deferred), Stop frontier, record-off receipt, Resume check (RP, the resumed pass), TogglePlay (STp! before the resumed Record's receipt or stage), Stop frontier: Stopped with the deferred command stranded.
- C9 in S, SP and P: the half speed is set before the start is submitted, as today (`host/transport.rs:300-303`); an ordering clarification, no new decision.
- R13 and V3 headings and RND cells, section 11: the cancel precedes the retirement in Rendering (D-6).

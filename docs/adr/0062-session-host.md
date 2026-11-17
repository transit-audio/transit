<!-- transit-index
title: The typed transport lifecycle: one transport phase is the truth for starts, stops, recording and the real-time render; every Player request is a tracked fact until retired; transport changes are admitted synchronously; engine guards read named capabilities; the record start publishes its stage through its ticket; status text is arbitrated per transition; the code stays in the interface crate with no window types, ready to move; lands before hardware sync slice 1
-->
# ADR 0062: The typed transport lifecycle

- **Status:** proposed 2026-11-16 (revised after judging rounds 1 to 3, same day; narrowed to the lifecycle after round 3)
- **Date:** 2026-11-16
- **Implementation:** not started. Slices S1 and S2a to S2g below, all before hardware sync slice 1 (#733, ADR 0043). Tracking issue #739 (topic B of #736). The appendix [0062/tables.md](0062/tables.md) is part of this record: the transition matrix (A), the simulation (B), the reader assignments (C), the display examples (D), the status writers that are not mechanical (E).
- **Reopen when:** ADR 0043 cannot add its states under the checklist of section 14; the display arbitration fails the maintainer's feel-right check; a lifecycle file nears the 1,500-line budget; one of the deferred records (Deferred to later records) needs a lifecycle change this record does not allow.

## Context

Facts re-read at `main` 1a56b895. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

- **The state is flags and text.** No type names the transport's states: they are combinations of `player_requests.*` (`host.rs:263-271`), `take_plan`, `Player::count_in()`, `sim_count_in`, `engine.transport.{playing,recording}` and the status string, whose four waits are compared at `host.rs:343, 354, 380` and `host/transport.rs:59, 165` and never age (`status.rs:26-28`). Section 13 lists the defects this causes.
- **Commands are queued, not admitted.** `transport.record` only pushes a request (`E/cmd/transport.rs:21`); the control channel runs up to 64 calls (`control.rs:53-56, 102`) before the frame polls the Player (`frame_logic.rs:325, 329`). Some transport changes do not pass through a command at all (section 7).
- **Transport is started outside the transport code.** `Player::start_realtime_render` calls `self.play` (`P/player/device/realtime_render.rs:106`); its result and cancel paths call `self.stop` (`:137, 165, 188`).
- **Recording without a Player is not reachable outside a fixture.** Audio takes need a recorder only a Player opens (`host/recording.rs:265`, `host.rs:118-141`); MIDI refuses without a Player (`host/recording.rs:212`); the simulated count-in is set only at `midi_transport/tests.rs:138`.
- **ADR 0043 slice 1** needs "a deterministic state matrix" without a window.
- **Conventions.** None from Ableton or Logic applies to internal structure. Ableton Live "will not begin recording until the count-in is complete" and Logic's count-in "switches to record when it reaches" the playhead (as quoted in ADR 0052): the count-in is not recording, this record's meaning of the flag. Capture during the count-in (ADR 0052 decision 8) is the audio thread's and not the flag.

## Decision

### 1. Settled

Maintainer (2026-11-16): M1 the transport gets a typed state and its orchestration leaves the interface crate (this record does the first half; the move is deferred, below); M2 the flag disagreements are fixed in slice 2, each named and tested; M3 slice 1 and slice 2 land before sync slice 1. Standing rules: no migrations while alpha; no interim surface a recorded plan replaces; exact where a professional could null-test; nothing audible changes unasked; 1,500-line files; related defects become task lines on #739. Coordinator rulings R-A to R-K, S-1 to S-16 and T-1 to T-9 are applied and listed under the judging responses.

Nothing here changes capture, the latch or any saved file. Two playback changes: the record start publishes its stage through its ticket (section 6), and the real-time render returns its transport receipts with its result or cancellation (section 11). Both land in S2b.

### 2. Where the code lives

The lifecycle lives in the interface crate, in its own module `crates/ui-studio/src/host/lifecycle/` (phase, requests, admission, capabilities, status line). Rule: the module names no egui type and reads no `StudioApp` field of an egui type; it takes the frame time and `dt` as parameters and calls no system clock. It can then move into its own crate unchanged (deferred, below). S2b adds the device-free step: `StudioApp::step_host(&mut self, now: f64, dt: f64)`, the transport part of `logic()` with no egui argument, and `host::testing::device_free_app()` (test only), the window-free adapter sync slice 1's gate uses.

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
pub enum AfterStop { Park { target: Option<Samples>, rtz: bool }, Resume(Samples), Reset }
pub enum RenderJob { Live, Retired }
```

- `RecordPending`, `CountIn`, `WaitingForClock` carry the full record-start ticket (`P/record/start.rs:17-20`: engine epoch and sequence). `Recording.latch` comes from the stage `Started(point)`; only a Player records, so it is required. The count-in plan rides for the take's plan fallback (`host/recording.rs:418`).
- `StopPending`: `Park.target` is a Locate made while stopping (the retained position after completion, section 9); `Park.rtz` is a second Stop (D9); `Resume(at)` a locate during a pass; `Reset` a session replaced. Reset supersedes Resume.
- `Rendering`: the host owns the render; it leaves only by the rule of section 11, so the phase never reads Stopped while a render plays.
- The request identities live in section 4's facts, not in the phase.

**Facts beside the phase:** the tracked requests (section 4); held commands (Park) and deferred commands (Resume), each stamped with the reset generation current when it was held; the reset generation (section 7); speed `Normal | Half` (invariant: `Half` only in StartPending or Playing; every other transition sets `Normal` and `Player::set_speed(1.0)`, B7); the wanted loop, the loop last sent, the loop a pass records under; observed playing and position, sampled together; the working cursor and retained position (section 9); reopen state; `faulted` (set by `PlayerError::Faulted` or a Player fault, cleared only by replacing the Player); the MIDI transport `recovering` flag; the ingress cutoff (section 14); the backend, Player or simulated.

**Invariant, tested after every admission and every step:** Stopped implies no tracked request that can move the transport (a start, Record, Stop, render Play or render Stop) and no observed playback. A step that observes playback while Stopped with nothing tracked adopts it (row O7: Playing, logged).

### 4. Request ownership

Every request the host submits to the Player is a tracked fact, with its sequence (and the ticket for Record), from submission until it is retired by its own receipt, by a terminal stage (Record), by a fault, or by the Player being replaced. Facts: the start groups; Record; record-off; Stop; Locate; release; render Play; render Stop.

- **Retirement is independent of the phase.** No cell calls a receipt impossible for a request that can still be tracked: a receipt retires its fact in every phase, and the phase rule then says what, if anything, changes. A receipt for a request that was superseded or already retired is a no-op, counted in the debug log.
- **Start groups.** Play-family requests (Play, PlayExact, PlaySelection, HalfSpeed, a Toggle that starts) join the current group, which has an `applied` bit. A Stop closes the current group: its members stay tracked until their receipts retire them, but nothing from a closed group, its `applied` bit included, carries into the next group. When the current group's last member retires, the phase becomes Playing if any member applied or playback is observed, else Stopped. There is no count limit: the bound is the action lane's capacity (`P/action_lane.rs:12`), and exceeding it is the lane's existing refusal.
- **Play A, then Record.** Record is admitted with A still in the current group; the phase is RecordPending (the pass is what the transport is doing); A's receipt later retires A with no phase change (its refusal text, if any, is posted). A Record from stopped no longer reuses the Play request (B6).
- **Stage before receipt.** A record stage can arrive before Record's action receipt; the stage moves the phase, and the later receipt retires the Record fact with no phase change (a refusal's text is posted only if no failure was reported for that ticket).
- **A failed or cancelled start's destination** is decided when the terminal observation is handled, from what is true then: Playing if playback is observed or the current start group has an applied member; StartPending if the current group still has members; else Stopped. Not from "stopped or punch" at request time.
- **A fault or a Player replacement** retires every fact, closes every group, and discards held and deferred work; adoption (O7) is disabled until the Player is replaced.

### 5. The state table

| Phase | Audible | State text | Record button | Commands with an effect |
|---|---|---|---|---|
| Stopped | no transport sound (live instruments sound, E3) | none | off | all |
| StartPending | the sound before the group (nothing, or the old playback) | Play pending | off | starts join the group; Toggle, Stop, Pause close it; Record; Locate (cursor only) |
| Playing | playback | none (outcome "Playing") | off | all but Play |
| RecordPending | from stopped nothing; punch playback | Count-in when a count-in was sent, else Record pending | off | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| CountIn | the click | Count-in | pulses while the published counter runs (`transport.rs:193-197`) | the starts, Toggle, Stop, Pause, Record, HalfSpeed cancel; Locate take-safe |
| WaitingForClock | from stopped nothing; punch playback | Record pending (B1) | lit | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| Recording | playback and capture | none (outcome "Recording") | lit | Toggle, Stop, Pause, Record (take finalized); Locate (take-safe) |
| StopPending Park | playback until the audio thread applies Stop | Stop pending | off | starts, Toggle, Record held; Stop sets `rtz`; Pause drops held starts; Locate sets the target |
| StopPending Resume | as Park | Stop pending | off | Stop, Pause, Record drop the resume; ResetSession turns it into Reset; Locate retargets; AllNotesOff and Scrub at once; HalfSpeed refused; the rest deferred |
| StopPending Reset | as Park | none | off | Stop re-sends Stop; Pause and everything else no effect (held and deferred work discarded) |
| Rendering | the render's playback | none (the render dialog shows progress) | off | all refused with the render's text, except All Notes Off (cancels the render first, `finish/realtime.rs:164-171`) and Scrub |

### 6. Transitions and the record start's stage

The matrix is [tables.md](0062/tables.md), section A; it is total for this record's phases. Cells that differ from today carry their B or D item. Rules:

- **Player errors, in this order:** `PlayerError::Faulted` applies row R13. After a call that cancels the ticket before it can fail (`Player::stop`, `locate`, `set_recording(false)` cancel first, `P/player/device.rs:319, 469-470, 475-476`) fails, the host reads the ticket's stage: `Cancelled` ends the pass (row O5); `Started` (a cancel cannot undo a started pass, `P/record/start.rs:133-134, 193-195`) keeps it. Any other error keeps the phase and posts the error.
- **While `faulted`** every start refuses with the fault's text; finalization runs with Player errors ignored.
- **Untracked refusals** reach the status line (`host.rs:362-364`, `player_tests.rs:99-116`).

**The stage (S2b).** The record start publishes through its ticket-owned structure: one state word per runtime, the ticket's sequence plus flag bits, issued with `checked_add` (`P/record/start.rs:10-13, 117-125`), validated against the full ticket by `Reader::status()` (`:232-250`). S2b widens the flags from two bits to three (`FLAGS = 7`, sequences step by 8; the identity space halves from about 2^62 to 2^61, exhaustion still refused, never wrapped) and adds stages: `Pending`, `CountingIn`, `Waiting`, `Started(Point)`, `Failed(StartFailure)`, `Cancelled`.
- **Audio side:** every stage is published by one compare-and-swap from the matching ticket's non-terminal state, never a plain store, and never retried (bounded work on the callback). A terminal stage is its payload stores (release) followed by that compare-and-swap, as `Started` is today (`P/record/start.rs:158-165`).
- **Main's cancel** retries its compare-and-swap when it loses to a stage advance, at most three times (`Pending`, `CountingIn`, `Waiting` are the only non-terminal stages), and gives up on a terminal stage.
- **Failure is published at its source,** before anything cancels: `record_within`'s error paths (`P/player/audio.rs:301-303` for speed, `:341-343` for the rest) and `count_in`'s refusals (`:383-386, 393-396`) publish `Failed(reason)` instead of calling `cancel`; a refusal with no start-failure reason publishes `Cancelled`. The `start_failure` atomic stores at `:949` and `:967` stay in S2b for today's consumer and are deleted in S2d with `take_start_failure()` (`P/player/device.rs:903-904`).
- **Readers:** acquire on the state word, read the payload, revalidate the state word (the existing pattern). `Reader::status()` is authoritative for the pass; `Started(point)` is the latch for `Recorder::applied_start` and `Player::midi_recording_latch`; `Failed(reason)` gives the message once per ticket; action receipts retire the Record fact; `count_in()` is display only.
- **Exhaustive consumers** change in the same commit: `P/record/start/mapping.rs:68-74` and `P/record.rs:758-769` treat `CountingIn` and `Waiting` as `Pending` and `Failed` as `Cancelled`; the tests that observe the status are listed in S2b.
- **Terminal stages** `Cancelled` and `Failed` end the pass in RecordPending, CountIn and WaitingForClock: resources finalized (`finish_recording`), destination by section 4. No phase waits for a latch that cannot come.

### 7. Admission

**One synchronous entry.** Everything that changes the transport reaches the host through `Host::admit(cause)`, which runs inside the change before its caller's next guard:

| Path | How it reaches the host |
|---|---|
| engine commands: `run`, `execute_ui_merged`, `execute_with_mode` and the direct `engine.execute` sites (`actions.rs:446-449`, `groups.rs:412-423`, `mixer/actions.rs:344-353`, `import.rs:307`, `sampler_panel/instrument/state.rs:178`, `pad_actions.rs:11`, `sampler_panel/slices/{detect_panel.rs:199, state.rs:228, use_panel.rs:99, 104}`, `arrange/track_height.rs:83-88`) | one `StudioApp::execute` wrapper calls `admit(Commands)` after `engine.execute`, draining the requests that command queued |
| control channel (`control.rs:102`), MIDI transport buttons (`midi_transport.rs:262`; their extra `handle_transport(0.0)` at `:270` is removed) | through `run` |
| ruler and empty-space locate (`actions.rs:180, 187`, today `locate_playing` directly) | `admit(Locate(at))`, keeping their own cursor and selection handling |
| crash recovery (`host/autosave.rs:160` calls `E/autosave.rs:400`, which replaces the session and queues `ResetSession`, `E/lib.rs:488-490`) | `recover` calls `admit(Commands)` right after the engine call |
| `run_or_ask`'s stop before a replace (`menus.rs:18-19`) and the replace preparation (`frame_logic.rs:148-151`), `close_autosave`'s stop (`host/autosave.rs:114-116`) | `admit(Stop)` |
| render start (`finish/realtime.rs:240`) and cancel (`finish/realtime.rs:328`, from `finish.rs:300, 692`) | `admit(RenderStart)`, `admit(RenderCancel)` |
| Player installation (`splash.rs:129-136`) and replacement (`host/reconnect.rs:212-255`, `audio_connection.rs:75-86`) | `admit(Install)`, `admit(Replace)`: a replacement retires every fact first (section 4) |
| test setup that injects requests (`browser_tests.rs:819`, `builtin_editor_tests.rs:565-566`, `slice6_tests.rs:442`, `transport_tests.rs`, `audio_devices_tests.rs`) | edited to call the entry (S2c) |
| device trouble, lap limit, natural end | inside the step (section 10), which is the host |

**Order inside one admission:**
1. **Supersede.** If the cause or the queue holds a `ResetSession`, `Stop` or `Pause`, the held and deferred work it cancels is discarded now; a `ResetSession` also increments the reset generation. The ingress cutoff (section 14) is applied here too.
2. **Drain receipts.** Call `poll_notices` until it returns fewer notices than its buffer (64; the lane is bounded at 256 actions, `P/action_lane.rs:12`), then read the stop and release frontiers. Each receipt retires its fact; a stop completion releases held work only if that work's reset generation is current, so a receipt never releases work against a document already replaced. A nested request made by a transition (the second Stop's Return to Zero) is applied as part of that transition, without draining again.
3. **Refresh** the observed playing and position pair, so a Record right after a Play receipt sees playback (punch versus stopped start, `host/recording.rs:54`).
4. **Apply** the remaining requests in order through the matrix; submit the Player requests; publish the capabilities.

**Multi-request commands** (`edit.play_timeline` queues Locate then Play, `E/cmd/edit_more.rs:132-133`) are applied in order with no rollback, as today: if the Play is refused or held, the Locate stays applied.

**Results.** A command returns `Ok(disposition)` or `Err(refusal)`. Dispositions, from its requests in order: `Err` if any was refused (the text of the first refusal; earlier requests stay applied); else `Held` or `Deferred` if any was; else `Submitted` if any Player request was submitted; else `NoEffect`. The refusal text also goes to the status line, as today. Transport commands' replies gain `"transport": "submitted" | "held" | "deferred" | "no_effect"`.

| Caller that depended on a queued `Ok` | Edit |
|---|---|
| control channel (`control.rs:102-104`) | none: `Err` already becomes `{"ok": false, "error": ...}`; the reply gains the disposition field |
| MCP `batch` tool (`crates/automation/src/mcp.rs:135-155`) | none in code: it stops at the first `Err` unless `keep_going`; a refused transport command now stops a batch; `docs/control-protocol.md` says so |
| MIDI button dispatch (`midi_transport.rs:260-266`) | an `Err` for an unarmed Record is its `refused` case, keeping "Record from <device>: no armed track"; other `Err`s continue as today |
| tests | `host/live_cycle_tests.rs:371, 387, 392` expect `Err` where they unwrapped a refused Record or HalfSpeed; tests that observed a queued request are listed in S2c |

**Reset generation.** The three readers that look for a queued `ResetSession` read the reset generation instead: `paste_at_cursor` (`actions.rs:538`) clears `copied` when its stored generation differs, then stores it; `sync_player_session` (`host/plugin_state.rs:16`) refuses while the newest reset's Stop is tracked and records the generation when it syncs; `apply_editor_edits` (`host/plugin_state.rs:174`) discards edits collected under an older generation.

### 8. Capabilities and the engine

The engine gains `capabilities: Option<Capabilities>`, written by the host at every admission and transition, and `Engine::capabilities()`, which returns it or, with no host attached, computes it from `TransportStatus`:

| Field | With a host | With no host (`from_status`) |
|---|---|---|
| `record_engaged` | RecordPending, CountIn, WaitingForClock, Recording, StopPending Resume | `recording` |
| `finalizing` | a take plan, a recorder take pending, a MIDI pass prepared, or record-off tracked | `recording` |
| `work_outstanding` | any tracked request, held or deferred command | the request queue is not empty |
| `exclusive_ok` | Stopped, nothing tracked, held or deferred, not `finalizing`, not `faulted`, no render | not (`playing` or `recording`) |

`TransportStatus` is unchanged (`playing`, `recording`, `position`); the host writes `playing` as observed playback and `recording` as the Recording phase, published with `position` at every admission and transition. Engine guards read `capabilities()`: `blocked_while_recording` (`E/lib.rs:770`) reads `record_engaged`; `hardware_measurement` (`:414`), `measure_blocker` (`E/cmd/external_instrument.rs:45`) and `realtime_plan` (`E/cmd/realtime_render.rs:115`) read `exclusive_ok`. With no host the fallback gives today's answers, so the engine-only CLI and the engine tests that set `playing` or `recording` directly keep their expectations: `E/tests/recording.rs:8`, the `realtime_render.rs` test (`:208-213`), the `external_instrument.rs` test (`:311-313`), `E/tests/session_transport.rs:9`. `transport.status` (`E/cmd/transport.rs:95`) adds a `capabilities` object; `ui.inspect` adds `record_engaged`; `docs/control-protocol.md` follows, keeping its `recording_pass` contract.

Host-only predicates: `running()` (not Stopped and not StopPending Park or Reset), `transport_active()` (not Stopped), `sync_allowed()` (no start, Locate, Record or record-off tracked, and no render in progress: the Rendering phase, or until S2e the `finish.realtime.running` interlock; Stop and release do not block, as `frame_logic.rs:365`). Every interface reader is assigned in [tables.md](0062/tables.md), section C. `recording_active()` is removed; each of its uses moves to the capability it means (S2d).

### 9. Positions and synchronous effects

Inside an admission or a step the host keeps a working cursor and working positions; each request sees the effects of those before it.

| Effect | Side |
|---|---|
| cursor move by a locate or take-locate (`host/transport.rs:145, 268`) | synchronous (working cursor); `CursorTo` for the interface's copy |
| transport and retained positions (`host/transport.rs:52, 66, 146, 223, 260, 270`) | synchronous |
| a stopped cursor move from the interface (`lib.rs:537-539`) | synchronous: `set_cursor` calls `host.cursor_moved(at)` |
| selection after a placed take (`host/recording.rs:640, 712`) | synchronous: the host runs `edit.select` |
| Return to Zero on a second Stop (`host/transport.rs:238-241`) | synchronous, inside the transition |
| undo squash of a Record operation (`host/recording.rs:466-478`) | synchronous |
| ResetSession's cursor clear (`host/transport.rs:199`) | synchronous |
| scroll to pickups, focus, take lanes, reset view clears, the Record hint, editor re-want, editor edits | deferred events |

**Retained position:** at Park completion, `Park.target` when a Locate set it, else the applied stop position read before parking (`host.rs:96-97`); written by PlayExact and Locate (their target) and ResetSession (0); read by Continue.

### 10. The step

The step handles receipts, observations, timers and device events, in today's `logic()` order:

| # | Block | Today |
|---|---|---|
| H1 | the drain and supersede rules of section 7 (steps 1 and 2); Park completion; held work released | `frame_logic.rs:329`, `host.rs:82-114` |
| H2 | recording clock and journal; lap limit | `:335-337` |
| H3 | output trouble, input trouble, input loss (message only, the pass keeps recording, `host/reconnect.rs:57-64`), reopen | `:355-357` |
| H3r | render: result poll (section 11) | `:360` (`finish.rs:809-811`) |
| H4 | click and monitor pushes; the record stage; the Resume check | `host/transport.rs:139-167` |
| H5 | loop retarget; readback, natural end, adoption (O7); simulation timers; meters | `host/transport.rs:309-386` |
| after | automation write pass; session sync when `sync_allowed()`; autosave; input | `frame_logic.rs:364-375` |

MIDI buttons run right after the control drain (D7); buttons received during StopPending stay in the button queue, as today (`midi_transport.rs:224-227`).

**Held and deferred work.** Park holds starts, Toggle (so Space twice restarts) and Record, admitted in order at completion; a held start is cancelled by a later Stop or Pause, by ResetSession, by a device reopen or replacement, by a fault, and by the ingress cutoff. A second Stop in Park sets `rtz` (D9). Reset discards held and deferred work. Resume's exceptions are Stop, Pause, Record and ResetSession (they drop the resume; ResetSession makes it Reset), Locate (retargets), AllNotesOff and Scrub (applied at once), HalfSpeed (refused); everything else is deferred. When the Stop and record-off are retired, H4 submits the full Record start at the target; the deferred commands are released once per resumed request, on the first of: its admission receipt, a stage of its ticket, its refusal receipt, or a local refusal (which leaves Stopped with the finished take preserved and the reason shown).

### 11. The real-time render

- **Entry.** `admit(RenderStart)` requires `exclusive_ok`; `start_realtime_render` returns the render Play's `Sequence` (a synchronous `play()` error stays its `Err`). The phase becomes Rendering, job Live; the render Play is a tracked fact.
- **Receipts with the result (S2b, playback change).** The job is removed when it is collected (`P/player/device/realtime_render.rs:171, 184`), so the Stop sequences travel with it: `realtime_render_result()` returns the result together with the render Stop it submitted, if any (`:159-166`); `cancel_realtime_render()` returns the Stop it submitted and the writer's own result when the writer had already finished. The render Stop becomes a tracked fact.
- **Ordering, from what the code does.** A cancel admitted before the step's result poll takes the job (today `slot.take()` at `:184`); a result collected first stands, and a later cancel finds no job: the job stays Retired, the collected result is kept, and the phase waits only for the render Stop's frontier.
- **Leaving.** Rendering becomes Stopped when the job is Retired, the render Play and render Stop are retired, and playback is not observed. A refused render Play cancels the job (no Stop needed).
- **Files, as today.** Cancellation removes unfinished part files (`P/realtime_render/writer.rs:269-271`); anything the writer had already committed (each file is synced and renamed when it finishes, `:261-266`, main then stems, `:319-321`) may remain. The message for a cancel caused by a session replacement: "Render cancelled: the session was replaced." and, when the cancel returned the writer's finished result, "The finished files were kept."; a reopen keeps today's text (`finish/realtime.rs:26`).
- **Fault.** No acknowledgment can arrive: the host cancels the job, which joins the writer (`P/player/device/realtime_render.rs:27-31`) and blocks the interface thread as it does today; the render facts are retired by the fault and the phase becomes Stopped with `faulted`. That blocking join is out of scope here (Deferred to later records).

### 12. Status text

- `StatusLine` (plain data, in the lifecycle module): `message: Option<Message { text, kind: Outcome | Note, rev, since }>`, `state: Option<State { text, rev }>`, `next_rev`, and `last_shown: Option<(u64, String)>` (the diagnostics report's last shown message and time, which today's `StatusAge::last` keeps after expiry, `status.rs:53, 70`). `set(text)` takes a new revision when the text differs; `say_again` (today's `StatusAge::restart`, #216; `import.rs:294`) takes one for equal text; `last_text()` returns the last written message. `StatusAge` ages only `message`.
- `state_text(phase, facts)` is the one producer of state texts: the phase's text over "All MIDI notes off pending" (release tracked).
- **Shown:** `message` when its `rev` exceeds `state.rev`, else `state`, else `message`, else empty.
- **Arbitration, once per transition, with the shown line before it as input:** (1) a new state text takes a fresh revision when the line showed a state text, nothing, or an Outcome; when it showed a Note, the state keeps the old state's revision, or 0 when there was none; (2) an outcome ("Playing", "Recording", "Stopped", "All MIDI notes off") is written as an Outcome only when the line showed a state text or nothing and no state text is live after the transition; (3) a transition with no outcome of its own clears an Outcome message, so "Playing" cannot reappear after a Reset.
- Writers that append to, read back, compare or return the status are treated one by one ([tables.md](0062/tables.md), section E); the rest change `app.status = x` to `app.status.set(x)` and `self.status.clone()` to `last_text()`.
- Ten worked sequences are in section D, each a test.

### 13. Behavior changes and related defects

| # | Change | Slice |
|---|---|---|
| B1 | the recording flag only from the latch; the clock wait shows "Record pending" (`P/player/audio.rs:299, 340, 412`; `host.rs:349-355`) | S2d |
| B2 | StopPending Park and Reset are not a record operation; Resume is (`host/recording.rs:375-381`, `host/transport.rs:63`, `host/recording.rs:281`) | S2d |
| B3 | Count-in leaves only on the record start's stage (`host/recording.rs:149-154`, `host/transport.rs:165-166`) | S2f |
| B4 | the shown state is derived and arbitrated (`host.rs:343, 354, 380`) | S2f |
| B5 | replacement and reopen leave no pending text (`host.rs:280-282`, `host/reconnect.rs:107-112, 227-228, 245`) | S2f |
| B6 | a Record start no longer pulses Play; the plug-in loading override stays (`host/recording.rs:150-152`, `transport.rs:120-127`) | S2f |
| B7 | the speed invariant; the simulation honors speed; the half-speed Record refusal stays (`host/transport.rs:12-21, 289-292, 361`) | S2g |
| B8 | a take-locate keeps readback and pushes; meters still suppressed while Stop is pending (`host/transport.rs:139-142, 324`) | S2d |
| B9 | one playing and position pair, refreshed per step and per admission; `midi_monitor.rs:674` included (`lib.rs:637`, `host/transport.rs:312-317`) | S2g |
| B10 | the count-in plan lives in phase data (`host/recording.rs:154`, `host/transport.rs:58`) | S2f |
| B11 | one owner for the positions, no timing change (`midi_transport.rs:59`, `lib.rs:537-539`) | S2g |
| B12 | readers that newly block: the raw-flag readers block from RecordPending on (`actions.rs:11, 15`, `menus.rs:18, 402`, `E/lib.rs:770`, `host/timed_read.rs:44`); exclusive operations need `exclusive_ok`; the save, timed-read and plan-wait guards also wait for a render (tables C) | S2d |
| B13 | a command returns `Ok(disposition)` or `Err(refusal)` at once (today `Ok` and a later status message) | S2c |

| # | Today | Instead | Slice |
|---|---|---|---|
| D1 | Play or Toggle during a clock wait from stopped restarts playback and drops the record start (`host/transport.rs:218-219, 246-249`; `P/player/audio.rs:452, 417-418`) | Play ignored; Toggle cancels | S2e |
| D2 | PlayExact and PlaySelection have no pass guard outside an observed count-in (`host/transport.rs:222-224, 273-279`) | ignored during RecordPending, WaitingForClock, Recording; PlayExact in CountIn cancels | S2e |
| D3 | a session replaced during a take-locate attempts the resume on the new document (`host/transport.rs:139-148`) | Reset supersedes Resume, before any receipt releases work | S2c |
| D4 | starts during Stop pending depend on the moment's observations and can coexist with the Stop (`host/transport.rs:21-23, 218, 246-256`) | held, released at completion; Toggle counts as a start | S2d |
| D5 | Locate or a loop change during Stop pending sends a request to a stopping Player (`host/transport.rs:85-92, 128-133, 268-270`) | sets the park target; nothing sent | S2d |
| D6 | Pause while stopped sends a Stop (`host/transport.rs:244`) | no effect | S2g |
| D7 | MIDI buttons run in the drawing pass after the write pass and autosave (`frame_logic.rs:363-372, 467`) | after the control drain, through the same admission | S2c |
| D8 | ResetSession during a render sends Stop under it (`host/transport.rs:176-184`) | the render is cancelled first, with its own message | S2e |
| D9 | a second Stop while stopping returns to zero only if the moment's observations say stopped (`host/transport.rs:228-241`) | always, at completion | S2d |

### 14. Relation to ADR 0043

| Requirement | Here |
|---|---|
| 1. sync mode changed only stopped and finalized | `exclusive_ok` (S2d) |
| 2. Start/Continue/Stop/SPP with its own start pending | start groups, `next()`, the matrix; ADR 0043 adds its armed states by amendment |
| 3. duplicate suppression against a pending locate | the tracked Locate keeps sequence and target; ADR 0043 adds the applied epoch and source generation |
| 4. record engaged, "Waiting for external start" | ADR 0043's state under the checklist below; B1 is its precondition |
| 5. sticky local Stop | ADR 0043's mechanism at ingress admission (ADR 0043:534-537). The host applies the ingress cutoff (`IngressCutoff { generation, offered_sequence }`, kept distinct) in step 1 of every admission and before releasing any held or deferred command |
| 6. half speed and scrub refused in clock follow | ADR 0043's condition on those rows; a refusal leaves phase, speed and position unchanged |
| 7. readout | ADR 0043's, through `TransportStatus` and `capabilities()` |
| 8. applied moments for sending | ADR 0043's callback-published spans (ADR 0043:319); the lifecycle's transitions are not used for sending |
| 9. slice 3 refusals, tempo, journal | unchanged |
| 10. a deterministic matrix without a window | the pure matrix (S2a) and the device-free step (S2b) |

**The matrix is total for this record's phases only.** ADR 0043 adds its states by amendment. Each added state must define: its payload and the resources it owns (a prepared ticket, a start boundary); its row for every existing event and column for every existing phase; which requests it tracks and how each retires; its capabilities (`record_engaged`, `finalizing`, `work_outstanding`, `exclusive_ok`); its state text; its observation and error precedence (stage, receipts, ingress); and its cancellation on local Stop, selected-source Stop, ingress overflow or loss, source replacement, session replacement, device failure and preparation failure. Input from round 2: (1) Record engaged locally, waiting for an external Start (holds a prepared ticket); (2) an external Start or Continue received, waiting for its first following clock tick (holds the start message's boundary); (3) a submitted recording action (RecordPending).

**"Opening a session always starts stopped"** (ADR 0043:85) holds because Reset supersedes in step 1 of every admission, before any receipt can release held or deferred work (section 7). No ADR 0043 decision becomes false. Its path statements naming `crates/ui-studio/src/host/transport.rs` stay true until the deferred move.

### 15. Relation to ADR 0052

ADR 0052 relies on the audio thread (decision 8 and its 2026-11-16 amendment: "The count-in bar keeps its full length in output frames"), `count_in_for`, `plan_recording` writing `count_in`, and `recording_pass`. None changes: the stage reports what the audio thread does, and moving failure publication to its source changes no timing.

## Deferred to later records

The settled direction, without detailed promises. Each later record decides its own design and slices.

- **The host crate and the move (was S3a, S3b).** The lifecycle module and the rest of `host/` move into a `transit-host` crate at layer 5 with no interface dependency; `StudioApp` holds one host value. That record decides the crate's API, the clock capability (frame, monotonic, calendar) the host is given instead of parameters, how view effects become events across the boundary, and how tests reach the host.
- **Service handles and an unnameable Player (was S3c to S3f).** The interface reaches the Player only through narrow host-owned handles, and loses its normal dependency on `transit-playback`. That record decides the handles and their methods, the connection traits' new home, the data re-exports, the public-surface audit and the compile-fail proof.
- **Headless play (was S4).** The automation backend drives the same host with a null Player and stops dropping transport requests. That record decides stepping, replies, recording refusal, confinement and shutdown.
- **Slow calls off the interface thread (was S5).** Device reopen, input open, the control-channel latency measurement, the timed autosave flush, device listing, and the render writer's blocking join on a fault (section 11). That record decides, per call, stepping, a worker for work that does not hold the Player, or a visible staged wait, given that the Player stays on its creating thread.

## Slices

Every commit builds; every slice runs the full local gates and is shippable alone. Assertions about what is heard, where a take lands and the record latch never weaken; a test that observed the old mechanism (a status variant, a queued `Ok`, an untagged failure atomic, a removed helper) is edited to observe the new one and is listed. Sizes are changed lines including tests.

| Slice | Changes | Depends on | Size |
|---|---|---|---|
| S1 | read-only `transport_phase()` in `host/lifecycle/` from today's data plus request metadata (play start, record from, count-in, ticket, record-off kind) and a persisted stop reason (`User | Locate | Reset`) written when a Stop is submitted, so a pending Reset is never read as Park; `classify` and `frontiers` behind wrappers with today's text and flag rules | none | about 500 |
| S2a | the pure reducer `next(phase, event, facts)`, section A as a literal table, unused; tests `transport_phase_matrix`, `the_matrix_has_no_empty_cell`, `stopped_implies_nothing_tracked` | S1 | about 900 |
| S2b | groundwork, no behavior change beyond its tests: `step_host` and `device_free_app`; the render's receipts and one orchestration function for the render; the stage publication with its consumers | S1 | about 1,300 |
| S2c | the admission boundary with today's lifecycle semantics: the one entry and every path of section 7, the order (supersede, drain, refresh, apply), the reset generation, dispositions (B13), D3, D7 | S2b | about 900 |
| S2d | the lifecycle becomes authoritative, atomically: request ownership, the phase, the capabilities with every safety reader and the engine guards and fallback, terminal stages, Park and Resume rules; B1, B2, B8, B12, D4, D5, D9 | S2a, S2c | about 1,800 |
| S2e | the render as a phase; D1, D2, D8 | S2d | about 600 |
| S2f | display arbitration and count-in reporting; the status writers; B3 to B6, B10 | S2d | about 1,400, about 400 of it mechanical |
| S2g | speed, positions, readouts; B7, B9, B11, D6 | S2d | about 700 |

S2d is not one review sitting; it cannot be cut further without a state where a guard reads a meaning its writer does not produce yet: the phase, the request facts and the capabilities together define "a pass is under way". Everything that can land before it (the reducer, the adapter, the receipts, the stage, the admission boundary) does.

**Tests that change, and why** (new tests are named in sections 6 to 12 and the gates below):

| Slice | Test | Why |
|---|---|---|
| S2b | `P/player/audio/tests/count_in.rs` (`:72` loops while `Pending`; the count-in and failure cases), `P/player/audio/tests.rs`, `P/player/audio/tests/speed.rs`, `P/player/audio/midi/tests.rs`, `P/record/tests.rs` | they observe `Status::Pending` during a count-in or wait, or `Cancelled` after a start failure; they now observe `CountingIn`, `Waiting` or `Failed(reason)`. Their latch, timing and audible assertions stay |
| S2b | `P/player/device/realtime_render_tests.rs` | `start_realtime_render` and the result and cancel calls return sequences |
| S2c | `host/live_cycle_tests.rs:371, 387, 392` | a refused Record or HalfSpeed returns `Err` (B13) |
| S2c | `browser_tests.rs:819`, `builtin_editor_tests.rs:565-566`, `slice6_tests.rs:442`, `transport_tests.rs` (2 sites), `audio_devices_tests.rs` (1 site) | they inject or inspect `engine.transport_requests`; admission is synchronous now |
| S2c | any test that asserted nothing happened between `run` and the next frame | the change now happens at `run`; the builder lists each and its reason |
| S2d | `player_tests.rs` (8) | `notice` and `recovery` give way to `classify` and `frontiers`; same cases |
| S2d | the `recording_active()` call sites: `host/live_cycle_tests.rs` (7), `midi_record_tests.rs` (7), `midi_transport/tests.rs` (5), `slice6_tests.rs` (5), `transport_tests.rs` (3), `audio_devices_tests.rs` (1) | the helper is removed: an assertion that a pass is under way reads `record_engaged`; that a take is still to place, `finalizing`; "nothing records" reads both false |
| S2d | the 23 sites in 10 interface test files that set `engine.transport.recording = true` | they stood in for a pass; they set the phase through `host::testing` |
| S2d | `host/state_guard_round3_tests.rs:31-34`; `midi_transport/tests.rs:138`; `host/live_cycle_tests.rs:408` | a pending Play set directly; the simulated count-in (no longer reachable); the untagged failure atomic, now a ticket `Failed` through a test hook |
| S2f | `status.rs:147`; `slice6_tests.rs:404, 422, 455, 462-463`; `transport_tests.rs:317`; `plugin_retry_tests.rs:330` | state texts are no longer in the message channel; they read the shown line or set StartPending |
| S2e, S2g | none known | the builder lists any it finds, with its reason |

Engine tests that set `playing` or `recording` directly keep their expectations through the no-host fallback (section 8).

**Gates (new tests).** S1: six derived-phase tests and `state_text_names_the_waits`. S2a: the three reducer tests. S2b: `the_render_returns_its_play_and_stop_sequences`, `a_refused_render_start_returns_no_job`, `the_record_stage_follows_count_in_wait_start_fail_and_cancel`, `a_late_stage_write_never_overwrites_cancelled`, `main_cancel_wins_over_a_stage_advance`, `stage_exhaustion_is_refused`, `step_host_matches_logic_for_the_transport_part`. S2c: `record_then_undo_in_one_drain_refuses_the_undo`, `record_then_measure_in_one_drain_refuses_the_measurement`, `ruler_locate_admits_synchronously`, `recovery_admits_its_reset`, `reset_discards_held_work_before_receipts_release_it`, `locate_then_play_keeps_the_locate_when_play_is_refused`, `paste_after_a_replace_clears_old_copies`, `record_after_a_play_receipt_sees_playback`. S2d: every B and D test of the slice; `play_a_then_record_retires_a_without_a_phase_change`, `a_stage_before_the_record_receipt_moves_the_phase`, `a_failed_start_goes_where_the_transport_is`, `a_stopped_group_does_not_lend_its_applied_bit`, `a_fault_retires_every_request`, `stopped_implies_nothing_tracked` in the integration tests, `record_off_tracked_keeps_exclusive_ok_false`, `input_loss_keeps_recording`, Resume with audio and MIDI, deferred commands and an intervening edit, the journal handoff interrupted at each step (`host/autosave.rs:72-83`), host steps that miss a whole count-in. S2e: render ownership, fault, cancel-versus-collected and session-replacement tests. S2f: the ten display sequences; `the_lap_limit_note_never_copies_a_state_text`; `last_shown_survives_expiry`. S2g: B7, B9, B11, D6 tests.

**Feel-right items:** S2d: at Stop after a take the Record light goes off at once; Space twice quickly restarts playback once the stop completes. S2f: Record from stopped with a one-bar count-in: Play does not pulse; Count-in, then Recording; Stop pending, then Stopped; Reconnect while playing leaves no Stop pending.

## Consequences

- One value answers what the transport is doing, a render included; every request has an owner until it is retired; commands see each other's effects at once.
- `recording` means capture; engine guards read named capabilities, with today's answers when no host is attached.
- A refused transport command fails its call; `Ok` carries what happened.
- The lifecycle has no window types and can move into a crate later.
- The interface still owns the Player; the move, the handles, headless play and slow calls are later records.

## Alternatives rejected

- **The whole host in one record** (rounds 1 to 3): each round resolved most findings and found new depth; the lifecycle alone is what sync needs first.
- **Request identities in the phase** (round 2's revision): receipts arriving in another phase had no owner.
- **Queued admission** (round 1's revision): commands in one drain evaluated their guards before the earlier transition.
- **A truncated ticket in a new atomic** (round 1's revision): loses the engine epoch and the exhaustion refusal.
- **The render left on the observed flag** (round 1's revision): its requests or job could be unsettled while the phase read Stopped.
- **Inferring the count-in end from the counter or elapsed time**: misses whole count-ins.
- **Strings as constants**: the text would still be the state.

## Judging round 1: responses

Every round-1 finding was verified at `main` 1a56b895 and accepted; none was rejected. Rounds 2 and 3 re-audited them; the ones they found unresolved are answered in the sections below.

## Judging round 2: responses

Every round-2 finding was verified and accepted (none rejected): replacement Play (now start groups), cancellation (terminal stages), input loss (message only), Reset versus Park holding, one deferred-release point, the park target, record-off as a fact, the simulation scoped to Stopped and Playing, StartArmed out of the matrix with a checklist, render substates, the E1 guard, the save and capture readers, synchronous admission, display revision 0 and live states over outcomes, the non-mechanical writers, the stage through the ticket, render receipts, the slice re-cut, `recording_active()`, ADR 0043 mapping 5, B12 and D4 to D8, and headless (now deferred). Round 3 found the remaining depth answered next.

## Judging round 3: responses

| Finding | Disposition |
|---|---|
| A1, C: Play A then Record; receipts in another phase marked impossible | Accepted. Verified: `P/action_lane.rs:381-382` matches by sequence. Request ownership independent of the phase (section 4); receipts retire in every phase (tables A). |
| A2, D: stage before receipt; failure after cancel; cancellation races | Accepted. Section 4 (stage before receipt), section 6 (failure at source, CAS rules). Verified the failure sites run after `cancel`: `P/player/audio.rs:341-343, 964-967`. |
| A3, F: render cancellation and files | Accepted. Section 11: receipts with the result, cancel-after-collect, today's file behavior, no "nothing was written", the blocking join deferred. Verified `writer.rs:199-201, 261-271, 319-321`, `realtime_render.rs:27-31, 171, 184`. |
| A4, D: Reset/Pause, Resume/Scrub, Resume/Reset, deferred release | Accepted. One rule in section 5, section 10 and tables A. |
| A5, I: unchanged-test promises, render in the save guards | Accepted. Blanket claims dropped; tests that change listed with reasons; `sync_allowed()` covers a render. |
| A6, H, J: old outcomes reappear; nested MCP batches | Outcome clearing accepted (section 12, example 10). Headless is deferred (T-1). |
| B: admission coverage (ruler, recovery, `run_or_ask`, render, installation, MIDI double dispatch, test bypasses) | Accepted. Verified `actions.rs:180, 187`; `host/autosave.rs:160`, `E/autosave.rs:400`; `menus.rs:18-19`. Section 7 table. |
| B: drain boundary and Reset before release | Accepted. Section 7 order; drain until fewer than 64 notices. |
| B: multi-request command | Accepted. In order, no rollback; the disposition rule. |
| B: B13 callers; `Ok` meaning | Accepted. Dispositions; caller table; verified `mcp.rs:140-152`. |
| B: engine-only compatibility | Accepted. `capabilities()` with the no-host fallback (section 8). |
| B: `Scrub` has no production producer | Confirmed: no engine command queues `TransportRequest::Scrub`. Kept in the matrix for totality, behavior unchanged. |
| C: Stop-group history; fault retirement | Accepted. Start groups; a fault retires every fact (section 4). |
| D: errors after attempted cancellation | Accepted. The host reads the ticket's stage (section 6); verified `P/record/start.rs:133-134, 193-195`. |
| D: 16-start limit untagged | Accepted. Removed; the lane's capacity bounds it. |
| E: stage protocol and S2b buildability | Accepted. CAS rules, Main's bounded retry, consumers and tests in the same commit (section 6, S2b). Verified `mapping.rs:68-74`, `record.rs:758-769`. |
| G: render in `sync_allowed`; reset-generation readers; freshness | Accepted. Section 8; section 7 (reset generation, refresh). Verified `actions.rs:538`, `host/plugin_state.rs:16, 174`. |
| H: diagnostics last-shown; `import.rs:294`; `clone_from` writers | Accepted. Section 12; tables E. |
| I: S1 Reset intent; S2c cut; `recording_active()` alias | Accepted. Persisted stop reason; the earlier admission cut is S2c; the helper is removed and its uses moved. |
| I: the ten named tests | Accepted as the judge read them: `count_in.rs:63` changes (S2b); `:118`, `:161`, `:240`, `midi/tests.rs:387`, `midi_transport/tests.rs:211, 257` keep their assertions; `live_cycle_tests.rs:366, 383, 402` change (S2c, S2d). |
| K: facade, render-measurement methods | Deferred with the handles record (T-1). |
| L: ADR 0043 checklist; "opening a session always starts stopped" | Accepted. Section 14. |
| M: B13 and D9 | Kept. |

## Judging

## Related

[ADR 0043](0043-hardware-sync-transport-and-clock.md), [ADR 0052](0052-pre-count-zone.md), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md), [ADR 0018](0018-plugin-host-thread.md), [ADR 0026](0026-cycle-recording-stacks-takes.md), [ADR 0027](0027-external-instrument-real-time-render.md). Issues #739, #733, #736.

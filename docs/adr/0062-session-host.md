<!-- transit-index
title: The session host: transport and recording orchestration move out of the interface crate into `transit-host` (layer 5), with one typed transport phase as the only truth, status text derived from it, the recording flag true only from the latch to the end of the take, the host stepped once per pass with time as an input, headless play and record, blocking work staged or moved off the interface thread, and a layer rule plus a source check that keep it out
-->
# ADR 0062: The session host: one typed transport phase, outside the interface

- **Status:** proposed 2026-11-16
- **Date:** 2026-11-16
- **Implementation:** not started. Slices S1 to S5 below. Tracking issue #739 (topic B of #736). S1 and S2 land before hardware sync slice 1 (#733, ADR 0043); S3 to S5 run beside sync and do not block it.
- **Reopen when:** a record changes the Player so that it may live on another thread (alternative iii); hardware sync slice 1 cannot express its state machine as phase variants and matrix rows of this record (section 15); a measured device reopen on the reference machine holds the interface thread for more than one second after S5; the status display rule of section 6 fails the maintainer's feel-right check; a host file nears the 1,500-line budget and the split by concern does not fit section 11.

## Context

The read-only inventory for #739 was taken at 3b12412e; every fact below was re-read at `main` 1a56b895 (only `audio_devices/`, `help/map.rs` and one line of `xtask/src/main.rs` changed between them). Paths are under `crates/ui-studio/src/` unless named.

**The host is the interface struct.** Every transport and recording function is a method on `StudioApp` (`lib.rs:214`), which holds the Player (`lib.rs:218`), the request tracker (`:219`), the recorder (`:221`), the status line (`:267`) and egui types (`synthetic: Vec<egui::Event>` `:319`, `retry_all_rect: Option<egui::Rect>` `:342`). `host/` is 4,241 lines with its tests. Nothing in it runs without a window type in scope.

**The state is flags and text.** The transport's states have no type: each is a combination of `player_requests.*` (`host.rs:263-271`), `take_plan`, `Player::count_in()` (`playback/src/player/device.rs:897-900`), `sim_count_in`, `engine.transport.{playing,recording}` and the status string. Four strings are both display and state: "Play pending" (`host/transport.rs:23`), "Stop pending" (`:41`), "Count-in" and "Record pending" (`host/recording.rs:153, 161`). They are compared at `host.rs:343, 354, 380` and `host/transport.rs:59, 165`, and `status.rs:26-28` (`is_state`) keeps them from ageing out. The inventory's section 2 lists every state and transition; section 10 below lists the eleven places where flags and text disagree.

**Where the step runs.** `logic()` (`frame_logic.rs:309`) drains control (`:325`), polls the Player (`:329`), follows a recording pass (`:335-337`), watches the devices (`:355-357`) and runs `handle_transport(dt)` (`:363`) with `dt` from egui's frame clock (`:323`). MIDI transport buttons run in the drawing pass (`ui()`, `frame_logic.rs:467`) and call `handle_transport(0.0)` once per accepted event (`midi_transport.rs:270`).

**The Player stays on its thread.** `Player` is neither `Send` nor `Sync` by design: "Main-thread facade. RuntimeControl deliberately makes this type !Send/!Sync." (`playback/src/player/device.rs:69`); its owner is "Creating-thread owner (not Send/Sync through Control)" and dropping an endpoint elsewhere "would destroy its plug-ins on that worker" (`playback/src/player/runtime.rs:20-23`). Its fields `stream: Option<Box<dyn ab::Stream>>` (`device.rs:88`) and `native_factory: Option<Box<dyn ab::BackendFactory>>` (`:111`) carry no `Send` bound. Whatever holds the Player runs on the thread that made it.

**Nothing headless plays.** `Headless` clears every transport request (`crates/automation/src/lib.rs:53-54`). Device-free Players exist behind the `test-hooks` feature (`playback/src/player/device/open.rs:29-60`); `Player::with_null_device` is production code (`device/null_open.rs:14`), paced by its own thread.

**Who needs it next.** ADR 0043 slice 1 adds a Start/Stop/Continue/Song Position machine driven by external gear and needs "a deterministic state matrix" without a window (ADR 0043, Implementation). The inventory's section 3 lists its ten requirements; section 15 maps each.

**Conventions.** None from Ableton or Logic applies to the internal structure. Where a fixed behavior is visible: Ableton Live "will not begin recording until the count-in is complete" and Logic's count-in "switches to record when it reaches" the playhead (both as quoted in ADR 0052, Context); both treat the count-in as not recording, which is decision 4's meaning of the flag (transit still captures MIDI during the count-in on the audio thread, ADR 0052 decision 8; that capture is not the flag). No convention bears on B6 (which button pulses) or B7 (when half speed ends).

## Decision

### 1. Settled by the maintainer (2026-11-16)

- **M1.** Transport and recording orchestration moves out of the interface crate into a host with no interface dependency, with a typed transport state.
- **M2.** The flag disagreements are fixed, each a named behavior change with its own test, in slice 2.
- **M3.** S1 and S2 land before hardware sync slice 1; S3 onward runs beside sync.
- Standing rules: no migrations while alpha; no interim user-facing surface a recorded plan replaces; exact where a professional could null-test it; nothing audible changes that the user did not ask for; 1,500-line file budget; related defects become task lines on #739.

Nothing in this record changes the audio thread, the action lane, capture, the latch (ADR 0052 decision 8 and its 2026-11-16 amendment) or any saved file.

### 2. The crate

- New crate `transit-host` at `crates/host`, registered in `xtask/src/main.rs` `LAYERS` as `("transit-host", 5)`, beside `transit-automation`.
- Normal dependencies: `transit-engine`, `transit-playback` (both L4), `transit-model`, `transit-mix`, `transit-time`, `serde_json`, `log`. Dev: `transit-test-support`, `transit-playback` with `test-hooks`.
- `check_layers` (`xtask/src/main.rs:179`) then bars it from egui and the other `UI_CRATES` (`:117`, `:211`) and from any upward edge (`:219`) with no new code. Unsafe stays forbidden (workspace lints; it is not in `UNSAFE_ALLOWED`).
- `transit-ui-studio` (L6) depends on it. In S4 `transit-automation` (L5) depends on it: `check_layers` flags only `dl > layer`, so a same-layer edge passes; `transit-host` never depends on `transit-automation`.
- `Host` owns, once S3b lands: the Player and recorder, `PlayerRequests`, the transport phase and its facts, the no-device simulation, starts, stops, locates, loops, recording (arm, plan, count-in, cycle, pre-roll, finish, take placement into the session), the MIDI journal lifetime, the autosave triggers that depend on transport, device reopen orchestration. `StudioApp` holds one `host: transit_host::Host`.
- `Host` does not own the `Engine`: `Host::step(&mut self, engine: &mut Engine, input: &StepInput, out: &mut Vec<HostEvent>)`. The interface keeps the engine because most of its work is document editing. (Decided here.)

### 3. The transport phase

One value is the truth; every reader asks it.

```rust
pub enum TransportPhase {
    Stopped,
    StartPending { seq: Sequence, from: Samples },
    Playing,
    RecordPending { seq: Sequence, from: Option<Samples>, count_in: Option<CountIn> },
    CountIn { from: Samples, plan: CountIn, seen: bool },
    WaitingForClock { from: Option<Samples>, count_in: Option<CountIn> },
    Recording { latch: Option<record::start::Point>, count_in: Option<CountIn> },
    StopPending { seq: Sequence, then: AfterStop },
}
pub enum AfterStop { Park, Resume(Samples), Reset }
```

| Variant | Data | Meaning |
|---|---|---|
| `StartPending` | the Play request's sequence, its start | Play sent; its receipt not seen |
| `RecordPending` | the Record request's sequence; `from` = `Some` from stopped, `None` for a punch on the fly; the count-in sent with it | Record sent; its receipt not seen |
| `CountIn` | start, plan, `seen` = `Player::count_in()` has read `Some` at least once | the audio thread admitted the count-in |
| `WaitingForClock` | as `RecordPending` minus the sequence | Record admitted, no latch yet (the audio thread waits up to one second for the device clock, `playback/src/player/audio.rs:296-299, 332-340`) |
| `Recording` | the latch (`None` only in the no-device simulation); the count-in plan, kept for the take's plan fallback (`host/recording.rs:418`) | from the latch to the end of the take |
| `StopPending` | the Stop request's sequence; `Park` (stop and park), `Resume(at)` (a locate during a pass: finalize, then a fresh pass at `at`), `Reset` (a session replaced) | Stop sent; its frontier not reached |

**Orthogonal facts, deliberately not phases** (fields of the host beside the phase):

| Fact | Values | Why not a phase |
|---|---|---|
| speed | `Normal`, `Half` | Invariant: `Half` only in `StartPending` or `Playing`; every other transition sets `Normal` and calls `Player::set_speed(1.0)` (B7) |
| cycle | the wanted loop, the loop last sent (`loop_seen`), the loop a pass records under (`take_cycle`) | retargeted live (`host/transport.rs:122-136`) without a transition |
| release pending | the All Notes Off request's sequence | independent of the transport |
| locate pending | the Locate request's sequence | a playing locate keeps `Playing` |
| observed playing | `Player::is_playing()` sampled once per step (B9) | an observation, not a decision |
| retained, position | section 8 | positions |
| reopen | the reopener's waiting and gave-up state (`playback` `reopen`) | device state; it refuses starts |
| interlocks | measuring, External Instrument measure, real-time render, MIDI transport recovering | owned by features that stay in the interface; refused at `run` before a command exists (`frame_logic.rs:47-66`, `finish/realtime.rs:174`) |
| input | open, failed | device state |
| backend | Player or simulated | without a Player, `StartPending`, `RecordPending`, `WaitingForClock` and `StopPending` never occur: a start enters `Playing` (or `CountIn`/`Recording` for Record), a stop enters `Stopped` |
| later, ADR 0043's | sync mode, source generation, record engaged, epochs | section 15 |

### 4. The state table

| Phase | Entered by | Audible | State text | Record button | Commands with an effect (others ignored; section 5) |
|---|---|---|---|---|---|
| Stopped | launch; R2, R5 from stopped, R6, O4, O5, V3, V4, R7 | no transport sound (live instruments still sound, E3) | none | off | all |
| StartPending | a Play-family request sent | nothing yet | Play pending | off | PlayExact, PlaySelection, HalfSpeed (re-send); Toggle, Stop, Pause; Record; Locate (cursor only) |
| Playing | R1 | playback | none (outcome "Playing") | off | all but Play |
| RecordPending | Record sent | from stopped: nothing; punch: playback | Count-in when a count-in was sent, else Record pending | off | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| CountIn | R3 with a count-in | the click | Count-in | pulses on the beat | the Play family, Toggle, Stop, Pause, Record and HalfSpeed cancel it; Locate (take-safe) |
| WaitingForClock | R3 without a count-in; O2 | from stopped: nothing (parked); punch: playback | Record pending (B1) | lit | Toggle, Stop, Pause, Record (cancel); Locate (take-safe) |
| Recording | R4, O3, T2 | playback and capture | none (outcome "Recording") | lit | Toggle, Stop, Pause, Record (take finalized); Locate (take-safe) |
| StopPending, Park | a Stop-family request sent; V1, V2, T1 | playback until the audio thread applies Stop | Stop pending | off | Stop, Pause, Toggle (re-send); Locate (cursor); PlayExact, PlaySelection, HalfSpeed (play again) |
| StopPending, Resume | Locate during a pass | as Park | Stop pending | off | Stop, Pause, Record drop the resume; Locate retargets; others deferred |
| StopPending, Reset | ResetSession | as Park | none | off | as Park |

The Record button's lit set (WaitingForClock, Recording) is today's visible set except that it goes off at Stop instead of at the record-off receipt (B2).

### 5. The transition table

Events into the step (section 9): commands C (drained from `engine.transport_requests`, `engine/src/lib.rs:87-101`), receipts R (Player notices and frontiers, `host.rs:82-114`), observations O (sampled once per step), device events V, timers T. Columns: S Stopped, SP StartPending, P Playing, RP RecordPending, CI CountIn, W WaitingForClock, R Recording, ST StopPending (Park and Reset), STr StopPending (Resume).

Legend: a phase name = the phase entered (for a command, once its Player request is accepted: a local refusal or a Player error keeps the phase and posts the message, except `PlayerError::Faulted`, row R7); `—` ignored, no message; `·` cannot occur (no matching request is outstanding; the receipt is logged and dropped, as `host.rs:336-361` does today); `!` the pass ends here (`finish_recording`: a take with content is placed, an empty one discarded); `def` kept in order and handled after the pass resumes; `msg` message only; `½` at half speed. Refusal texts: Tr reopen (`host/reconnect.rs:120-135`), Ta no armed track plus the Record hint (`host/recording.rs:20-28`), Te "Cannot record: ..." (`:36`), Th `HALF_SPEED_WHILE_RECORDING` (`:738`), Td `REFUSED_RECORDING` (`audio_devices.rs:46`), Tp `refusal_text` (`host.rs:206`), Tf `start_failure_text` (`host.rs:201`). A tag Bn or Dn marks a cell that differs from today's behavior.

| Event | S | SP | P | RP | CI | W | R | ST | STr |
|---|---|---|---|---|---|---|---|---|---|
| C1 Play | SP (Tr) | — | — | — | ST | — D1 | — | — | def |
| C2 PlayExact(at) | SP | SP | SP | — D2 | ST D2 | — D2 | — D2 | SP | def |
| C3 PlaySelection | SP | SP | SP | — D2 | ST | — D2 | — D2 | SP | def |
| C4 TogglePlay | SP (Tr) | ST | ST | ST! | ST! | ST! D1 | ST! | ST, Stop re-sent | def |
| C5 Stop | S, Return to Zero | ST | ST | ST! | ST! | ST! | ST! | ST, Stop re-sent | ST, resume dropped |
| C6 Pause | ST | ST | ST | ST! | ST! | ST! | ST! | ST, Stop re-sent | ST, resume dropped |
| C7 Record | RP (Tr, Ta, Te) B7 | RP | RP punch | ST! | ST! | ST! | ST! | — | ST, resume dropped |
| C8 Locate(at) | S; cursor, position, retained := at | SP; cursor, retained := at; start not moved | P; Player locate; cursor, retained := at | STr(at)! | STr(at)! | STr(at)! | STr(at)! | ST; cursor, retained := at B2 | STr, target := at |
| C9 HalfSpeed | SP½ (Tr) B7 | SP½ | SP½ | Th | ST! | Th | Th | SP½ B2 | Th |
| C10 Scrub(at) | position := at until the next readback; phase unchanged, every column | | | | | | | | |
| C11 AllNotesOff | release request sent; phase unchanged, every column | | | | | | | | |
| C12 ResetSession | ST Reset | ST Reset | ST Reset | ST Reset! | ST Reset! | ST Reset! | ST Reset! | ST Reset | ST Reset D3 |
| R1 Play applied | · | P, "Playing" | · | · | · | · | · | · | · |
| R2 Play refused | · | S, Tp, speed Normal B7 | · | · | · | · | · | · | · |
| R3 Record admitted (applied, no latch) | · | · | · | CI if a count-in was sent, else W; B1 | · | · | · | · | · |
| R4 Record started (latch) | · | · | · | R, "Recording" | · | · | · | · | · |
| R5 Record refused | · | · | · | from stopped S!, Tp; punch P, Tp | · | · | · | · | · |
| R6 Stop frontier reached, not playing | · | · | · | · | · | · | · | Park: S, "Stopped"; Reset: S | RP at the target, deferred commands re-queued; B8 |
| R7 Player faulted | S, msg | S, msg | S, msg | S!, msg | S!, msg | S!, msg | S!, msg | S, msg | S, msg |
| O1 count-in read Some | — | — | — | — | CI, seen | — | — | — | — |
| O2 count-in read None after seen | — | — | — | — | W; B3 | — | — | — | — |
| O3 latch read | — | — | — | R, "Recording" | R, "Recording" | R, "Recording"; B1 | — | — | — |
| O4 start failure read | — | — | — | — | S!, Tf | from stopped S!, Tf; punch P, Tf | — | — | — |
| O5 Player stopped by itself | — | — | S | — | — | punch S! | S! | — | — |
| V1 output trouble | msg | ST, msg | ST, msg | ST!, msg | ST!, msg | ST!, msg | ST!, msg | msg | ST, resume dropped, msg |
| V2 input trouble | msg | msg | msg | ST!, msg | ST!, msg | ST!, msg | ST!, msg | msg | ST, resume dropped, msg |
| V3 driver reset or stall (reopen cause) | S, msg; B5 | S, msg; B5 | S, msg; B5 | S!, msg; B5 | S!, msg; B5 | S!, msg; B5 | S!, msg; B5 | S, msg; B5 | S, msg; B5 |
| V4 Player replaced (Apply, Reconnect, `ui.set {audio}`, a needed reopen) | S | S; B5 | S; B5 | Td | Td | Td | Td | S; B2, B5 | Td |
| T1 lap limit | — | — | — | — | — | — | ST!, note | — | — |
| T2 simulated count-in done | — | — | — | — | R | — | — | — | — |
| T3 simulated end reached | — | — | S | — | — | — | S! | — | — |

Notes on cells:
- C5 at S is today's second Stop: `transport.rtz` locates through the engine without an undo step and is handled before any later queued request (`host/transport.rs:234-242`). The host calls `engine.execute("transport.rtz")` itself; this skips the interface's `run` interceptor, which is equivalent because the only `run` rule that names `transport.rtz` is the real-time render refusal (`finish/realtime.rs:29-38`), and that refusal also refuses the Stop that leads here.
- C6 at S sends a Stop as today (`host/transport.rs:244`); ADR 0043's button table already ignores Pause while stopped before the command exists.
- C8 during a pass is today's take-locate (`host/transport.rs:259-266`): the pass ends, and once R6 arrives a fresh pass starts at `at` without a count-in (`host/recording.rs:48-53`).
- R6 waits for the stop frontier and for no outstanding record-off request, as `host/transport.rs:140` does; it no longer skips the rest of the step meanwhile (B8).
- A command that names a position takes it from the command or the step input, never from `ui.cursor` (section 8).
- The table is `next()`'s specification and the S2 matrix test's expected values (section 9).

### 6. Status text

- `state_text(&TransportPhase, release_pending: bool) -> Option<&'static str>` is the one function that produces "Play pending", "Record pending", "Count-in", "Stop pending" and "All MIDI notes off pending" (the last only when the phase has none). No other code writes these strings.
- `StudioApp::status` stays the message channel (failures, notes, outcomes); its 333 writers are unchanged. A message can never block or cancel a transition: `next()` takes no text.
- The displayed line is `status_line()`: the state text when there is one and `status` is empty or still equals `status_at_phase`, the copy of `status` taken when the phase last changed; otherwise `status`. So a message posted during a pending phase shows; when it ages out (`StatusAge::tick`, `menus.rs:507`) the state text returns; when the phase leaves, its text leaves.
- Outcomes "Playing", "Recording" and "Stopped" are written to `status` on R1, R4/O3 and R6 (Park) only when the displayed line was the state text, so a newer message stays visible. "All MIDI notes off" on the release frontier is written unconditionally, as today (`host.rs:384-387`).
- `is_state` (`status.rs:26-28`) is deleted: a state text is never in `status`, so nothing needs to stop it ageing. `ui.inspect`'s `status` and the status bar read `status_line()`; the diagnostics report keeps `StatusAge::last` (messages only).

### 7. Predicates

| Predicate | Definition | Today's readers that move to it |
|---|---|---|
| `recording()` | `Recording` | `TransportStatus.recording` (written once per step); arrange take preview `arrange.rs:1048`; `ui.inspect` `recording` `lib.rs:665`; Record button lit set with `WaitingForClock` `transport.rs:176, 197` |
| `pass_in_progress()` | `RecordPending`, `CountIn`, `WaitingForClock`, `Recording`, `StopPending { then: Resume(_) }` | new `TransportStatus.pass`; undo and redo `actions.rs:15`; width change `actions.rs:11`; engine `blocked_while_recording` `engine/src/lib.rs:769-771`; replace and quit `menus.rs:18`, `frame_logic.rs:148`; Reconnect enabled `menus.rs:402`; device changes `audio_devices.rs:511, 574`, `audio_devices/window.rs:164`, `frame_logic.rs:256`, `host/reconnect.rs:217`; `close_autosave` `host/autosave.rs:113`; recovery refusal `:153`; timed autosave skip `host/timed_read.rs:44`; loop retarget deferral `host/transport.rs:124`; finish-a-take checks `host/transport.rs:43, 53`, `host.rs:54`; Record toggle and take-safe locate `host/transport.rs:253, 261`; half-speed refusal `:285`; reopen wording and stops `host/reconnect.rs:29, 54, 157, 169`; MIDI dispatch `midi_transport.rs:221, 271, 283` |
| `stopped()` | `Stopped` | second-Stop test `host/transport.rs:228-232`; External Instrument and real-time render conditions `external_measure.rs:227`, `finish/realtime.rs:139`; window repaint rate `frame_logic.rs:530`; hardware measure start `hardware_measure.rs:187`; reopen stop `host/reconnect.rs:138`; engine `hardware_measurement` `engine/src/lib.rs:414`, `measure_blocker` `cmd/external_instrument.rs:45`, `realtime_plan` `cmd/realtime_render.rs:115` (these three read `!(playing || pass)` from `TransportStatus`) |
| `stopped_and_finalized()` | `Stopped`, no play, locate, stop or record request outstanding, no take pending in the recorder | MIDI transport recovery release `midi_transport.rs:204`; ADR 0043's sync mode changes (section 15) |
| `running()` | not `Stopped` and not `StopPending { then: Park \| Reset }` | MIDI dispatch `running` `midi_transport.rs:216-220` (a pending start, count-in or record start counts as running, ADR 0043 amendment) |
| `playing()` | the observed value of the step (B9) | `is_playing` `lib.rs:636-638`, `position` `:641-648`, `set_cursor` `:537`, `meters_live` `host.rs:47`, `watch_output` `host/reconnect.rs:29` |

`recording_active()` (`host/recording.rs:280-284`) is deleted; its readers are in the second row. `TransportStatus` gains `pass: bool`; `transport.status` (`engine/src/cmd/transport.rs:95`) reports it. Invariant pinned by a test: `recording` implies `pass`.

### 8. Positions

| Position | Owner | Written by | Read by |
|---|---|---|---|
| cursor (`ui.cursor`) | interface | the interface; on `HostEvent::CursorTo(at)` (C8, take-locate) and `HostEvent::SessionReset` | the step input `cursor` |
| transport position (`engine.transport.position`) | host | once per step: the Player's position while not `Stopped`, the step's `cursor` while `Stopped`; C10 until the next readback | `transport.status`, playhead drawing, engine `cur()` (`engine/src/cmd/transport.rs:101-103`) |
| retained (`midi_transport::State.retained`, `midi_transport.rs:59`, today) | host | R6 Park (the applied stop position, before parking), C2 and C8 (the target), C12 (0) | Continue (`transport.play {from: retained}`); the MIDI locate duplicate test |

- `set_cursor` (`lib.rs:532-540`) stops writing the transport position; the next step publishes it (B11).
- Play reads the cursor from the step input (`play_from_cursor`, `host/transport.rs:71-81`); Record reads it the same way (`begin_pass`, `host/recording.rs:82`); R6 Park parks at the step's `cursor`, so a cursor moved while the stop was pending is where the transport parks, as today (`host/transport.rs:66`).

### 9. The step

**Time is an input.** `StepInput { dt: f64, frame_now: f64, wall_now: Duration, cursor: Samples, prefs: HostPrefs }`. `dt` and `frame_now` come from the caller's frame clock (the interface passes `frame_logic.rs:323`'s values); `wall_now` is a monotonic duration from the caller's clock for the two-minute timed autosave, which must run while the window's frame clock stands still (`host/timed_read.rs:37-39`). `HostPrefs` copies the preference values the host reads: count-in bars and switch, click level, monitor settings, audio configuration, low-latency limit, MIDI transport profiles. The host never calls `Instant::now()` or reads `UiState`.

**Order within one step** (S2 inside `logic()`; S3b inside `Host::step`):
1. Receipts: Player notices in the order `poll_notices` returns them (the audio thread's application order), then the stop and release frontiers. A receipt whose sequence is not the outstanding one of its kind is stale (`·`).
2. Observations, sampled once: playing, position, `count_in()`, start failure, latches (`Recorder::applied_start`, `Player::midi_recording_latch`). O-rows fire in the order O4, O3, O2, O1, O5.
3. Device events (V-rows) and the lap limit (T1), in today's `logic()` order (`frame_logic.rs:337, 355-357`).
4. Queued commands, first in first out; each sees the phase the previous one left.
5. MIDI transport buttons, in arrival order: each is decided against the current phase, run through `run` (the interceptor) into a command, and applied at once, so two queued Play or Record messages never both see the old phase (ADR 0043 amendment). `midi_transport_feed` moves from `ui()` (`frame_logic.rs:467`) to this point of `logic()`; `dispatch_midi_buttons` replaces its `handle_transport(0.0)` (`midi_transport.rs:270`) with the command-applying half of the step. Nothing drives the transport from the drawing pass.
6. Timers: the simulation advances by `dt` times the speed; the simulated count-in counts down.
7. Publication, once: `TransportStatus { playing, recording, pass, position }`, meters at most every 50 ms of `frame_now` (`host/transport.rs:324`).

A receipt for a request sent in this step is handled in a later step. A Stop clears the outstanding Play and Record sequences (`host/transport.rs:38-40`), so their late receipts are stale.

**Events out** (`HostEvent`), emitted in the order the transitions happened; the interface drains them after the step, in order. A `PhaseChanged` precedes the `Outcome` of the same change.

| Event | The interface's reaction (it keeps all view state) |
|---|---|
| `PhaseChanged { from, to }` | copies `status` into `status_at_phase` (section 6) |
| `Outcome(Playing \| Recording \| Stopped \| NotesOff)` | writes the outcome text by section 6's rule |
| `Message(String)` | writes `status` |
| `CursorTo(Samples)` | `set_cursor` |
| `SessionReset` | clears cursor, cursor track, focus, gestures, copies, automation selection, meter displays (today `host/transport.rs:191-204`) |
| `TakePlaced { clips, focus, take_lanes }` | `edit.select` of the clips, focus track, `show_takes` (today `host/recording.rs:631, 640-643, 653, 711-715`) |
| `PickupsBefore(Samples)` | `scroll_back_to` (today `host/recording.rs:493-503`, #707) |
| `RecordHint(String)` | stores it with its own `frame_now` (today `host/recording.rs:27`, `midi_transport.rs:290`) |
| `EditorsReopened` | re-wants the open editor windows after a Player replacement (today `host/reconnect.rs:231-235`) |

**The pure core.** `next(phase: &TransportPhase, event: &TransportEvent, facts: &TransportFacts) -> Next { to: Option<TransportPhase>, effects: Effects }` decides; an executor performs the effects (Player calls, `finish_recording`, events). A Player request's result returns as `Sent(kind, seq)` or `NotSent(text)` and is fed to `next()` in the same step; the matrix's command cells are the phase on `Sent`. `next()` takes no strings and no Player.

### 10. The behavior changes of slice 2

"Read" means confirmed by reading `main` 1a56b895; every one is confirmed again by its test's negative control, run on the S1 commit, before the fix. None was found unreal, so none is dropped.

| # | Today | Confirmed | Instead | Affected | Test (S2) |
|---|---|---|---|---|---|
| B1 | A count-in admission and a start waiting for the device clock both answer `ActionResult::Applied` (`playback/src/player/audio.rs:412`; `:299, :340`); any applied Record receipt sets `engine.transport.recording = true` (`host.rs:349-357`), and the wait shows "Recording" (`:354-355`) | read | R3 enters CountIn or WaitingForClock; the flag is set only by R4/O3; the wait shows "Record pending" | `transport.status` and `ui.inspect` `recording` (false until the latch); status text while waiting; every flag reader keeps its answer through `pass` or `stopped()` (section 7) | `recording_flag_is_false_through_the_count_in_and_true_from_the_latch` (device-free); `an_admitted_record_without_a_latch_is_waiting_not_recording` (a hand-built Applied receipt, as `player_tests.rs:9` builds them) |
| B2 | With a Player, `finish_recording` never clears the flag (`host/recording.rs:375-377`); it stays true until the record-off receipt or `complete_stop` (`host/transport.rs:63`), so `recording_active()` gates device changes, undo, recovery and the timed autosave during "Stop pending" | read | The take ends at Stop (`finish_recording` already places it at once, `host/transport.rs:43-45`); StopPending is not a pass | the readers in section 7's second row, during StopPending only (usually one audio block); the Record light goes off at Stop | `stop_ends_the_pass_at_once_and_device_changes_are_allowed_while_the_stop_is_pending` (device-free, fake connection) |
| B3 | "Count-in" is set when Record is sent (`host/recording.rs:153`); the next frame's check (`host/transport.rs:165-167`) reads `count_in_state()` None before the audio thread admits the count-in (it publishes at the end of a block, `audio.rs:876-880`) and writes "Recording" | read | CountIn leaves for WaitingForClock only after `count_in()` has read `Some` (O1, O2), or for Recording on the latch | status text at a count-in start | `count_in_text_stays_until_the_audio_thread_publishes_and_ends_its_count_in` (host steps without a device step) |
| B4 | The text promotions at `host.rs:343, 354, 380` and `host/transport.rs:165` fire only if the line still holds the state text. By reading, no transport transition is gated by text, only the displayed state: after another message, the line never shows the outcome, and once that message ages out it is blank while still pending | read (restated: the inventory said a transition does not fire) | section 6: the state text is derived and returns when a message ages out; transitions never read text | status line | `a_message_during_a_pending_start_neither_strands_nor_hides_the_state` |
| B5 | `PlayerRequests::reset()` (`host.rs:280-283`; called at `host/reconnect.rs:142, 245` and on `Faulted`) never clears the text; Reconnect while playing sends Stop ("Stop pending", `host/transport.rs:41`), replaces the Player, resets the requests and, when the state was captured, writes nothing (`host/reconnect.rs:107-112`); `is_state` keeps "Stop pending" up for good. The same holds for "Play pending" | read | V3 and V4 enter Stopped; the text is derived, so it leaves | status line | `reconnect_while_playing_leaves_no_pending_text` |
| B6 | A Record from stopped also sets `player_requests.play` (`host/recording.rs:150-152`); `transport_wait()` then pulses Play with "Play pending" (`transport.rs:119-132`) during "Record pending" or "Count-in" | read | `play_pending()` is `StartPending` only; Record no longer sets the Play request | Play button pulse and tooltip; the Play click guard (`transport.rs:191`); MIDI `running` (unchanged: `running()` covers RecordPending) | `a_record_start_does_not_pulse_play` |
| B7 | `HalfSpeed` stores 0.5 before starting (`host/transport.rs:289-292`); a start refused by `refuse_for_reopen` (`:12`), `sync_player_session` (`:15`), the Player (`:21`) or its receipt (`host.rs:362-366`) leaves 0.5 until a stop, an end or a reset (`host/transport.rs:35, 178, 315`), and Record is then refused "at half speed" while stopped (`host/recording.rs:11-15`). The simulation ignores speed (`host/transport.rs:359-361`) | read | the speed invariant (section 3); the simulation advances by `dt` times the speed | Record after a refused half-speed start; `--no-audio` playback position | `a_refused_half_speed_start_leaves_normal_speed_and_record_works`; `the_simulated_transport_plays_at_half_speed` |
| B8 | While a take-locate waits, `handle_transport` returns at `host/transport.rs:139-142`, skipping the click and monitor pushes, the count-in check, position and meter readback and natural-end detection | read | StopPending (Resume) is a phase; the step runs whole | playhead, meters, click level while a locate finalizes a pass | `a_take_locate_keeps_reading_back_position_and_meters` |
| B9 | `is_playing()` reads the atomic (`lib.rs:636-638`); `engine.transport.playing` is copied at `host/transport.rs:317`; `set_cursor` (`lib.rs:537`), `meters_live` (`host.rs:47`), `watch_output` (`host/reconnect.rs:29`) and `stop_for_reopen` (`:138`) read the copy, others the atomic, which the audio thread can change between them | read | the step samples the atomic once; every reader, the copy included, reads that value until the next step | all `playing` readers (section 7) | `every_reader_sees_the_playing_value_of_the_last_step` |
| B10 | `count_in_plan` outlives the count-in until `complete_stop` (`host/transport.rs:58`); harmless because `count_in_label` and `beat_phase` also check `count_in_state` (`metronome.rs:50-51, 60-61`) | read; real, harmless | the plan lives in phase data (`RecordPending`, `CountIn`, `WaitingForClock`, `Recording`) and leaves with it | none visible | `the_count_in_plan_leaves_with_the_pass` |
| B11 | Three positions (`midi_transport.rs:59`, `engine.transport.position`, `ui.cursor`); `complete_stop` sets retained from the position, then the position from the cursor (`host/transport.rs:52, 66`); `set_cursor` writes the transport position when stopped (`lib.rs:537-539`) | read | section 8 | `transport.status` position right after a stopped cursor move (now after the next step) | `a_stopped_cursor_move_reaches_the_transport_position_at_the_next_step`; `continue_after_stop_uses_the_applied_stop_position_not_the_cursor` |

**Related defects found while writing this record** (same code path; proposed as task lines on #739, fixed in S2 if the coordinator agrees):
- **D1.** Play or Toggle while a start waits for the device clock from stopped: `play.is_none()` and `!is_playing()` hold (the Record receipt cleared the request, the transport is parked), so `play_from_cursor` runs (`host/transport.rs:218-219, 246-249`); its Play withdraws the waiting start on the audio thread (`audio.rs:452` calls `end_count_in`, which cancels the waiting ticket, `:415-422`) while the host keeps the plan. Instead: C1 ignored, C4 cancels like Stop. Test: `play_while_waiting_for_the_clock_does_not_drop_the_record_start` (pure matrix plus a hand-built receipt).
- **D2.** `PlayExact` and `PlaySelection` have no pass guard (`host/transport.rs:222-225, 278-279`): during a pass they send Play, which ends a count-in or a waiting start on the audio thread while the host keeps the plan, and during a take moves the transport without a take-locate. Instead: ignored during RecordPending, WaitingForClock and Recording; PlayExact cancels a count-in like Play. Test: `play_from_a_position_during_a_pass_leaves_the_pass_alone`.
- **D3.** `ResetSession` during a take-locate is held by the early return with the other requests, so the resumed pass starts on the new session before the reset is handled (it is then refused, since arm state is not saved, ADR 0011). Instead: C12 drops the resume. Test: `a_session_replaced_during_a_take_locate_does_not_resume_recording`.

### 11. The crate boundary

| Moves to `transit-host` | Slice |
|---|---|
| `host/phase.rs` (S1, S2: `TransportPhase`, `AfterStop`, facts, `next()`, `state_text`, predicates); `PlayerRequests` and its receipt types (`host.rs:263-396`); the pure texts (`refusal_text`, `start_failure_text`, `failed_plugin_status`, `plugin_error_status`, `record_speed_refusal`, `before_count_in_note`, `HALF_SPEED_WHILE_RECORDING`); `midi_transport::decode` and the button state table; the count-in arithmetic of `count_in_for` as a function of the session, a position and `HostPrefs` | S3a |
| `Host` holding the Player, recorder and requests; `host/transport.rs`, `host/recording.rs` (minus the view edits, now events), `host/autosave.rs`, `host/plugin_state.rs`, `host/reconnect.rs` orchestration, `host/state_guard.rs`, `host/timed_read.rs` (its gate takes the pointer-down flag as a `StepInput` fact), the non-drawing parts of `midi_transport.rs` (to `:304`), `metronome.rs` (to `:96`), `transport.rs` (to `:134`), `RecordClock`, `MidiLive`, `ShutDown`, the simulation | S3b |

| Stays in the interface | Why |
|---|---|
| cursor, scroll, focus, selection, take-lane visibility, gestures | view state (decision 7) |
| the status line: message channel, ageing, `status_line()` | display |
| `feed_meter` ballistics (`host.rs:146`) | display; the host hands out the snapshot |
| `run` and its interlocks, MIDI button cancellation and capture-before-save rules (`frame_logic.rs:17-189`) | command interception for features that stay here |
| measurement windows, Audio Devices window draft and base, editor windows and groups, finish dialogs, preferences (`UiState`, ui.json) | window features |

**Read-only handles** (`&self` methods of `Host`): `phase()`, `playing()`, `position()`, `retained()`, `count_in()`, `count_in_plan()`, `meters() -> &MeterSnapshot`, `midi_live() -> Option<&MidiLive>`, `live_clock()`, `record_start()`, `take_plan()`, `take_cycle()`, `recording_pass_inspect()`, `record_hint_text()`. For the services that are not transport (editors, plug-in state jobs, presets, telemetry, MIDI Learn and Monitor, the I/O map, measurements, the real-time render), `player() -> Option<&Player>`; `player_mut()` only for `apply_calibration` (`hardware_measure.rs:297`). Borrowing is not holding: section 14's check forbids storing a Player or Recorder and calling the transport methods.

**Edges.** `transit-ui-studio` (L6) -> `transit-host` (L5) -> `transit-engine`, `transit-playback` (L4). S4: `transit-automation` (L5) -> `transit-host` (L5). Apps are exempt (`xtask/src/main.rs:119`); `apps/transit` creates the first Player and hands it to `Host::new`.

**Tests.** No existing test moves in S3a or S3b: the 37 test files that reach into moved fields keep their assertions and drive `StudioApp`, which delegates (`poll_player`, `handle_transport`, `is_playing`, `position` stay as delegating methods). In S3b their field paths change mechanically (`app.player` to `app.host.player()`, `app.player_requests` to `app.host.requests()`, likewise `take_plan`, `midi_transport`, `count_in_plan`, `sim_count_in`, `record_start`, `take_cycle`, `recorder`); the reviewer checks that the test diff is only those substitutions. What moves: in S3a the pure tests (`host/phase_tests.rs`, the matrix; the requests tests that S2 rewrote) go to `crates/host/src/`, unchanged. Tests needing a window (`transport_tests.rs`, `slice6_tests.rs`, `pre_count_ui_tests.rs`, `tests/stop_return.rs`, `plugin_retry_tests.rs`, `emergency_tests.rs`, parts of `midi_record_tests.rs` and `state_guard_tests.rs`) stay. Tests with no window (`live_cycle_tests.rs`, `zone_record_tests.rs`, `midi_pickup_tests.rs`, `recovery_tests.rs`, `midi_transport/tests.rs`, `host/reconnect_tests.rs`) may be ported to `Host` after S4; not required by this record.

### 12. Blocking calls on the interface thread

The host stays on the Player's thread. A call becomes non-blocking by being stepped, by moving only the work that does not hold the Player to a worker, or it is staged behind a visible pending fact and named out of scope.

| # (inventory section 5) | Call | Target | Slice |
|---|---|---|---|
| 1 | device reopen: `reopen_audio` (`audio_devices.rs:527`) from Apply (`:509`), `ui.set {audio}` (`frame_logic.rs:291`), Reconnect (`host/reconnect.rs:104`), the automatic reopen (`:173`), a needed reopen inside `sync_player_session` (`host/plugin_state.rs:40-41`, reachable from Play) | staged: a `ReopenPending` device fact, state text "Reopening the audio device", set in one step; the reopen runs in the next, so the window paints the text first; a Play that needed the reopen is held and applied after it. The open itself (`audio_connection.rs:75`, `Player::with_config_and_monitor`) and `preserve_audio_state` hold the Player: out of scope, reopen condition below | S5b |
| 1a | after a reopen, `save_good_audio`'s ui.json write (`audio_devices.rs:325-356`) | worker (writes a file; holds no Player) | S5a |
| 2 | `io.measure_hardware_latency` (`frame_logic.rs:114-115`) sleeps 20 ms per poll up to 8 s (`hardware_measure.rs:86-94, 21`) | stepped: the control request starts `start_measure` (`:98`) and is parked; each step runs `step_measure` (`:136`); the control channel answers when it finishes. A parked reply, not a new method: clients see the same reply later | S5a |
| 3 | `wait_for_plan`, 2 ms polls up to 150 ms (`host/state_guard.rs:215-241`, `PLAN_WAIT` `:87`) | out of scope: bounded, holds the Player; reopen if a stall over one frame budget is measured | none |
| 4 | plug-in state capture (`host/plugin_state.rs:105`, 500 ms modal deadline `playback/src/player/handles.rs:31`) | out of scope: holds the Player and the plug-in host thread (ADR 0018); reopen with an ADR 0018 amendment for asynchronous capture | none |
| 5 | `Autosaver::flush` (`engine/src/autosave.rs:83-86`) via `autosave_to` (`host/autosave.rs:89-97`) | the timed write becomes queue and poll by ticket, as the journal steps already do (`host.rs:252-261`): non-blocking. The panic path (`host/autosave.rs:143`), `close_autosave` (`:127-130`) and `recover` (`:166`) keep the flush: they must complete before what follows (quit, replace); reopen if a measured quit or replace waits on it | S5a |
| 6 | saves and engine commands inside `engine.execute` (`frame_logic.rs:160`) | out of scope: engine command execution; a separate record | none |
| 7 | input open: `ensure_input` (`host.rs:118-141`) on Record and each frame live input is needed | staged as 1: an `InputOpening` fact, the Record held one step; the open holds the Player: out of scope | S5b |
| 8 | `machine_listing` (`audio_devices.rs:716`) | worker, as the ASIO listing already is (`:731`) | S5a |
| 9 | quit: `release_native_drivers` up to 10 s (`host.rs:75-78`) | out of scope: the quit path under the emergency bound | none |
| 10 | file pickers | #39 | none |

Reopen condition for the out-of-scope parts: a record that lets a Player be created or dropped on a worker and handed over.

### 13. Headless play and record (S4)

- `Headless` (`crates/automation/src/lib.rs:30`) holds a `Host` and stops clearing `transport_requests` (`:53-54`). After each `engine.execute` it steps the host; `dt` and `wall_now` come from its own clock (production: a monotonic clock in `transit-automation`, which is the caller, so decision 5 holds) or from an injected clock (tests).
- Production Headless opens `Player::with_null_device` (`--null-audio`'s Player, paced by its own thread). Tests use a device-free Player or `with_null_device_clock` with `StepClock` (`test-hooks`).
- `transport.status` gives `playing`, `recording`, `pass`, `position`; `ui.*` stays refused (`:61-63`).
- Consequence: `transit-automation`, and `transit-cli` through it, link `transit-playback` and its backends.
- Open point for S4: whether the protocol needs an explicit step or wait method for clients that poll a pending start; decided with S5a's parked replies.

### 14. The guard

- **(a) Layer rule (S3a).** `("transit-host", 5)` in `LAYERS`; `check_layers` bars UI crates and upward edges (section 2). A test in `xtask/src/tests.rs` (beside the existing `check_layers` cases) asserts a fixture `transit-host` depending on `egui` fails.
- **(b) Source check (S3a, tightened in S3b).** New `xtask/src/host_guard.rs`, run by `layers()` after `unsafe_modules` (`xtask/src/main.rs:262-267`). It scans every `.rs` file under `crates/ui-studio/src` except test files (a name ending `_tests.rs` or equal to `tests.rs`, any file under a `tests/` directory) and `crates/ui-studio/tests/`. On each line, text from the first `//` on is dropped. Two rules:
  - **Holding**, regexes: `\bPlayerRequests\b`; `Option<\s*(transit_playback::)?Player\s*>`; `:\s*(transit_playback::)?Player\b`; `Option<\s*(transit_playback::)?(record::)?Recorder\s*>`; `:\s*(transit_playback::)?(record::)?Recorder\b`; `\bPlayer::(with_|open|prepare_)`. A borrow (`&Player`) does not match.
  - **Transport calls**, regex: `\.(play|stop|locate|set_loop|set_recording|record|record_with_count_in|record_midi_with_count_in|set_speed|all_notes_off|prepare_recording|prepare_midi_recording|finish_midi_recording|poll_notices|recovery_progress|take_start_failure|monitored_input|close_input)\(`.
  - Both use a ratchet table `HOST_GUARD: &[(&str, usize)]` (repo-relative path, allowed matches), as `file_size::OVER_BUDGET` does (`xtask/src/file_size.rs:11-38`): a file over its count, an unlisted file with a match, a listed file under its count, or a listed file that no longer exists fails, each with a message naming the file.
- **Tightening.** S3a lands the table with today's counts (the builder records them; `lib.rs` and the `host/` files are the expected rows). S3b removes every row for the files it moves and for `lib.rs`; what remains are matches on other objects (for example the browser preview player, `browser/preview.rs:178, 187`), each with a comment saying what it is. Later slices only remove rows.
- Unit tests in `xtask/src/tests.rs`: a stored `Option<Player>` fails; `fn f(p: &Player)` passes; a mention in a comment passes; a listed preview `.play(` passes at its count and fails at one more; an unlisted `.set_speed(` fails.
- The check does not cover blocking at run time; S5's tests do.

### 15. Relation to ADR 0043

| ADR 0043 requirement (inventory section 3) | Here |
|---|---|
| 1. per-input sync mode, changed only while stopped and finalized | `stopped_and_finalized()` given by S2; the mode and its preferences are ADR 0043's |
| 2. selected-source Start/Continue/Stop/SPP machine with its own start pending | `TransportPhase::StartPending`, `next()` and the matrix given by S2; retained position as host state given by S2 (B11); exact start exists (`PlayExact`); the source's rows are ADR 0043's |
| 3. duplicate suppression against a pending locate in the current epoch | ADR 0043's, using the locate-pending fact and the Locate receipt (S1's receipt type) extended with the epoch the audio thread sets (`audio.rs:361, 411`) |
| 4. record engaged, "Waiting for external start" | ADR 0043's, as one `TransportPhase` variant or fact entered through `next()` with its text from `state_text()`; B1 (S2) is its precondition: recording means recording |
| 5. local Stop cancels queued starts by source generation; sticky Stop | ADR 0043's, using the step's input order (section 9) and a `TransportEvent` variant |
| 6. half speed and scrub refused in clock follow | ADR 0043's, as a condition on rows C9 and C10 using the speed fact |
| 7. transport bar readout through `TransportStatus` | ADR 0043's; S2 adds `pass` to the same struct |
| 8. applied starts and stops with generations for sending | applied moments given by S2 (`PhaseChanged` from receipts); generations are ADR 0043's |
| 9. slice 3 refusals, tempo editing, journal mapping segments | unchanged; ADR 0043's |
| 10. a deterministic state matrix without a window | given by S2 (the pure matrix test) and S4 (headless end to end) |

**What sync slice 1 extends:** `TransportPhase` (a variant for record engaged, if a phase), `TransportEvent` (a variant for a selected-source message carrying source generation and sequence), `next()` and its matrix test, `state_text()`, and `TransportStatus` (the readout). Before S3a they are in `crates/ui-studio/src/host/phase.rs`; after, in `crates/host/src/phase.rs`.

**Statements that become stale.** No decision of ADR 0043 becomes false. Two path statements do when S3b lands: "`crates/ui-studio/src/host/transport.rs` (exact-position starts and independent retained position)" (Implementation) and "Slice 0 must adapt `crates/ui-studio/src/host/transport.rs`" (2026-11-04 amendment). They need a pointer amendment naming `crates/host/src/transport.rs`, written with S3b. Its Context sentence "`TransportStatus` reports playing, recording and session-sample position" describes the base it was written on and stays.

### 16. Relation to ADR 0052

ADR 0052's count-in and latch statements rely on the audio thread (capture from the count-in start, the latch at the output frame presenting the record start, decision 8 and its 2026-11-16 amendment), on `count_in_for` measuring the bar containing the start (decision 3), on `plan_recording` writing `count_in` into the plan (decision 8) and on `ui.inspect`'s `recording_pass` (decision 12). None changes: the audio thread is untouched, the count-in arithmetic moves without change in S3a, and the plan and `recording_pass` come from the same data. Path statements become stale at S3a and S3b and need the same pointer amendment: `count_in_for` in `crates/ui-studio/src/metronome.rs` (decision 3), `play_from_cursor` and `wanted_loop` in `crates/ui-studio/src/host/transport.rs` (decisions 5, 7), `begin_pass` and the #440 paragraph's `crates/ui-studio/src/host/recording.rs` (decisions 4, 8).

## Slices

Every commit builds on its own; every slice runs the full local Windows and Linux gates. "Pass unchanged" means the test file is not edited.

### S1: derive the phase, typed receipts (no behavior change)

- **Scope.** New `host/phase.rs`: `TransportPhase`, `AfterStop`, `state_text()`, and `StudioApp::transport_phase(&self) -> TransportPhase` derived, read-only, from today's flags in this order: `player_requests.stop` Some -> StopPending (Resume when `midi_transport.take_locate` is Some, else Park); `take_locate` Some -> StopPending Resume; `take_plan` Some and `player_requests.recording` Some -> RecordPending; `take_plan` Some and `count_in_state()` Some -> CountIn; `take_plan` Some and a latch (`Recorder::applied_start` or `Player::midi_recording_latch`; without a Player, the flag) -> Recording; `take_plan` Some -> WaitingForClock; `player_requests.play` Some -> StartPending; `is_playing()` -> Playing; else Stopped. In `host.rs`, `PlayerRequests::notice` splits into `classify(notice) -> Receipt` (`Receipt::{Session { rejected }, Play { applied }, Record { applied, latch, payload }, Locate, Other}` with the refusal text) and a wrapper `notice(notice, &mut String, &mut bool) -> bool` that applies today's exact text and flag rules to the `Receipt`; `recovery` splits into `frontiers(..) -> Frontiers { stop_reached, release_reached }` and a wrapper. The wrappers keep `player_tests.rs` unchanged.
- **Files.** `host.rs`, `host/phase.rs` (new), `host/phase_tests.rs` (new).
- **Size.** About 150 lines of code and 200 of tests.
- **Not in it.** No reader changes; nothing writes a phase; no text, flag or ordering change.
- **Gate.** Pass unchanged: `player_tests.rs` (8), `transport_tests.rs` (20), `host/live_cycle_tests.rs` (14), `tests/stop_return.rs` (4), `midi_transport/tests.rs` (11), `slice6_tests.rs` (36), `pre_count_ui_tests.rs` (17), `midi_record_tests.rs` (29), `zone_record_tests.rs` (7), `midi_pickup_tests.rs` (7), `recovery_tests.rs` (8), `status.rs` tests, `plugin_retry_tests.rs` (7), `host/reconnect_tests.rs` (1), `host/state_guard_tests.rs` (10) and rounds 2 and 3, `emergency_tests.rs` (3), engine `tests/transport_buttons.rs` (2) and `tests/session_transport.rs` (4). New (device-free, live_cycle style, each asserting the phase beside today's status assertion): `derived_phase_follows_a_play_start_to_playing`, `derived_phase_follows_a_stop_to_stopped`, `derived_phase_follows_a_record_start_without_a_count_in`, `derived_phase_follows_a_count_in_to_recording`, `derived_phase_shows_a_take_locate_as_stop_pending_resume`, `derived_phase_follows_the_simulated_count_in`; and `state_text_names_exactly_the_four_waits_and_notes_off`.
- **Feel-right.** None: nothing visible changes.

### S2: the phase is authoritative; text derived; B1 to B11

- **Scope.** `next()` and `TransportFacts` in `host/phase.rs`; `StudioApp` holds the phase and facts; every writer of the four state strings, of `engine.transport.recording` and of `player_requests.play` for a record start goes through the executor; section 6's display; section 7's predicates and `TransportStatus.pass`; section 8's positions; section 9's order, with `midi_transport_feed` moved into `logic()`; B1 to B11 as section 10; D1 to D3 if the coordinator adds them. `recording_active()`, `is_state` and `count_in_plan` are deleted. Commit order: (1) `next()`, facts and the matrix test, unused; (2) the phase authoritative with today's texts derived; (3) predicates and `pass`; (4) one commit per B and D item with its test.
- **Files.** ui-studio: `host.rs`, `host/phase.rs`, `host/transport.rs`, `host/recording.rs`, `host/reconnect.rs`, `host/autosave.rs`, `host/timed_read.rs`, `lib.rs`, `status.rs`, `transport.rs`, `metronome.rs`, `midi_transport.rs`, `frame_logic.rs`, `menus.rs`, `actions.rs`, `arrange.rs`, `external_measure.rs`, `finish/realtime.rs`, `hardware_measure.rs`, `audio_devices.rs`, `audio_devices/window.rs`. Engine: `src/lib.rs`, `src/cmd/external_instrument.rs`, `src/cmd/realtime_render.rs`.
- **Size.** About 700 lines of code changed and 700 of tests.
- **Not in it.** No crate move; no time input; no headless; no blocking change; the interlocks stay in `run`.
- **Gate.** Pass unchanged: S1's list except the edits named next. Named edits: `player_tests.rs` rewritten to `classify` and `frontiers` (the wrappers go), same cases; `status.rs` `transport_waits_stay_until_the_transport_code_clears_them` (`:147`) replaced by `a_state_text_stays_while_its_phase_holds_and_leaves_with_it`; assertions on state texts read `status_line()` instead of `status`: `slice6_tests.rs` (`:404, :422, :455, :462-463`), `transport_tests.rs` (`:317`) and `plugin_retry_tests.rs` (`:330`), which also set the phase to StartPending where they set `player_requests.play`; the 23 sites in 10 ui-studio test files that set `engine.transport.recording = true` to stand in for a pass (`arrange/recording_strip_tests.rs`, `audio_devices_tests.rs`, `midi_pickup_tests.rs`, `midi_record_tests.rs`, `midi_transport/tests.rs`, `recovery_tests.rs`, `take_lanes_tests.rs`, `track_actions_tests.rs`, `transport_tests.rs`, `width_history_tests.rs`) set the Recording phase through a test helper; engine `tests/recording.rs:8` and the `realtime_render.rs` test (`:211`) set `pass`. Any other expectation that moves stops the slice for a ruling. New: `transport_phase_matrix` (pure: every phase, Park and Reset included, against every event of section 5, asserting the resulting phase or refusal and the effect kinds, the expected table written as a literal), `recording_implies_pass`, the eleven B tests and the D tests of section 10, each with its negative control run on the S1 commit and its failing assertion recorded in the result note.
- **Feel-right.** Record from stopped with a one-bar count-in: Play does not pulse; the line reads Count-in, then Recording; at Stop the Record light goes off at once and the line reads Stop pending, then Stopped; Reconnect Device while playing leaves no Stop pending on the line.

### S3a: the pure machine moves into `transit-host`

- **Scope.** Create `crates/host` (`Cargo.toml` with the workspace lints, `src/lib.rs`), register it in `LAYERS`, move section 11's S3a row as pure moves, `transit-ui-studio` depends on it; land `host_guard.rs` with today's counts.
- **Files.** `Cargo.toml` (workspace member and dependency), `crates/host/**` (new), `crates/ui-studio/Cargo.toml`, the files the items leave, `xtask/src/main.rs`, `xtask/src/host_guard.rs` (new), `xtask/src/tests.rs`.
- **Size.** About 1,000 lines moved, 250 new.
- **Not in it.** The Player stays in `StudioApp`.
- **Gate.** All S2 tests pass unchanged, the moved pure tests unchanged in their new place; `cargo xtask layers`; the guard's unit tests.
- **Feel-right.** None.

### S3b: the Player moves into `Host`

- **Scope.** `Host` as section 2 and 11; `StudioApp` holds `host`; `StepInput` (time, cursor, prefs, pointer-down); view edits become `HostEvent`s; `apps/transit` hands its Player to `Host::new`; the guard's rows for moved files and `lib.rs` removed; the pointer amendments of ADR 0043 and ADR 0052 (sections 15, 16).
- **Open points.** Split by concern (transport and recording first, then autosave, plug-in state, reconnect and timed read) if one review sitting cannot hold it; the exact `HostPrefs` field list.
- **Size.** About 3,000 lines moved; 49 non-test ui-studio files and 37 test files change paths.
- **Not in it.** Any behavior change.
- **Gate.** Every test passes with only the mechanical path substitutions of section 11, checked in review; the guard at its final table; a test that `Host` steps with a device-free Player and no `egui` type in scope (it lives in `crates/host`).
- **Feel-right.** None: nothing visible changes.

### S4: headless play and record

- **Scope.** Section 13.
- **Files.** `crates/automation/Cargo.toml`, `crates/automation/src/lib.rs`, its tests; `apps` call sites that construct `Headless`.
- **Size.** About 300 lines.
- **Gate.** New: headless `transport.play` then steps reach Playing; `transport.record` with an armed MIDI track and a count-in reaches Recording through CountIn with `recording` false before the latch; Stop places the take; the S2 matrix run end to end through `Headless` for the command and receipt rows a null Player can produce.
- **Open point.** Section 13's step or wait method.
- **Feel-right.** None in the window.

### S5: blocking work off the interface thread

- **S5a.** The parked control reply for the hardware measurement, the timed autosave by ticket, listing and the ui.json write on workers (section 12 rows 1a, 2, 5, 8). Gate: a control-channel measurement returns its reply after `step_measure` completes with the interface stepping meanwhile (a fake probe, stepped clock); the timed autosave never calls `flush`; existing `audio_devices_tests.rs` and `recovery_tests.rs` pass unchanged.
- **S5b.** Staged reopen and input open (rows 1, 7). Gate: Apply sets `ReopenPending` and shows its text in a step before the reopen runs (fake connection); a Play that needs a reopen is held and then starts.
- **Open points.** The exact texts; whether the held Play gets a timeout.
- **Feel-right.** Apply in Audio Devices shows "Reopening the audio device" before the window waits; a hardware latency measurement from the control channel no longer freezes the window.

Order: S1, S2 (before sync slice 1); S3a, S3b, S4, S5a, S5b in order, each beside sync.

## Consequences

- One value answers "what is the transport doing"; status strings are display only; a stray message can no longer strand or hide a state.
- `recording` means a take is being captured; questions that meant "a pass is under way" say so (`pass`); the control channel's `recording` turns true later (at the latch) and `transport.status` gains `pass`.
- Device changes, undo and recovery are allowed while a stop is pending after a take, because the take is already placed.
- The interface crate loses its Player; window code borrows it for services and cannot store it or drive the transport (section 14).
- Headless can play and record, so ADR 0043's gate and later agent tests run without a window; `transit-cli` links the audio backends.
- The host still runs on the interface thread, so a slow driver open still holds the window (section 12).
- About 4,000 lines move; paths in ADR 0043 and ADR 0052 need a pointer amendment at S3b.

## Alternatives rejected

- **(i) Type the phase in place and never move.** Fixes B1 to B11 but leaves nothing headless and the window types beside the machine; ADR 0043's gate needs the machine without a window, and the guard has nothing to guard.
- **(ii) The host in `transit-engine`.** Same layer as playback, so the layer check would allow the edge; rejected because every engine user (`transit-cli`, the headless backend, the engine's own tests) would link the device backends and plug-in hosts, and the document crate would gain the Player's thread rule, which it does not have today.
- **(iii) A host actor on its own thread.** The Player is neither Send nor Sync by design (Context); revisit only with a record that changes the Player.
- **(iv) The host in `transit-playback`.** Playback would have to depend on the engine for commands, take placement and the session, coupling the device crate to the document crate.
- **(v) Keep the strings as named constants.** Removes typos, not B3 to B5: the text would still be the state.
- **(vi) A phase per record pass kind** (punch, cycle, count-in, pre-roll as variants). Punch and pre-roll are plan data, not states the transport waits in; the `from: Option` and `count_in` fields carry them.

## Choices made in this record

The skeleton left these to the author; each is open to the judge.

1. The phase's variants and data (section 3), including `AfterStop::Reset` so a session replacement shows no "Stop pending", and `seen` on `CountIn` for B3.
2. WaitingForClock shows "Record pending", not a new text.
3. The display rule (`status_at_phase`) and that outcomes replace only a state text (section 6).
4. The predicate set and its reader assignment (section 7), and `TransportStatus.pass`.
5. The Record button's lit set.
6. `next()` as a pure function with effects; the matrix test is pure.
7. S1 keeps wrappers so `player_tests.rs` passes unchanged.
8. `Host::step` borrows the engine; the cursor arrives in the step input.
9. The second Stop's Return to Zero runs `engine.execute` directly (section 5 notes).
10. The interlocks stay in the interface's `run`.
11. S3 and S5 are each split in two; S2 lands as ordered commits.
12. The measurement's control reply is parked, not polled through a new method.
13. B4 restated as a display defect and B10 kept as a structural fix (section 10).
14. D1 to D3 proposed as related defects.

## Judging

## Related

[ADR 0043](0043-hardware-sync-transport-and-clock.md) (hardware sync), [ADR 0052](0052-pre-count-zone.md) (pre-count zone, count-in and latch), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md) (arm state is runtime), [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md) (unsafe policy), [ADR 0018](0018-plugin-host-thread.md) (plug-in host thread), [ADR 0026](0026-cycle-recording-stacks-takes.md) (cycle recording). Issues #739, #733, #736, #39.

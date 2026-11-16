# ADR 0062 tables: matrix, simulation, readers, display

This file is part of [ADR 0062](../0062-session-host.md) and shares its status. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

## A. The transition matrix

Columns: **S** Stopped, **SP** StartPending, **P** Playing, **RP** RecordPending, **CI** CountIn, **W** WaitingForClock, **R** Recording, **ST** StopPending Park or Reset, **STr** StopPending Resume, **RND** Rendering.

Legend: a phase name is the phase entered (for a command, once its Player request is accepted; record section 5); `—` no effect, no message; `·` cannot occur for a tracked request (a stale receipt clears nothing); `!` the pass ends here (`finish_recording`: a take with content is placed, an empty one discarded, the recorder disarmed); `hold` held until the stop completes, then applied (D4); `def` deferred until the resumed pass starts or fails; `msg` message only; `½` at half speed; `STz` StopPending Reset. Refusals: Tr reopen (`host/reconnect.rs:120-135`), Ta no armed track plus the Record hint (`host/recording.rs:20-28`), Te "Cannot record: ..." (`:36`), Ts `record_speed_refusal` with the speed kept (`host/recording.rs:721-724`, pinned by `host/live_cycle_tests.rs:366`), Th `HALF_SPEED_WHILE_RECORDING` (`:738`), Td `REFUSED_RECORDING` (`audio_devices.rs:46`), Tn the render's `REFUSED_TRANSPORT` (`finish/realtime.rs`), Tp `refusal_text` (`host.rs:206`), Tf `start_failure_text` (`host.rs:201`). Global: while `faulted`, every start (C1 to C3, C9, and C7 outside a pass) refuses with the fault's text. A Bn or Dn tag marks a cell that differs from today.

**Commands** (`E/lib.rs:87-101`; queued commands in H5, MIDI buttons in H6)

| Event | S | SP | P | RP | CI | W | R | ST | STr | RND |
|---|---|---|---|---|---|---|---|---|---|---|
| C1 Play | SP (Tr) | — | — | — | ST! | — D1 | — | hold D4 | def | Tn |
| C2 PlayExact(at) | SP | SP, prev kept | SP, prev Playing | — D2 | ST! D2 | — D2 | — D2 | hold D4 | def | Tn |
| C3 PlaySelection | SP | SP | SP | — D2 | ST! | — D2 | — D2 | hold D4 | def | Tn |
| C4 TogglePlay | SP (Tr) | ST | ST | ST! | ST! | ST! D1 | ST! | ST, Stop re-sent | def | Tn |
| C5 Stop | S, Return to Zero | ST | ST | ST! | ST! | ST! | ST! | ST, Stop re-sent, held starts dropped | ST Park, resume dropped | Tn |
| C6 Pause | — D6 | ST | ST | ST! | ST! | ST! | ST! | ST, Stop re-sent, held starts dropped | ST Park, resume dropped | Tn |
| C7 Record | RP (Tr, Ta, Te); B7: Ts cannot occur here | normal: RP; half: Ts | normal: RP punch; half: Ts | ST! | ST! | ST! | ST! | hold D4 | ST Park, resume dropped | Tn |
| C8 Locate(at) | S; cursor, position, retained := at | SP; cursor, retained := at; start not moved | P; Player Locate; cursor, retained := at | STr(at)! | STr(at)! | STr(at)! | STr(at)! | ST; cursor, retained := at; no Player Locate D5 | STr, target := at | Tn |
| C9 HalfSpeed | SP½ (Tr) B7 | SP½ | SP½, from the cursor | Th | ST! | Th | Th | hold D4 | Th | Tn |
| C10 Scrub(at) | position := at until the next readback; phase unchanged (every column, RND included: not in the render's refused list) | | | | | | | | | |
| C11 AllNotesOff | release sent; phase unchanged | same | same | same | same | same | same | same | same | render cancelled first, then release; RND finishing |
| C12 ResetSession | STz | STz | STz | STz! | STz! | STz! | STz! | STz | STz D3 | render cancelled, then STz D8 |

**Receipts** (H1)

| Event | S | SP | P | RP | CI | W | R | ST | STr | RND |
|---|---|---|---|---|---|---|---|---|---|---|
| R1 Play applied | · | P, "Playing" | · | · | · | · | · | · | · | · |
| R2 Play refused | · | prev, Tp; speed Normal when prev is S (B7) | · | · | · | · | · | · | · | · |
| R3 Record admitted (applied, no latch) | · | · | · | CI with a count-in, else W (B1) | — | — | · | · | · | · |
| R4 Record started (latch) | · | · | · | R, "Recording" | R, "Recording" | R, "Recording" | · | · | · | · |
| R5 Record refused | · | · | · | from stopped S!, Tp; punch P!, Tp | · | · | · | · | · | · |
| R6 Stop frontier reached, not playing | · | · | · | · | · | · | · | Park: position read, retained := it, position := working cursor, S, "Stopped", held commands released (H1 row below); Reset: S | `stop` := none (H2 below) | · |
| R7 record-off applied | fact cleared | same | same | same | same | same | same | `record_off` := none | `record_off` := none (H2) | same |
| R8 record-off refused | as R7, Tp | | | | | | | | | |
| R9 Locate applied / refused | locate fact cleared; refused: Tp; phase unchanged (every column) | | | | | | | | | |
| R10 release frontier | release fact cleared; outcome "All MIDI notes off" by section 6; phase unchanged (every column) | | | | | | | | | |
| R11 Session rejected | message "Audio session N rejected" (`host.rs:316-318`); phase unchanged (every column) | | | | | | | | | |
| R12 untracked action refused | Tp (`host.rs:362-364`, `player_tests.rs:99-116`); phase unchanged (every column) | | | | | | | | | |
| R13 Player faulted | `faulted`; msg | S, msg | S, msg | S!, msg | S!, msg | S!, msg | S!, msg | S, msg | S, msg | RND finishing, msg |

**Observations** (O1 to O5 in H4, O6 in H7)

| Event | S | SP | P | RP | CI | W | R | ST | STr | RND |
|---|---|---|---|---|---|---|---|---|---|---|
| O1 stage: counting in | — | — | — | CI | — | — | — | — | — | — |
| O2 stage: waiting for the clock | — | — | — | W | W (B3) | — | — | — | — | — |
| O3 stage started, or latch read | — | — | — | R, "Recording" | R, "Recording" | R, "Recording" (B1) | — | — | — | — |
| O4 stage failed, or start failure read | — | — | — | from stopped S!, Tf; punch P!, Tf | S!, Tf | from stopped S!, Tf; punch P!, Tf | — | — | — | — |
| O5 stage cancelled | — | — | — | — | — | — | — | — | — | — |
| O6 Player stopped by itself (no Stop outstanding) | — | — | S, speed Normal | — | — | punch S! | S! | — | — | finishing: S; else — |

**Render, device, loop, timers, held commands**

| Event | S | SP | P | RP | CI | W | R | ST | STr | RND |
|---|---|---|---|---|---|---|---|---|---|---|
| E1 render start (host) | RND | refused: "Stop the transport to render in real time." (`E/cmd/realtime_render.rs:115`) | same | same | same | same | same | same | same | refused |
| E2 render result taken or cancelled | · | · | · | · | · | · | · | · | · | RND finishing |
| V1 output trouble | msg | msg (today) | ST, msg | ST!, msg | ST!, msg | ST!, msg | ST!, msg | msg | ST Park, msg | RND finishing (render lost), msg |
| V2 input trouble, or input loss reported (`host/reconnect.rs:57-64`) | msg | msg | msg | ST!, msg | ST!, msg | ST!, msg | ST!, msg | msg | ST Park, msg | msg |
| V3 driver reset or stall (reopen cause) | S, msg B5 | S, msg B5 | S, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S, msg B5 | S, msg B5 | render cancelled, S, msg |
| V4 Player replaced (Apply, Reconnect, `ui.set {audio}`, a needed reopen); clears `faulted` | S | S B5 | S B5 | Td | Td | Td | Td | S B2, B5 | Td | render cancelled (`finish/realtime.rs:26`), S |
| V5 MIDI transport lost (`midi_transport.rs:162-173`): offers cancelled, held and deferred cleared, `recovering`, message | S D6 | ST | ST | ST! | ST! | ST! | ST! | ST, Stop re-sent | ST Park | msg |
| L1 wanted loop changed | `loop_seen` := wanted | `set_loop` sent | `set_loop` sent | deferred to the pass's end | same | same | same | `loop_seen` := wanted, nothing sent D5 | deferred | — |
| T1 lap limit | — | — | — | — | — | — | ST!, note | — | — | — |
| H1 held commands released (after R6 Park) | applied in order | | | | | | | | | |
| H2 Resume ready (`stop` and `record_off` none) | · | · | · | · | · | · | · | · | a queued ResetSession, Stop, Pause or Record supersedes it (STz or ST Park); else the full Record start at the target: RP, or S with the take preserved and the reason visible; then the deferred commands | · |

## B. The simulated transport (no Player)

Data: `Sim { pos, end, looped, laps, speed }` and `SimCountIn { left, play: (from, end, looped) }`. StartPending, RecordPending, WaitingForClock and StopPending never occur; Rendering needs a Player.

| Event | S | P | CI | R |
|---|---|---|---|---|
| Play, PlayExact, PlaySelection | P, `Sim` from the start, end and loop | P restarted (PlayExact, PlaySelection); Play — | S! (cancel) | — |
| HalfSpeed | P with `speed` Half (B7) | P restarted at half | S! | Th |
| Toggle, Stop, Pause | Toggle: P; Stop: Return to Zero; Pause: — (D6) | S: retained := pos, pos := cursor | S! | S! |
| Record | count-in: CI, `left` := plan frames; else R with `Sim` | R (punch at the position) | S! | S! |
| Locate(at) | position := at | pos := at | take-safe: S!, then R at `at` | take-safe: S!, then R at `at` |
| dt timer | — | pos += dt × rate × speed; wraps add to `laps`; end reached: S | `left` -= dt × rate; at 0: R | as P; end reached: S! |
| ResetSession | S | S | S! | S! |

## C. Readers and the capability each moves to

Each reader reads exactly one field or predicate of record section 7. B12 marks a reader whose blocking changes.

| Reader | Question | Capability |
|---|---|---|
| `actions.rs:11` width change, `:15` undo and redo | a record operation under way | `record_engaged` (B12 before admission) |
| `menus.rs:18` replace or quit; `:402` Reconnect enabled | same | `record_engaged` (B12) |
| `frame_logic.rs:148` replacing; `:256` `ui.set {audio}` | same | `record_engaged` |
| `audio_devices.rs:511, 574`; `audio_devices/window.rs:164`; `host/reconnect.rs:217` | same | `record_engaged` |
| `host/autosave.rs:113` (stop before close; keep the autosave); `:153` recovery refusal | same | `record_engaged` |
| `host/timed_read.rs:44` timed autosave skip | same | `record_engaged` |
| `host/transport.rs:124` loop retarget deferral | same | `record_engaged` |
| `host/reconnect.rs:29, 54, 157, 169` reopen wording and stops | same | `record_engaged` |
| `E/lib.rs:770` `blocked_while_recording` | same | `record_engaged` (B12) |
| `host/transport.rs:43, 53`; `host.rs:54`; `host.rs:100` | a take still to finalize | `finalizing` |
| `arrange.rs:1048` take preview | capture under way | `recording` |
| `ui.inspect` `recording` (`lib.rs:665`) | same | `recording` |
| Record button (`transport.rs:176, 197`) | lit in WaitingForClock and Recording; pulses from the published counter | phase (display) |
| `lib.rs:636-638` `is_playing`, `:641-648` `position`, `:537` | playback observed | `playing` |
| `host.rs:47` `meters_live`; `host/reconnect.rs:29` | same | `playing` |
| `arrange.rs:1079, 1170`; `keys/view.rs:211`; `sampler_panel/slices/placement.rs:64` | playhead drawing, following, placement | `playing` |
| `browser/preview.rs:272` | stop the preview on a playback edge | `playing` |
| `metronome.rs:67`; `transport.rs:120, 175`; `lib.rs:664` | beat display, loading and Play display, inspected playing | `playing` |
| `midi_monitor.rs:674` (reads the Player directly today) | timestamp display | `playing` and `position` from the step (B9) |
| `audio_devices.rs:528` `was_playing` before a reopen; `host/reconnect.rs:227` stop the old Player | same | `playing` |
| `automation/pass.rs:9` write pass | whether to write and keep the merge | `playing` (ordering test, D7) |
| `host/recording.rs:54` punch or stopped start; `:334` clock observation; `:359` live clock | same | `playing` (race test, B9) |
| `host/timed_read.rs:53` defer state capture while playing | same | `playing` |
| `E/cmd/event.rs:367`, `E/cmd/options.rs:66`, `E/cmd/transport.rs:102` | which position a command uses | `playing` with `position`, published together |
| `audio_devices.rs:481`; `hardware_ui.rs:105`; `hardware_measure.rs:187` | may a measurement start | `exclusive_ok` (B12) |
| `external_measure.rs:227`; `finish/realtime.rs:139` | may a measurement or render start | `exclusive_ok` (B12) |
| `E/lib.rs:414`; `E/cmd/external_instrument.rs:45`; `E/cmd/realtime_render.rs:115` | same, engine side | `exclusive_ok` (B12) |
| `midi_transport.rs:204` end of MIDI recovery | stop acknowledged and nothing left | `exclusive_ok` (B12: also waits for a locate or release) |
| `frame_logic.rs:385` repaint while anything is pending, release included | work outstanding | `work_outstanding` |
| `frame_logic.rs:530` repaint while the transport is active | not stopped | `transport_active()` |
| `host/reconnect.rs:138` `stop_for_reopen` | same | `transport_active()` |
| `frame_logic.rs:365` session sync gate | no Play, Locate, Record start or record-off outstanding | `sync_allowed()` |
| `midi_transport.rs:216-221, 280, 284` running and recording in a batch | logical state during the batch | `running()` and `record_engaged`, read from the working phase |
| `host/transport.rs:85, 128, 140, 158, 166, 218, 228-232, 246, 253, 261, 285, 312-313` | locate admission, loop submission, Resume readiness, start-failure cleanup, legacy text, start and toggle decisions, second Stop, Record toggle, take-safe locate, half-speed refusal, natural end | the matrix (rows C1 to C9, L1, H2, O4, O6); the legacy text is deleted |
| `host.rs:96` stop completion | the stop frontier | row R6 |
| backend-local: `crates/mix/src/strip.rs:418`; `P/player/audio.rs:775, 871`; `P/player/device.rs:265, 966`; `P/player/device/calibrate.rs:25`, `external_measure.rs:31`, `realtime_render.rs:61` | audio-side admission and observation | unchanged, not redirected |
| `transit-automation` | none today (it deletes requests, `crates/automation/src/lib.rs:53-54`) | S4 reads `TransportStatus` |

## D. Display examples (S2c tests)

Notation: what the status line shows after each step. N is a Note message, O an Outcome message.

1. `play_pending_then_failure_then_applied_keeps_the_failure`: StartPending: "Play pending". Plug-in failure F (Note): F. Play applied (line before showed F, a Note): Playing, no outcome written: F. F ages out: empty.
2. `a_failure_survives_admission_into_the_count_in`: RecordPending with a count-in: "Count-in". F: F. Admission to CountIn (line before showed a Note): the state keeps its old revision: F. F ages out: "Count-in". The stage moves to started: Recording; line before showed "Count-in": "Recording" written (O).
3. `a_message_during_a_long_count_in_ages_back_to_count_in`: CountIn: "Count-in". Message M (N): M. Eight seconds of frame clock: "Count-in".
4. `three_transitions_in_one_step_arbitrate_in_order`: StartPending "Play pending"; one step applies Play (line before: state text; "Playing" written, O), then a queued Stop (line before: an Outcome; "Stop pending" with a fresh revision): "Stop pending". Next step, the frontier (line before: state text): "Stopped" (O).
5. `a_refusal_during_stop_pending_stays_and_ages_to_empty`: StopPending "Stop pending". A refusal F (N): F. Frontier (line before showed a Note): Stopped, no outcome: F. F ages out: empty.
6. `notes_off_pending_shows_over_an_old_outcome`: Playing, "Playing" (O) shown. All Notes Off: release pending (line before: an Outcome): "All MIDI notes off pending". Release frontier (line before: state text): "All MIDI notes off" (O).

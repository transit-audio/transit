# ADR 0062 tables: matrix, simulation, readers, display, status writers, facade

This file is part of [ADR 0062](../0062-session-host.md) and shares its status. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

## A. The transition matrix (a Player is open)

Columns: **S** Stopped, **SP** StartPending, **P** Playing, **RP** RecordPending, **CI** CountIn, **W** WaitingForClock, **R** Recording, **STp** StopPending Park, **STr** StopPending Resume, **STz** StopPending Reset, **RND** Rendering.

Legend: a phase name is the phase entered. Command rows apply at admission (record section 9): the phase moves and the Player request is submitted before the next command runs. `—` no effect, no message; `·` cannot occur for a tracked request (a stale receipt clears nothing); `!` the pass ends here (`finish_recording`: a take with content is placed, an empty one discarded, the recorder disarmed); `hold` held until the stop completes, then admitted (D4); `def` deferred until the resumed Record is admitted or refused; `msg` message only; `½` at half speed. Refusals (the phase is kept, the call returns `Err`, the text goes to the status line): Tr reopen (`host/reconnect.rs:120-135`), Ta no armed track plus the Record hint (`host/recording.rs:20-28`), Te "Cannot record: ..." (`:36`), Ts `record_speed_refusal` with the speed kept (`host/recording.rs:721-724`; `host/live_cycle_tests.rs:366`), Th `HALF_SPEED_WHILE_RECORDING` (`:738`), Td `REFUSED_RECORDING` (`audio_devices.rs:46`), Tn the render's `REFUSED_TRANSPORT` (`finish/realtime.rs:16`), Tp `refusal_text` (`host.rs:206`), Tf `start_failure_text` (`host.rs:201`), Tx "Stop the transport to render in real time." (`E/cmd/realtime_render.rs:115`). Global rules: while `faulted`, every start (C1 to C3, C9, and C7 outside a pass) refuses with the fault's text; the ingress cutoff (record section 15) is applied before any command row and before any held or deferred command is admitted. A Bn or Dn tag marks a cell that differs from today.

**Commands**

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| C1 Play | SP (Tr) | — | — | — | STp! | — D1 | — | hold D4 | def | — | Tn |
| C2 PlayExact(at) | SP; retained := at | SP, added to the set | SP, added to the set | — D2 | STp! D2 | — D2 | — D2 | hold D4 | def | — | Tn |
| C3 PlaySelection | SP | SP, added | SP, added | — D2 | STp! | — D2 | — D2 | hold D4 | def | — | Tn |
| C4 TogglePlay | SP (Tr) | STp, set dropped | STp | STp! | STp! | STp! D1 | STp! | hold, a start D4 | def | — | Tn |
| C5 Stop | S, Return to Zero | STp, set dropped | STp | STp! | STp! | STp! | STp! | `rtz` := true D9; held starts dropped | STp, resume dropped | Stop re-sent | Tn |
| C6 Pause | — D6 | STp, set dropped | STp | STp! | STp! | STp! | STp! | held starts dropped | STp, resume dropped | — | Tn |
| C7 Record | RP (Tr, Ta, Te) | normal: RP; half: Ts | normal: RP punch; half: Ts | STp! | STp! | STp! | STp! | hold D4 | STp, resume dropped | — | Tn |
| C8 Locate(at) | S; cursor, position, retained := at | SP; cursor, retained := at; the starts not moved | P; Player Locate; cursor, retained := at | STr(at)! | STr(at)! | STr(at)! | STr(at)! | `target` := at; cursor := at; nothing sent D5 | target := at | — | Tn |
| C9 HalfSpeed | SP½ (Tr) B7 | SP½, added | SP½, added | Th | STp! | Th | Th | hold D4 | Th | — | Tn |
| C10 Scrub(at) | position := at until the next readback, phase unchanged, every column | | | | | | | | | | |
| C11 AllNotesOff | release sent, phase unchanged | same | same | same | same | same | same | same | same | same | render cancelled first (job Cancelling), then release |
| C12 ResetSession | STz | STz, set dropped | STz | STz! | STz! | STz! | STz! | STz, held dropped | STz, deferred dropped D3 | STz | job Cancelling (session replaced) D8; S once settled (E5) |

**Receipts** (drained at the start of every admission and in H1)

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| R1 Play applied (a tracked start) | · | removed, `applied` := true; set empty: P, "Playing" | · | · | · | · | · | · | · | · | render Play: `play` := none |
| R2 Play refused (a tracked start) | · | removed, Tp; set empty: P if `applied` or playback observed, else S with speed Normal (B7) | · | · | · | · | · | · | · | · | render Play: job cancelled (no Stop), `play` := none |
| R3 Record admitted (applied, no latch) | · | · | · | CI with a count-in, else W (B1); deferred released | — | — | · | · | · | · | · |
| R4 Record started (latch in the receipt) | · | · | · | R, "Recording"; deferred released | R, "Recording" | R, "Recording" | · | · | · | · | · |
| R5 Record refused | · | · | · | from stopped S!, punch P!; Tp; deferred released | · | · | · | · | · | · | · |
| R6 Stop frontier reached, not playing | · | · | · | · | · | · | · | retained := `target` or the applied position; position := working cursor; `rtz`: Return to Zero; S, "Stopped"; held admitted in order | `stop` := none (H2) | S, no outcome | `stop` := none (E5) |
| R7 record-off applied | fact cleared, every column; in STr it may complete (H2) | | | | | | | | | | |
| R8 record-off refused | as R7, and Tp | | | | | | | | | | |
| R9 Locate applied or refused | fact cleared; refused: Tp; every column | | | | | | | | | | |
| R10 release frontier | fact cleared; "All MIDI notes off" by record section 6 (not while a state text is live); every column | | | | | | | | | | |
| R11 Session rejected | message (`host.rs:316-318`), every column | | | | | | | | | | |
| R12 untracked refusal | Tp (`host.rs:362-364`, `player_tests.rs:99-116`), every column | | | | | | | | | | |
| R13 Player faulted | `faulted`; held and deferred dropped; msg | S, msg | S, msg | S!, msg | S!, msg | S!, msg | S!, msg | S, msg | S, msg | S, msg | job cancelled, writer collected, S, msg |

**Observations** (the record start's stage in H4; O6 and O7 in H5)

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| O1 stage CountingIn | — | — | — | CI; deferred released | — | — | — | — | — | — | — |
| O2 stage Waiting | — | — | — | W; deferred released | W (B3) | — | — | — | — | — | — |
| O3 stage Started(point) | — | — | — | R, "Recording"; deferred released | R, "Recording" | R, "Recording" (B1) | — | — | — | — | — |
| O4 stage Failed(reason) | — | — | — | from stopped S!, punch P!; Tf once per ticket; deferred released | S!, Tf | from stopped S!, punch P!; Tf | — | — | — | — | — |
| O5 stage Cancelled, or a ticket-cancelling call failed | — | — | — | finalize; P if playback observed, else S; deferred released | same | same | — | — | — | — | — |
| O6 Player stopped by itself, no Stop outstanding | — | — (the set decides) | S, speed Normal | — | — | punch S! | S! | — | — | — | — |
| O7 playback observed, nothing outstanding | P (adopted, logged) | — | — | — | — | — | — | — | — | — | — |

**Render, device, loop, timers, Resume**

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| E1 render start (host) | with `exclusive_ok`: RND (`play`, job Live); without: Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | refused |
| E2 render Cancel (dialog) | · | · | · | · | · | · | · | · | · | · | job Cancelling (user) |
| E3 render result collected | · | · | · | · | · | · | · | · | · | · | job Retired; `stop` := its Stop |
| E4 render cancel completed | · | · | · | · | · | · | · | · | · | · | job Retired; `stop` := its Stop |
| E5 render settled (`play` none, `stop` none, Retired, not playing) | · | · | · | · | · | · | · | · | · | · | S; the result's or the cancel reason's message |
| V1 output trouble | msg | msg (today) | STp, msg | STp!, msg | STp!, msg | STp!, msg | STp!, msg | msg | STp, resume dropped, msg | msg | job Cancelling (device lost), msg |
| V2 input trouble (device or disk, `host/reconnect.rs:66-102`) | msg | msg | msg | STp!, msg | STp!, msg | STp!, msg | STp!, msg | msg | STp, msg | msg | msg |
| V3 driver reset or stall | msg | S, msg B5 | S, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S, held dropped, msg B5 | S, msg B5 | S, msg B5 | job cancelled (reopen), writer collected, S, msg |
| V4 Player replaced (clears `faulted`) | S | S B5 | S B5 | Td | Td | Td | Td | S, held dropped B2, B5 | Td | S | render cancelled (`finish/realtime.rs:26`), S |
| V5 MIDI transport lost: offers cancelled, held and deferred cleared, `recovering`, msg | STp (today sends Stop) | STp, set dropped | STp | STp! | STp! | STp! | STp! | Stop re-sent | STp | Stop re-sent | msg |
| V6 input loss reported (`host/reconnect.rs:57-64`) | msg, the pass keeps recording (S-4); every column | | | | | | | | | | |
| L1 wanted loop changed | `loop_seen` := wanted | `set_loop` sent | `set_loop` sent | deferred to the pass's end | same | same | same | `loop_seen` := wanted, nothing sent D5 | deferred | — | — |
| T1 lap limit | — | — | — | — | — | — | STp!, Note (section E) | — | — | — | — |
| H2 Resume ready (`stop` and record-off none) | · | · | · | · | · | · | · | · | full Record start at the target: RP; a local refusal: S, take preserved, reason shown, deferred released | · | · |

## B. The simulated transport (no Player)

Recording is not reachable without a Player outside a fixture (record Context), so the simulation has two phases. Data: `Sim { pos, end, looped, laps, speed }`.

| Event | S | P (simulated) |
|---|---|---|
| Play | P from the cursor (pre-roll as today) | — |
| PlayExact(at), PlaySelection | P from the start, end and loop given | P restarted |
| TogglePlay | P | S: retained := pos; pos := cursor |
| Stop | Return to Zero | S as Toggle |
| Pause | — D6 | S as Toggle |
| Record | refused: Ta when nothing is armed, else "Cannot record: no audio input device" or "Cannot record: no audio device is open to record MIDI" (`host/recording.rs:212, 265`) | same |
| Locate(at) | position, cursor, retained := at | pos, cursor, retained := at |
| HalfSpeed | P with `speed` Half (B7) | P restarted at half |
| Scrub(at) | position := at | position := at until the next timer |
| AllNotesOff | "All MIDI notes off" at once (`host/transport.rs:303-305`) | same |
| ResetSession | S | S, `Sim` cleared |
| loop changed | `loop_seen` := wanted | `looped` := wanted (`host/transport.rs:133`) |
| dt timer | — | pos += dt × rate × speed (B7); first the loop wrap (every wrap, overshoot carried, added to `laps`), then the end test: at or past `end`, S (`host/transport.rs:359-378`) |
| lap limit, receipts, stage, device events, faults | cannot occur | cannot occur |
| render start | refused: "No audio device" (`finish/realtime.rs:239`) | same |

## C. Readers, and clocks

Each reader reads exactly one field or predicate of record section 7. B12 marks a reader whose blocking changes.

| Reader | Question | Capability |
|---|---|---|
| `actions.rs:11` width change, `:15` undo and redo | a record operation under way | `record_engaged` (B12: blocks from RecordPending) |
| `menus.rs:18` replace or quit; `:402` Reconnect enabled | same | `record_engaged` (B12) |
| `E/lib.rs:770` `blocked_while_recording` | same | `record_engaged` (B12) |
| `frame_logic.rs:148` replacing; `:256` `ui.set {audio}` | same | `record_engaged` |
| `audio_devices.rs:511, 574`; `audio_devices/window.rs:164`; `host/reconnect.rs:217` | same | `record_engaged` |
| `host/autosave.rs:113` stop before close and keep the autosave; `:153` recovery refusal | same | `record_engaged` |
| `host/timed_read.rs:44` timed autosave skip | same | `record_engaged` (B12) |
| `host/transport.rs:124` loop retarget deferral | same | `record_engaged` |
| `host/reconnect.rs:29, 54, 157, 169` reopen wording and stops | same | `record_engaged` |
| `host/transport.rs:43, 53`; `host.rs:54`; `host.rs:100` | a take still to finalize | `finalizing` |
| `host/plugin_state.rs:69-73` capture before save: refused when an admitted action is pending **and** the document changed since the Player's requested session | the save would supersede an admitted action | `!sync_allowed()` and the changed-document test, unchanged |
| `host/timed_read.rs:84-86` timed state capture | same, without the document test | `!sync_allowed()` (B12: the pending-start set) |
| `host/state_guard.rs:216-220` plan wait before captures | same | `!sync_allowed()` |
| `frame_logic.rs:365` session sync | no pending start, Locate, Record or record-off | `sync_allowed()` |
| `arrange.rs:1048` take preview; `lib.rs:665` inspected recording | capture under way | `recording` |
| Record button (`transport.rs:176, 197`) | lit in WaitingForClock and Recording; pulses from the published counter | phase (display) |
| `lib.rs:636-648` `is_playing`, `position`; `lib.rs:537` | playback observed | `playing` |
| `host.rs:47` `meters_live`; `host/reconnect.rs:29` | same | `playing` |
| `arrange.rs:1079, 1170`; `keys/view.rs:211`; `sampler_panel/slices/placement.rs:64` | playhead drawing, following, placement | `playing` |
| `browser/preview.rs:272` | stop the preview on a playback edge | `playing` |
| `metronome.rs:67`; `transport.rs:120, 175`; `lib.rs:664` | beat display, loading and Play display, inspected playing | `playing` |
| `midi_monitor.rs:674` (a direct Player read today) | timestamp display | `playing` and `position` (B9) |
| `audio_devices.rs:528` `was_playing`; `host/reconnect.rs:227` stop the old Player | same | `playing` |
| `automation/pass.rs:9` write pass | whether to write and keep the merge | `playing` (ordering test, D7) |
| `host/recording.rs:54` punch or stopped start; `:334`; `:359` | same | `playing` (race test, B9) |
| `host/timed_read.rs:53` defer state capture while playing | same | `playing` |
| `E/cmd/event.rs:367`, `E/cmd/options.rs:66`, `E/cmd/transport.rs:102` | which position a command uses | `playing` with `position` |
| `audio_devices.rs:481`; `hardware_ui.rs:105`; `hardware_measure.rs:187`; `external_measure.rs:227`; `finish/realtime.rs:139` | may a measurement or render start | `exclusive_ok` (B12) |
| `E/lib.rs:414`; `E/cmd/external_instrument.rs:45`; `E/cmd/realtime_render.rs:115` | same, engine side | `exclusive_ok` (B12) |
| `midi_transport.rs:204` end of MIDI recovery | stop acknowledged, nothing left | `exclusive_ok` (B12: also waits for a locate or release) |
| `frame_logic.rs:385` repaint while anything is pending | work outstanding | `work_outstanding` |
| `frame_logic.rs:530` repaint while active; `host/reconnect.rs:138` | not stopped | `transport_active()` |
| `midi_transport.rs:216-221, 280, 284` running and recording in a batch | the admitted phase | `running()` and `record_engaged` |
| `host/transport.rs:85, 128, 140, 158, 166, 218, 228-232, 246, 253, 261, 285, 312-313`; `host.rs:96` | locate admission, loop submission, Resume, start failure, legacy text, starts, second Stop, Record toggle, take-safe locate, half-speed refusal, natural end, stop completion | the matrix (C1 to C9, L1, H2, O4, O6, R6); the legacy text is deleted |
| backend-local: `crates/mix/src/strip.rs:418`; `P/player/audio.rs:775, 871`; `P/player/device.rs:265, 966`; `P/player/device/calibrate.rs:25`, `external_measure.rs:31`, `realtime_render.rs:61` | audio-side admission and observation | unchanged |
| `transit-automation` | none today | S4 reads `TransportStatus` |

| Timed thing | Clock |
|---|---|
| simulation `dt`; meter poll every 50 ms (`host/transport.rs:324`) | frame |
| reopen attempts (`host/reconnect.rs:34, 86, 151`; hidden-window time does not count, as today); stream verification (`audio_devices.rs:299-320, 340`) | frame |
| Record hint lifetime (`host/recording.rs:27`, `lib.rs:653`) | frame, interface side |
| timed state read and autosave (`host/timed_read.rs:42, 47, 57`) | monotonic |
| MIDI journal flush interval (`E/midi_journal.rs:134, 140, 212`); plan wait (`host/state_guard.rs:215-241`, with `sleep`); hardware measurement deadlines when stepped (`hardware_measure.rs:21, 47`) | monotonic |
| MIDI journal file name (`E/midi_journal.rs:114`) | calendar |
| exempt: the Player's state read budget and save deadline (`P/player/handles.rs:31, 897-913`) | the Player's own |

## D. Display examples (S2d tests)

N is a Note, O an Outcome; the line after each step is what the status line shows.

1. `play_pending_then_failure_then_applied_keeps_the_failure`: StartPending "Play pending". Failure F (N): F. Play applied (before: a Note): Playing, no outcome: F. F ages out: empty.
2. `a_failure_survives_admission_into_the_count_in`: RecordPending with a count-in "Count-in". F: F. Stage CountingIn (before: a Note; the state keeps its revision): F. F ages out: "Count-in". Stage Started (before: a state; no state live after): "Recording" (O).
3. `a_message_during_a_long_count_in_ages_back_to_count_in`: CountIn "Count-in". M (N): M. Eight seconds of frame clock: "Count-in".
4. `three_transitions_arbitrate_in_order`: StartPending "Play pending"; Play applied (before: a state): "Playing" (O); Stop admitted (before: an Outcome; fresh revision): "Stop pending". Frontier (before: a state; no state live): "Stopped" (O).
5. `a_refusal_during_stop_pending_stays_and_ages_to_empty`: StopPending "Stop pending". F (N): F. Frontier (before: a Note): Stopped, no outcome: F. F ages out: empty.
6. `notes_off_pending_shows_over_an_old_outcome`: Playing, "Playing" (O). All Notes Off (before: an Outcome; fresh revision): "All MIDI notes off pending". Release frontier (before: a state; no state live): "All MIDI notes off" (O).
7. `a_note_before_the_first_state_stays_on_top`: Stopped, F (N): F. Play (before: a Note, no old state: revision 0): F. F ages out: "Play pending". Play applied (before: a state): "Playing" (O).
8. `release_completing_during_stop_pending_keeps_stop_pending`: Playing; All Notes Off: "All MIDI notes off pending"; Stop (the phase text outranks release): "Stop pending". Release frontier (a state is still live): "Stop pending", no outcome. Stop frontier (no state live): "Stopped" (O).
9. `stop_completing_with_a_release_outstanding_keeps_the_release_state`: StopPending with a release outstanding: "Stop pending". Stop frontier (a state is still live: release pending): "All MIDI notes off pending", no "Stopped". Release frontier: "All MIDI notes off" (O).

## E. Status writers that are not mechanical (S2d)

Everything else changes `app.status = x` to `app.status.set(x)` and `self.status.clone()` to `self.status.last_text()`.

| Site | Today | Treatment |
|---|---|---|
| `audio_devices.rs:566`, `host/reconnect.rs:194-195` | appends the state-from-document sentence to the message just written | a local string, set once |
| `bounce_in_place.rs:177`, `dawproject.rs:23`, `hardware_measure.rs:301` | appends a notice or warning to the command's own message | a local string, set once |
| `host/autosave.rs:175, 178` | appends the MIDI recovery result to the recovery message | a local string, set once |
| `host/recording.rs:448, 482, 538, 598, 709` | composes the placement report by reading back and appending (problem, approximate take, lap notes, notes before the count-in, take number) | `finish_recording` builds one local report and `report_lap_notes` takes `&mut String`; the report is set once as a Note |
| `host/recording.rs:568` | appends the lap-limit note to whatever the line holds, which can be "Stop pending" when no take was placed | the lap limit's own Note: the placement report (if any) plus the note, never read from the status line |
| `host/recording.rs:13` | logs the status line | logs the local refusal string |
| `host/transport.rs:59, 165` | compares state texts | deleted (state text is derived) |
| `browser/plugins.rs:39-41` | compares the line before and after a call to learn whether it wrote | compares `status.rev()` |
| `PlayerRequests::{result, fail, notice, recovery, show_error}(&mut self.status, ...)` (about 30 sites, e.g. `host.rs:85-95`, `host/transport.rs:21, 36, 90`) | writes or accumulates errors into the string | takes `&mut StatusLine`; `fail` keeps its accumulator (`host.rs:289-297`) and sets the joined text |
| `Err(self.status.clone())` and `ok_or_else(\|\| self.status.clone())` (e.g. `actions.rs:227`, `finish/realtime.rs:176`, `host/plugin_state.rs:134, 145`, `mixer/actions.rs:293, 308`, `external_instrument.rs:101`) | returns the message the same function just wrote | `last_text()`; the S2d review checks each site writes before it returns |
| `browser.rs:652` | copies the last message into the import notice | `last_text()` |
| `lib.rs:684` `ui.inspect` `status` | the string | the shown line |

## F. The facade (S3c to S3e)

| Handle | Methods (Player calls behind them) | Interface files |
|---|---|---|
| `PluginServices` | editor open, close, state, group raise; `idle` (edits as `EditorEdits` events); preset and state jobs; slot load, stuck and timing state; retry; parameter echo | `editors.rs`, `editor_group.rs`, `plugin_header.rs` (its timings at `:664-665` through `Readouts`), `host_hang.rs`, `unresponsive.rs`, `devices.rs`, `mixer/inserts.rs` (MIDI destination status through `Readouts`), `mixer/actions.rs`, `external_instrument.rs` |
| `Readouts` | meters snapshot; telemetry and device metadata, and the mutation `reset_peak_load` (`telemetry.rs:494-497`); MIDI input and output status; the muted-input capability `reenable_input` (wrapping `MutedMidiInput::reenable`, `midi_output.rs:41`); monitor activity, events, step position, and the monitor enable and configuration `set_midi_monitor` (`midi_monitor.rs:678`); scope hub (`scope_strip.rs:76`); plug-in timings; the step's playing and position pair | `telemetry.rs`, `debug_log.rs`, `midi_monitor.rs`, `midi_output.rs`, `midi_input.rs`, `mixer/channel.rs`, `arrange.rs`, `arrange/tracks.rs`, `about.rs`, `diagnostics.rs`, `render_measurement.rs`, `scope_strip.rs` |
| `LiveInput` | audition `note` (`keys.rs`, `sampler_panel/behavior.rs:207, 229`); the stamped played-MIDI drain (`keys/step.rs:133`); MIDI Learn: mapping configuration `set_midi_mappings`, transport learn `set_transport_learn`, learn arming `set_midi_learn`, the event drain `take_midi_map_events`, pickup sync `sync_midi_map` (`midi_learn.rs:743-784`); its diagnostics reads (`:674-691`) through `Readouts` | `keys.rs`, `keys/step.rs`, `sampler_panel/behavior.rs`, `midi_learn.rs` |
| `DeviceServices` | device listing, settings panel, low latency and its plan report, I/O map read and write, runtime properties, monitor controls and loudness reset, click level, calibration, measurement start, poll, cancel, the preview player | `audio_devices.rs`, `audio_devices/*.rs`, `audio_devices_system.rs`, `hardware_measure.rs`, `hardware_ui.rs`, `external_measure.rs`, `io_settings.rs`, `low_latency.rs`, `main_monitor.rs`, `ui_state.rs`, `browser/preview.rs` |
| none: inside the host, or reading host accessors | transport, recording, render, reopen, Player installation, the connection traits | `host.rs`, `host/*.rs`, `transport.rs` (to `:134`), `metronome.rs` (to `:96`), `midi_transport.rs` (to `:304`), `arrange/recording_preview.rs`, `frame_logic.rs`, `lib.rs`, `menus.rs`, `splash.rs:129-136`, `audio_connection.rs`, `finish/realtime.rs` |

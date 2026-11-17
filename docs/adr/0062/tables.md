# ADR 0062 tables: matrix, simulation, readers, display, status writers

This file is part of [ADR 0062](../0062-session-host.md) and shares its status. Paths are under `crates/ui-studio/src/` unless named; `P/` is `crates/playback/src/`, `E/` is `crates/engine/src/`.

## A. The transition matrix (a Player is open)

Columns: **S** Stopped, **SP** StartPending, **P** Playing, **RP** RecordPending, **CI** CountIn, **W** WaitingForClock, **R** Recording, **STp** StopPending Park, **STr** StopPending Resume, **STz** StopPending Reset, **RND** Rendering.

Legend: a phase name is the phase entered. Command rows apply at admission (record section 7): the phase moves and the Player request is submitted (and tracked) before the next command runs. `—` no effect, no message; `!` the pass ends here (`finish_recording`: a take with content is placed, an empty one discarded, the recorder disarmed); `hold` held until the stop completes, replacing any held start (one at most, `Ok(Held)`); `def` deferred (up to 32; the 33rd is refused, record section 4); `msg` message only; `½` at half speed; "by 4" the destination rule of record section 4. Refusals (phase kept, `Err`, text on the status line): Tr reopen (`host/reconnect.rs:120-135`), Ta no armed track plus the Record hint (`host/recording.rs:20-28`), Te "Cannot record: ..." (`:36`), Ts `record_speed_refusal`, speed kept (`host/recording.rs:721-724`), Th `HALF_SPEED_WHILE_RECORDING` (`:738`), Td `REFUSED_RECORDING` (`audio_devices.rs:46`), Tn the render's `REFUSED_TRANSPORT` (`finish/realtime.rs:16`), Tp `refusal_text` (`host.rs:206`), Tf the stage's failure text (`start_failure_text`, `host.rs:201`), Tx "Stop the transport to render in real time." (`E/cmd/realtime_render.rs:115`). Global: while `faulted` every start refuses with the fault's text; the ingress cutoff and the supersession of step 1 apply before any row; a Stop the Player could not take is owed (row W1). A Bn or Dn tag marks a cell that differs from today. Until their slices, the D1, D2 (S2e), B7 and D6 (S2g) cells select today's behavior (record, S2d's activation).

**Commands**

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| C1 Play | SP (Tr) | — | — | — | STp! | — D1 | — | hold D4 | def | — | Tn |
| C2 PlayExact(at) | SP; retained := at | joins the group | SP, new group | — D2 | STp! D2 | — D2 | — D2 | hold D4 | def | — | Tn |
| C3 PlaySelection | SP | joins the group | SP, new group | — D2 | STp! | — D2 | — D2 | hold D4 | def | — | Tn |
| C4 TogglePlay | SP (Tr) | STp, group closed | STp | STp! | STp! | STp! D1 | STp! | hold, a start D4 | def | — | Tn |
| C5 Stop | S, Return to Zero | STp, group closed | STp | STp! | STp! | STp! | STp! | `end` := ReturnToZero (D9); held start dropped | STp, resume dropped | Stop re-sent | Tn |
| C6 Pause | — D6 | STp, group closed | STp | STp! | STp! | STp! | STp! | held start dropped | STp, resume dropped | — | Tn |
| C7 Record | RP (Tr, Ta, Te) | normal: RP, the group stays tracked; half: Ts | normal: RP punch; half: Ts | STp! | STp! | STp! | STp! | hold D4 | STp, resume dropped | — | Tn |
| C8 Locate(at) | S; cursor, position, retained := at | SP; cursor, retained := at; the group not moved | P; Player Locate (tracked); cursor, retained := at | STr(at)! | STr(at)! | STr(at)! | STr(at)! | `end` := Target(at); cursor := at; nothing sent D5 | target := at | — | Tn |
| C9 HalfSpeed | SP½ (Tr) B7 | joins the group, ½ | SP½, new group | Th | STp! | Th | Th | hold D4 | Th | — | Tn |
| C10 Scrub(at), no production producer | position := at until the next readback; phase unchanged; every column | | | | | | | | | | |
| C11 AllNotesOff | release sent (tracked); phase unchanged | same | same | same | same | same | same | same | same | same | render cancelled first (E2), then release |
| C12 ResetSession | STz | STz, group closed | STz | STz! | STz! | STz! | STz! | STz, held discarded | STz, deferred discarded, restart disabled D3 | STz | render cancelled (E2, session replaced) D8; `then_reset` |
| C13 ruler or empty-space click (`actions.rs:180, 187`) | as C8 | as C8 | as C8 | as C8, take-safe (B14) | same | same | same | as C8 | as C8 | — | Tn |

**Receipts.** Drained in step 2 of every admission and in H1. Every receipt first retires its fact in every column; a receipt with no live fact has no lifecycle effect, and a refusal's text is shown once per request identity. Release effects are queued and run after the refresh (step 4). The cells give what changes.

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| R1 Play applied | — | current group: `applied` := true; group empty: P, "Playing" | — | — | — | — | — | — | — | — | render Play retired |
| R2 Play refused | Tp | Tp; current group empty: P if `applied` or playback observed, else S, speed Normal (B7) | Tp | Tp | Tp | Tp | Tp | Tp | Tp | Tp | render Play: job disposed without Stop |
| R3 Record admitted, no latch | — | — | — | CI with a count-in, else W, unless a stage moved it (B1); deferred released once | — | — | — | — | — | — | — |
| R4 Record started (latch) | — | — | — | R, "Recording"; deferred released once | R, "Recording" | R, "Recording" | — (never regresses) | — | — | — | — |
| R5 Record refused | — | — | — | unless a terminal stage retired Record: `!`, by 4, Tp; deferred released once | same | same | — | — | — | — | — |
| R6 Stop frontier, not playing | — | — | — | — | — | — | — | `end` Target(at): retained, position, cursor := at; ReturnToZero: retained := the applied position, then Return to Zero; AtCursor: retained := the applied position, position := working cursor; S, "Stopped" unless a state is live; the held start (current reset generation) admitted | Resume check (H2) | S, no outcome | render Stop retired; E5 |
| R7 record-off applied | in STr: Resume check (H2); elsewhere no change | | | | | | | | | | |
| R8 record-off refused | as R7, and Tp | | | | | | | | | | |
| R9 Locate or SetLoop applied or refused | refused: Tp; no change; every column | | | | | | | | | | |
| R10 release frontier | "All MIDI notes off" by record section 12; every column | | | | | | | | | | |
| R11 Session rejected | message (`host.rs:316-318`); every column | | | | | | | | | | |
| R12 untracked refusal | Tp, once per request identity; every column | | | | | | | | | | |
| R13 Player faulted (or an owed-Stop deadline) | every fact and owed Stop retired, groups closed, held and deferred discarded, resources finalized, `faulted`, adoption off; msg | S, msg | S, msg | S!, msg | S!, msg | S!, msg | S!, msg | S, msg | S, msg | S, msg | job cancelled (the join blocks, as today), S, msg |

**Observations** (the record start's stage in H4 and admission step 3; O6 and O7 in H5)

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| O1 stage CountingIn | — | — | — | CI; deferred released once | — | — | — | — | — | — | — |
| O2 stage Waiting | — | — | — | W; deferred released once | W (B3) | — | — | — | — | — | — |
| O3 stage Started(point) | — | — | — | R, "Recording"; deferred released once | R, "Recording" | R, "Recording" (B1) | — | — | — | — | — |
| O4 stage Failed(reason): retires Record | — | — | — | `!`, by 4, Tf once per ticket; deferred released once | same | same | — | — | — | — | — |
| O5 stage Cancelled: retires Record | — | — | — | `!`, by 4; deferred released once | same | same | — | — | — | — | — |
| O6 Player stopped by itself, no Stop tracked or owed | — | — (the group decides) | S, speed Normal | — | — | punch: S! | S! | — | — | — | — |
| O7 playback observed, nothing tracked, not faulted | P (adopted, logged) | — | — | — | — | — | — | — | — | — | — |

**Render, device, loop, timers, owed Stops, Resume**

| Event | S | SP | P | RP | CI | W | R | STp | STr | STz | RND |
|---|---|---|---|---|---|---|---|---|---|---|---|
| E1 render start | with `exclusive_ok`: RND, job Live, render Play tracked; without: Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | Tx | refused |
| E2 render cancel (dialog, All Notes Off, Reset, reopen, device trouble) | — | — | — | — | — | — | — | — | — | — | job Live: the cancel takes the job; Retired with the reason; every Stop it returns tracked, owed ones by W1. job Retired: no effect, the collected result kept |
| E3 render result collected (H3r) | — | — | — | — | — | — | — | — | — | — | job Retired with the result; every Stop it returns tracked, owed ones by W1 |
| E5 render settled (job Retired, render Play and Stops retired, none owed, not playing) | — | — | — | — | — | — | — | — | — | — | S; the result's or the reason's message; then the Reset continuation if set |
| W1 owed Stop (a Stop the Player refused without faulting) | — | — | — | — | — | — | — | retried each step; sent: tracked; 2 s unsent: R13 | same | same | same |
| V1 output trouble | msg | msg (today) | STp, msg | STp!, msg | STp!, msg | STp!, msg | STp!, msg | msg | STp, resume dropped, msg | msg | E2 (device lost), msg |
| V2 input trouble (device or disk, `host/reconnect.rs:66-102`) | msg | msg | msg | STp!, msg | STp!, msg | STp!, msg | STp!, msg | msg | STp, msg | msg | msg |
| V3 driver reset or stall: every fact retired as in R13, without setting `faulted` | msg | S, msg B5 | S, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S!, msg B5 | S, msg B5 | S, msg B5 | S, msg B5 | E2 (reopen), then S, msg |
| V4a Player replacement requested | S | S (Stop first, as today) | S (Stop first) | Td, phase and facts kept | Td | Td | Td | S B2, B5 | Td | S | render cancelled (`finish/realtime.rs:26`) |
| V4b the old Player retired | every fact retired, held and deferred discarded, resources finalized, `faulted` cleared; S with the new Player, every column | | | | | | | | | | |
| V5 MIDI transport lost: offers cancelled, held and deferred cleared, `recovering`, msg | STp (today sends Stop) | STp, group closed | STp | STp! | STp! | STp! | STp! | Stop re-sent | STp | Stop re-sent | msg |
| V6 input loss reported (`host/reconnect.rs:57-64`) | msg; the pass keeps recording; every column | | | | | | | | | | |
| L1 wanted loop changed | `loop_seen` := wanted | `set_loop` sent (tracked) | `set_loop` sent (tracked) | deferred to the pass's end | same | same | same | `loop_seen` := wanted, nothing sent D5 | deferred | — | — |
| T1 lap limit | — | — | — | — | — | — | STp!, Note (section E) | — | — | — | — |
| H2 Resume check (Stop and record-off retired, not superseded) | — | — | — | — | — | — | — | — | the full Record start at the target: RP; a local refusal: S, take preserved, reason shown, deferred released | — | — |

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
| Locate(at), ruler click | position, cursor, retained := at | pos, cursor, retained := at |
| HalfSpeed | P with `speed` Half (B7) | P restarted at half |
| Scrub(at) | position := at | position := at until the next timer |
| AllNotesOff | "All MIDI notes off" at once (`host/transport.rs:303-305`) | same |
| ResetSession | S | S, `Sim` cleared |
| loop changed | `loop_seen` := wanted | `looped` := wanted (`host/transport.rs:133`) |
| dt timer | — | pos += dt × rate × speed (B7); first the loop wrap (every wrap, overshoot carried, added to `laps`), then the end test: at or past `end`, S (`host/transport.rs:359-378`) |
| receipts, stage, device events, faults, owed Stops, lap limit | cannot occur (nothing is tracked) | same |
| render start | refused: "No audio device" (`finish/realtime.rs:239`) | same |

## C. Readers

Each reader reads exactly one capability or predicate of record section 8. B12 marks a reader whose blocking changes.

| Reader | Question | Capability |
|---|---|---|
| `actions.rs:11` width change, `:15` undo and redo | a record operation under way | `record_engaged` (B12: blocks from RecordPending) |
| `menus.rs:18` replace or quit; `:402` Reconnect enabled | same | `record_engaged` (B12) |
| `E/lib.rs:770` `blocked_while_recording` | same | `record_engaged` (B12; no host: `recording`) |
| `frame_logic.rs:148` replacing; `:256` `ui.set {audio}` | same | `record_engaged` |
| `audio_devices.rs:511, 574`; `audio_devices/window.rs:164`; `host/reconnect.rs:217` | same | `record_engaged` |
| `host/autosave.rs:113-116` request Stop before closing | same | `record_engaged` |
| `host/autosave.rs:117` keep the autosave (a take must stay protected) | a take still under way or still to finalize | `record_engaged || finalizing` |
| `host/autosave.rs:153` recovery refusal | a record operation under way | `record_engaged` |
| `host/timed_read.rs:44` timed autosave skip | same | `record_engaged` (B12) |
| `host/transport.rs:124` loop retarget deferral | same | `record_engaged` |
| `host/reconnect.rs:29, 54, 157, 169` reopen wording and stops | same | `record_engaged` |
| `host/transport.rs:43, 53`; `host.rs:54`; `host.rs:100` | a take still to finalize (idempotent: a tracked record-off alone never resubmits) | `finalizing` |
| `host/plugin_state.rs:69-73` capture before save: refused when an admitted action is pending **and** the document changed since the Player's requested session | the save would supersede an admitted action | `!sync_allowed()` (a render included, B12) and the changed-document test, unchanged |
| `host/timed_read.rs:84-86` timed state capture | same, without the document test | `!sync_allowed()` (B12) |
| `host/state_guard.rs:216-220` plan wait before captures | same | `!sync_allowed()` (B12) |
| `frame_logic.rs:365` session sync | nothing tracked that the plan must not change under | `sync_allowed()` (a render now included, B12) |
| `actions.rs:538` (with `copy_selected`, `:522-529`); `host/plugin_state.rs:16, 174` | a session replaced since this reader's data was made | the reset generation and `restore_pending` (record section 7) |
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
| `host/recording.rs:54` punch or stopped start; `:334`; `:359` | same | `playing`, refreshed at admission (B9) |
| `host/timed_read.rs:53` defer state capture while playing | same | `playing` |
| `E/cmd/event.rs:367`, `E/cmd/options.rs:66`, `E/cmd/transport.rs:102` | which position a command uses | `playing` with `position` |
| `audio_devices.rs:481`; `hardware_ui.rs:105`; `hardware_measure.rs:187`; `external_measure.rs:227`; `finish/realtime.rs:139` | may a measurement or render start | `exclusive_ok` (B12) |
| `E/lib.rs:414`; `E/cmd/external_instrument.rs:45`; `E/cmd/realtime_render.rs:115` | same, engine side | `exclusive_ok` (B12; no host: not `playing` or `recording`) |
| `midi_transport.rs:204` end of MIDI recovery | stop acknowledged, nothing left | `exclusive_ok` (B12: also waits for a locate or release) |
| `frame_logic.rs:385` repaint while anything is pending | work outstanding | `work_outstanding` |
| `frame_logic.rs:530` repaint while active; `host/reconnect.rs:138` | not stopped | `transport_active()` |
| `midi_transport.rs:216-221, 271, 280, 283, 284` running and recording within a batch, and the Record bookkeeping after a dispatch | the admitted phase | `running()` and `record_engaged` |
| `host/transport.rs:85, 128, 140, 158, 166, 218, 228-232, 246, 253, 261, 285, 312-313`; `host.rs:96` | locate admission, loop submission, Resume, start failure, legacy text, starts, second Stop, Record toggle, take-safe locate, half-speed refusal, natural end, stop completion | the matrix (C1 to C9, L1, H2, O4, O6, R6); the legacy text is deleted |
| backend-local: `crates/mix/src/strip.rs:418`; `P/player/audio.rs:775, 871`; `P/player/device.rs:265, 966`; `P/player/device/calibrate.rs:25`, `external_measure.rs:31`, `realtime_render.rs:61` | audio-side admission and observation | unchanged |

## D. Display examples (S2f tests)

N is a Note, O an Outcome; the line after each step is what the status line shows.

1. `play_pending_then_failure_then_applied_keeps_the_failure`: StartPending "Play pending". Failure F (N): F. Play applied (before: a Note): Playing, no outcome: F. F ages out: empty.
2. `a_failure_survives_admission_into_the_count_in`: RecordPending with a count-in "Count-in". F: F. Stage CountingIn (before: a Note; the state keeps its revision): F. F ages out: "Count-in". Stage Started (before: a state; none live after): "Recording" (O).
3. `a_message_during_a_long_count_in_ages_back_to_count_in`: CountIn "Count-in". M (N): M. Eight seconds of frame clock: "Count-in".
4. `three_transitions_arbitrate_in_order`: StartPending "Play pending"; Play applied: "Playing" (O); Stop admitted (before: an Outcome; fresh revision): "Stop pending". Frontier (before: a state; none live): "Stopped" (O).
5. `a_refusal_during_stop_pending_stays_and_ages_to_empty`: StopPending "Stop pending". F (N): F. Frontier (before: a Note): Stopped, no outcome: F. F ages out: empty.
6. `notes_off_pending_shows_over_an_old_outcome`: Playing, "Playing" (O). All Notes Off (before: an Outcome): "All MIDI notes off pending". Release frontier (before: a state; none live): "All MIDI notes off" (O).
7. `a_note_before_the_first_state_stays_on_top`: Stopped, F (N): F. Play (before: a Note, no old state: revision 0): F. F ages out: "Play pending". Play applied: "Playing" (O).
8. `release_completing_during_stop_pending_keeps_stop_pending`: Playing; All Notes Off: "All MIDI notes off pending"; Stop (the phase text outranks release): "Stop pending". Release frontier (a state still live): "Stop pending", no outcome. Stop frontier (none live): "Stopped" (O).
9. `stop_completing_with_a_release_outstanding_keeps_the_release_state`: StopPending with a release tracked: "Stop pending". Stop frontier (release still live): "All MIDI notes off pending", no "Stopped". Release frontier: "All MIDI notes off" (O).
10. `a_reset_clears_an_old_outcome`: Playing, "Playing" (O). Stop admitted: "Stop pending". `session.new` (StopPending Reset, no state text, no outcome of its own: the Outcome is cleared): empty. Frontier: Stopped, empty.
11. `a_note_outlasts_a_release_round_trip`: Stopped, F (N): F. All Notes Off (before: a Note; state revision 0): F. Release frontier (before: a Note): no outcome: F. F ages out: empty.
12. `a_note_survives_a_reset_during_the_count_in`: CountIn "Count-in". F (N): F. `session.new` (StopPending Reset, no state text): F. F ages out: empty, never "Count-in".

## E. Status writers that are not mechanical (S2f)

Everything else changes `app.status = x` to `app.status.set(x)`, `self.status.clone_from(x)` (14 sites) to `set`, and `self.status.clone()` to `last_text()`.

| Site | Today | Treatment |
|---|---|---|
| `audio_devices.rs:566`, `host/reconnect.rs:194-195` | appends the state-from-document sentence to the message just written | a local string, set once |
| `bounce_in_place.rs:177`, `dawproject.rs:23`, `hardware_measure.rs:301` | appends a notice or warning to the command's own message | a local string, set once |
| `host/autosave.rs:175, 178` | appends the MIDI recovery result to the recovery message | a local string, set once |
| `host/recording.rs:448, 482, 538, 598, 709` | composes the placement report by reading back and appending | `finish_recording` builds one local report; `report_lap_notes` takes `&mut String`; set once as a Note |
| `host/recording.rs:568` | appends the lap-limit note to whatever the line holds, which can be "Stop pending" | the lap limit's own Note: the placement report (if any) plus the note, never read from the status line |
| `host/recording.rs:13` | logs the status line | logs the local refusal string |
| `host/transport.rs:59, 165` | compares state texts | deleted |
| `browser/plugins.rs:39-41` | compares the line before and after a call | compares `status.message_rev()` (bumped only by message writes, so a state change cannot suppress its success message) |
| `import.rs:292-294` | rewrites a progress line and calls `status_age.restart()` (#216) | `status.say_again(text)` |
| `PlayerRequests::{result, fail, notice, recovery, show_error}(&mut self.status, ...)` (about 30 sites) | writes or accumulates errors into the string | takes `&mut StatusLine`; `fail` keeps its accumulator (`host.rs:289-297`) |
| `Err(self.status.clone())`, `ok_or_else(\|\| self.status.clone())` (e.g. `actions.rs:227`, `finish/realtime.rs:176`, `host/plugin_state.rs:134, 145`, `mixer/actions.rs:293, 308`, `external_instrument.rs:101`) | returns the message the same function just wrote | `last_text()`; the review checks each site writes before it returns |
| `browser.rs:652` | copies the last message into the import notice | `last_text()` |
| `lib.rs:684` `ui.inspect` `status` | the string | the shown line |
| diagnostics report (`diagnostics.rs:347, 508-510`, through `StatusAge::last`) | the last shown message after expiry | `StatusLine::last_shown()`, updated from the presented non-empty line with the supplied epoch time |

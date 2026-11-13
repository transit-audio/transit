<!-- transit-index
title: With Cycle on, Record always loops the cycle range and stacks a take per lap until Stop; the Loop Record option (`edit.loop_record`, `options.loop_record`, Alt+L) is removed and old files that carry it still load
-->
# ADR 0026: With Cycle on, Record always loops and stacks takes; Loop Record is removed

- **Status:** accepted 2026-10-24 (decided by Derek, #18)
- **Date:** 2026-10-24
- **Reopen when:** users ask to record a single pass through the cycle range and stop at its end (a one-shot punch over a loop); or a take-count limit is needed to protect disk or memory on very long loops.

## Context

Recording used to depend on two switches that had to agree. Cycle looped playback, and a separate Loop Record option (Options menu, Alt+L, `options.loop_record`, `EditState::loop_record`) made Record stack one take per lap. With Cycle on and Loop Record off, Record punched out at the loop end: the pass stopped at the first wrap, and a player overdubbing a guitar had to find out why nothing was recording on the second lap (#18). Cubase and Live both loop and stack takes whenever the cycle is on.

## Decision

1. **Cycle on means Record loops.** With Cycle on, pressing Record starts the pass at the cycle start (after the count-in, if any), keeps recording lap after lap, and ends only at Stop. Each lap is its own take on its own playlist, for audio and MIDI alike. This holds for every start: from a stop, while already playing (punch in on the fly, at the loop start or inside the loop), with a count-in, and with pre-roll.
2. **Cycle off means one open-ended pass.** Record runs from the cursor (less pre-roll) until Stop, as before.
3. **The Loop Record option is removed:** the Options menu item, its Alt+L shortcut, the `options.loop_record` command, the `loop_record` field of `EditState`, and the Loop Rec toggle in the legacy transport panel.
4. **Migration.** A session file that still carries `loop_record: true` or `false` loads unchanged: the value is ignored (`skip_deserializing`). Test: `a_session_saved_with_the_removed_loop_record_option_still_loads`.
5. **Tests.** `record_with_cycle_on_loops_and_stacks_a_take_per_lap_until_stop` (three laps are three takes of one lap each, and Stop ends the pass), `record_with_cycle_off_is_one_open_ended_pass_until_stop`, and `three_laps_of_cycle_recording_are_three_audio_takes_each_one_lap_long`.

### Amendment: Pickups (#327, 2026-11-04)

Notes played during the record count-in join the take (Logic Pro's behavior; Ableton Live does not record during its count-in and offers Capture MIDI instead; the maintainer chose Logic's). (1) MIDI capture for a pass begins at the count-in start on the same device-frame clock the pass latches on, so a note played with N samples of count-in left lands at the record start less N; the pre-roll is already inside the pass (its start is the record start), and nothing lands before session sample 0 (clamped). (2) When the pass ends, each armed MIDI track's clip starts at the earliest note-on before the record start, else at the record start as before; every note keeps its absolute position, and audio takes are unchanged. (3) With Cycle on, pickups belong to the first take, whose clip starts early; later laps are unchanged, and a first take that now overlaps earlier material follows the existing overlap and take rules (ADR 0039). This paragraph narrows decision 1 for a behavior the maintainer decided; it is docs-only and needs no judging round.

### Amendment: The lap limit (#522, #567, 2026-11-13)

(1) **Every lap is kept, up to 10,000.** One recording turns at most 10,000 laps into takes (`take_laps::MAX_LAPS`): over five and a half hours of a one-bar loop at 120 BPM, over 80 minutes of a one-beat loop. The limit exists because placing the takes at Stop costs time that grows with the square of the lap count, and a tiny cycle region could otherwise make millions of laps. (2) **Past it, the newest laps are kept,** because the newest take is the one that plays. (3) **What the user is told:** the status line says "Cycle recording keeps the newest 10000 laps as takes: N earlier laps were left out", then where they remain. (4) **Where the left-out laps remain:** audio stays in the take file, which always holds every lap, and the note names that file. MIDI is written once to a MIDI file beside the pass's journal, where the audio takes go (`<session> left-out laps NN.mid`): one track per armed track, the notes as played from the record start with the loop unrolled, on the session's tempo map. The note names that file. It is the user's: unlike the journal, no cleanup removes it. If it cannot be written, the note says so and why. (5) **The earlier cap was a defect.** Until this amendment the audio and MIDI take builders kept only the first 1,000 laps, silently: the newest laps were dropped, and their MIDI was lost when the journal was deleted (#522). (6) **The same cut for audio and MIDI.** Audio and MIDI takes are cut into laps by one function. This includes a pass started inside the loop while the transport was already cycling: the first take runs from where recording started to the loop end, then come whole laps. Audio used to become one clip running past the loop end there (#567). The loop a take recorded under is kept in the recording plan, so crash recovery cuts it the same way. This narrows the Consequences line "A cap is not added now".

## Consequences

- One switch decides the behavior, so a recording never stops for a reason the user cannot see.
- A long Cycle recording can stack many takes; stopping is the user's control. A cap is not added now (see Reopen when). Narrowed by the lap limit amendment: at most 10,000 laps become takes, and nothing past it is dropped silently (#522).
- A user who wants a single pass over a region turns Cycle off and punches in by selection.
- `EditState::loop_record` stays as a write-only field, always saved as `false` through v0.3.x so v0.2.0, which requires it, can open our sessions; it is dropped after v0.4.0.
- Stop mid-lap keeps the partial lap as a take (MIDI: passes = offset / lap length + 1). For audio, a partial lap shorter than one eighth of the lap is dropped, a rule `add_recording` already had.

## Alternatives rejected

- **Keep Loop Record as a preference defaulting to on.** It keeps the case where two switches disagree, which is what the issue was about.
- **Warn when Record is pressed with Cycle on and Loop Record off.** A warning explains the surprise rather than removing it.

## Related

- Issue #18; `docs/contributing/knowledge/recording.md`.

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
4. **Migration.** A session file that still carries `loop_record: true` or `false` loads unchanged: the field is unknown to the reader and ignored, and it is not written again on the next save. Test: `a_session_saved_with_the_removed_loop_record_option_still_loads`.
5. **Tests.** `record_with_cycle_on_loops_and_stacks_a_take_per_lap_until_stop` (three laps are three takes of one lap each, and Stop ends the pass), `record_with_cycle_off_is_one_open_ended_pass_until_stop`, and `three_laps_of_cycle_recording_are_three_audio_takes_each_one_lap_long`.

## Consequences

- One switch decides the behavior, so a recording never stops for a reason the user cannot see.
- A long Cycle recording can stack many takes; stopping is the user's control. A cap is not added now (see Reopen when).
- A user who wants a single pass over a region turns Cycle off and punches in by selection.
- A session saved by this build lacks the field, which builds before it require, so one of those builds reports an unreadable edit state on such a file. Newer builds open every older file.

## Alternatives rejected

- **Keep Loop Record as a preference defaulting to on.** It keeps the case where two switches disagree, which is what the issue was about.
- **Warn when Record is pressed with Cycle on and Loop Record off.** A warning explains the surprise rather than removing it.

## Related

- Issue #18; `docs/contributing/knowledge/recording.md`.

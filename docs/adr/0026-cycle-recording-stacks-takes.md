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

1. **Cycle on means Record loops.** From stopped, Record starts at the cycle start after any count-in. While already playing, Record punches in at the current playhead; a pass started inside the cycle first runs to the cycle end, then wraps. Recording continues until Stop. Each lap is its own take on its own playlist, for audio and MIDI alike. Take construction follows the pickup and lap-limit amendments and the partial-lap rule below. As described in ADR 0039, the original active playlist remains active, with the newest retained take playing over its range and earlier material around it. (Wording amended 2026-11-13 by the lap-limit amendment: the earlier text said every start began at the cycle start, which the punch-in path never did.)
2. **Cycle off means one open-ended pass.** Record runs from the cursor (less pre-roll) until Stop, as before.
3. **The Loop Record option is removed:** the Options menu item, its Alt+L shortcut, the `options.loop_record` command, the `loop_record` field of `EditState`, and the Loop Rec toggle in the legacy transport panel.
4. **Migration.** A session file that still carries `loop_record: true` or `false` loads unchanged: the value is ignored (`skip_deserializing`). Test: `a_session_saved_with_the_removed_loop_record_option_still_loads`.
5. **Tests.** `record_with_cycle_on_loops_and_stacks_a_take_per_lap_until_stop` (three laps are three takes of one lap each, and Stop ends the pass), `record_with_cycle_off_is_one_open_ended_pass_until_stop`, and `three_laps_of_cycle_recording_are_three_audio_takes_each_one_lap_long`.

### Amendment: Pickups (#327, 2026-11-04)

Notes played during the record count-in join the take (Logic Pro's behavior; Ableton Live does not record during its count-in and offers Capture MIDI instead; the maintainer chose Logic's). (1) MIDI capture for a pass begins at the count-in start on the same device-frame clock the pass latches on, so a note played with N samples of count-in left lands at the record start less N; the pre-roll is already inside the pass (its start is the record start), and nothing lands before session sample 0 (clamped). (2) When the pass ends, each armed MIDI track's clip starts at the earliest note-on before the record start, else at the record start as before; every note keeps its absolute position, and audio takes are unchanged. (3) With Cycle on, pickups belong to the first take, whose clip starts early; later laps are unchanged, and a first take that now overlaps earlier material follows the existing overlap and take rules (ADR 0039). This paragraph narrows decision 1 for a behavior the maintainer decided; it is docs-only and needs no judging round.

### Amendment: The lap limit (#522, #567, #634, 2026-11-13)

(1) **One lap window per pass, at most 10,000 laps (`take_laps::MAX_LAPS`).** A pass's laps are cut once, from its start, its loop and its end, and every recorded track of the pass, audio and MIDI, keeps the same laps. The pass's end is set at Stop: the furthest of the Stop position, the take files and the last captured MIDI event. When crash recovery places a pass, which has no Stop, the end is the furthest of its take files and its MIDI journal's last event; recovery's audio and MIDI paths read the same files, so they agree. A track's laps still run through all the material it holds; a file a few frames longer than the host's clock is not cut short. Empty MIDI laps make no clip. Audio keeps the rule below that a last partial lap shorter than an eighth of the loop is no take; the status line now says so, with the frames, their duration and the take file that keeps them (#634). For scale: 10,000 laps are over five and a half hours of a one-bar loop in 4/4 at 120 BPM.

(2) **Why there is a limit, and why the newest laps.** Placing and settling takes includes work quadratic in lap count per track. Each kept lap adds a clip per recorded track and generally another playlist: 10,000 full laps on 16 audio tracks produce 160,000 recorded clips, before existing material and splits. Placement is one Record undo step built on session snapshots. This is not a disk-space or byte-memory limit, and 10,000 is an engineering choice, not a measured safe maximum. Past the limit, transit keeps the newest laps. That preserves the performer's latest attempts and the newest-take playback model of ADR 0039. Ableton documents take lanes without a numerical limit and a new lane per recording pass ([Live manual, Comping](https://www.ableton.com/en/manual/comping/)). Logic documents adding successive cycle passes to a take folder ([Logic Pro guide, Record multiple audio takes](https://support.apple.com/guide/logicpro/record-multiple-audio-takes-lgcpb19806af/mac)). Neither source establishes a cutoff or which laps to drop; transit's 10,000-lap cutoff and oldest-first eviction are transit's engineering choices.

(3) **What the user is told.** After Stop, the status line ends with the lap notice, after every other report of the pass, so none replaces it. It gives the count once ("Cycle recording keeps the newest 10000 laps as takes: N earlier laps were left out", or "1 earlier lap was"), then where they remain: each affected audio take file, and the MIDI file. If the MIDI file could not be written, it gives the reason and where the journal is kept. With more than three places, the line gives the count of files and "see the log", and the log lists every one.

(4) **Where the laps left out remain.** Audio stays in the take file, which always holds every lap. MIDI is exported once, during Stop placement or journal recovery, before the kept MIDI clips are placed. The export is a Standard MIDI File with one musical track per affected track that has material in the laps left out, at unrolled session positions on the session's tempo map. Every note that starts before the cutoff is whole in the file, with its real note-off even when that falls after the cutoff. A sustain pedal held at the cutoff keeps its real release. Controller changes before the cutoff are in the file, later ones in the kept take. In the kept takes, the first kept lap resumes a note held into it from the lap start, as every lap after a wrap does (decision 1 and the note rule in `crates/engine/src/midi_take.rs`). Pedal state is not carried, as at every wrap (K58). If the laps left out hold no MIDI, no file is written and the notice says so. The file goes in the journal's folder, which is where the audio takes go, or the saved session's Audio Files folder when there is no journal path. It is named after the session with the first unused number, `<session> left-out laps NN.mid`, and an existing file is never overwritten. It is the user's: no cleanup removes it. If the file cannot be written, the journal is kept and exempt from the cleanup that removes a journal after Stop, and the notice names it, so the MIDI is not lost. `midi_journal::export_left_out` writes the file from a kept journal later without placing clips; it has no menu entry yet (see the result note for the options).

(5) **The earlier cap was a defect.** Until this amendment, the audio and MIDI take builders silently kept only the first 1,000 laps. The newest laps were dropped, and their MIDI was lost when the journal was deleted (#522).

(6) **The same cut for audio and MIDI,** including a pass started inside the loop while the transport was already cycling. Audio used to become one clip running past the loop end there (#567). The loop a take recorded under is kept in the recording plan, so crash recovery cuts it the same way.

## Consequences

- One switch decides the behavior, so a recording never stops for a reason the user cannot see.
- A long Cycle recording can stack many takes; stopping remains the user's control. The lap-limit amendment supersedes the original no-cap decision: at most 10,000 laps of a pass become takes, and omissions must be reported (#522).
- A user who wants a single pass over a region turns Cycle off and punches in by selection.
- `EditState::loop_record` stays as a write-only field, always saved as `false` through v0.3.x so v0.2.0, which requires it, can open our sessions; it is dropped after v0.4.0.
- Stop mid-lap keeps the partial lap as a take (MIDI: passes = offset / lap length + 1). For audio, a partial lap shorter than one eighth of the lap is no take, a rule `add_recording` already had; since the lap-limit amendment the status line says so, and the frames stay in the take file (#634). Making it a take instead is a product choice left open.

## Alternatives rejected

- **Keep Loop Record as a preference defaulting to on.** It keeps the case where two switches disagree, which is what the issue was about.
- **Warn when Record is pressed with Cycle on and Loop Record off.** A warning explains the surprise rather than removing it.

## Related

- Issue #18; `docs/contributing/knowledge/recording.md`.

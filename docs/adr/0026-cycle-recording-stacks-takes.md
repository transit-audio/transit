<!-- transit-index
title: With Cycle on, Record always loops the cycle range and stacks a take per lap until Stop; the Loop Record option (`edit.loop_record`, `options.loop_record`, Alt+L) is removed and old files that carry it still load
-->
# ADR 0026: With Cycle on, Record always loops and stacks takes; Loop Record is removed

- **Status:** accepted 2026-10-24 (decided by Derek, #18)
- **Date:** 2026-10-24
- **Reopen when:** users ask to record a single pass through the cycle range and stop at its end (a one-shot punch over a loop); or the lap limit or its stopping policy needs revision.

## Context

Recording used to depend on two switches that had to agree. Cycle looped playback, and a separate Loop Record option (Options menu, Alt+L, `options.loop_record`, `EditState::loop_record`) made Record stack one take per lap. With Cycle on and Loop Record off, Record punched out at the loop end: the pass stopped at the first wrap, and a player overdubbing a guitar had to find out why nothing was recording on the second lap (#18). Cubase and Live both loop and stack takes whenever the cycle is on.

## Decision

1. **Cycle on means Record loops.** From stopped, Record starts at the cycle start after any count-in. While already playing, Record punches in at the current playhead; a pass started inside the cycle first runs to the cycle end, then wraps. Recording continues until Stop, or until the pass reaches the lap limit. Take construction follows the pickup and lap-limit amendments and the partial-lap rule below. As described in ADR 0039, the original active playlist remains active, with the newest take playing over its range and earlier material around it. (Wording amended 2026-11-13 by the lap-limit amendment: the earlier text said every start began at the cycle start, which the punch-in path never did.)
2. **Cycle off means one open-ended pass.** Record runs from the cursor (less pre-roll) until Stop, as before.
3. **The Loop Record option is removed:** the Options menu item, its Alt+L shortcut, the `options.loop_record` command, the `loop_record` field of `EditState`, and the Loop Rec toggle in the legacy transport panel.
4. **Migration.** A session file that still carries `loop_record: true` or `false` loads unchanged: the value is ignored (`skip_deserializing`). Test: `a_session_saved_with_the_removed_loop_record_option_still_loads`.
5. **Tests.** `record_with_cycle_on_loops_and_stacks_a_take_per_lap_until_stop` (three laps are three takes of one lap each, and Stop ends the pass), `record_with_cycle_off_is_one_open_ended_pass_until_stop`, and `three_laps_of_cycle_recording_are_three_audio_takes_each_one_lap_long`.

### Amendment: Pickups (#327, 2026-11-04)

Notes played during the record count-in join the take (Logic Pro's behavior; Ableton Live does not record during its count-in and offers Capture MIDI instead; the maintainer chose Logic's). (1) MIDI capture for a pass begins at the count-in start on the same device-frame clock the pass latches on, so a note played with N samples of count-in left lands at the record start less N; the pre-roll is already inside the pass (its start is the record start), and nothing lands before session sample 0 (clamped). (2) When the pass ends, each armed MIDI track's clip starts at the earliest note-on before the record start, else at the record start as before; every note keeps its absolute position, and audio takes are unchanged. (3) With Cycle on, pickups belong to the first take, whose clip starts early; later laps are unchanged, and a first take that now overlaps earlier material follows the existing overlap and take rules (ADR 0039). This paragraph narrows decision 1 for a behavior the maintainer decided; it is docs-only and needs no judging round.

### Amendment: The lap limit (#522, #567, #634, 2026-11-13)

(1) **A cycle recording pass records at most 10,000 laps (`take_laps::MAX_LAPS`).** A pass's first lap runs from its recording latch to the first wrap, also when Record was pressed inside the loop; every later lap runs from one wrap to the next. The host counts completed laps on the control side, once a frame, from the pass's unwrapped clock: the output frames the audio thread has consumed since the latch, the same clock the MIDI take's offsets are measured on (`take_laps::completed`, read by `end_recording_at_lap_limit` in `crates/ui-studio/src/host/recording.rs`). A frame that comes late, or that sees the playhead where an earlier one saw it laps before, misses no wrap. The audio thread's only addition is one relaxed store of the frame counter its timeline already keeps, beside the position it already publishes. At the first check that sees 10,000 completed laps, the host requests Stop as Record pressed again does: the transport stops and the takes are placed. Every lap up to there is a take on every armed track, audio and MIDI alike; nothing is left out. What the pass recorded after its 10,000th wrap and before the Stop took effect is one more lap window, zero-based lap 10,000, under the ordinary partial-lap rules and with no notice of its own: on audio a take when at least an eighth of the loop long, otherwise no take and the short-lap notice names the take file that keeps it; on MIDI a clip when it holds any event, whatever its length. The status line then says "Cycle recording ended at its limit of 10000 laps; press Record to continue" (status lines write counts as plain digits, as the other recording messages do), the log records it, and Record starts a new pass at once. For scale: 10,000 laps are over five and a half hours of a one-bar loop in 4/4 at 120 BPM.

(2) **The cut never drops a lap.** The cut makes at most 10,001 ordinary lap windows, zero-based laps 0 to 10,000 (`take_laps::cut`). A pass that reaches lap 10,001 or later, by its end or its material (a stall longer than a lap at the limit, or crash recovery of a pass that ran on) keeps laps 0 to 9,999 as separate takes and merges everything from lap 10,000 on into one final take, lying from the loop start for as long as it holds, and the status line says "Cycle recording ran past its limit of 10000 laps: its last N laps are one longer final take". Every recorded frame and every MIDI event then stays in a clip. Short of that, the partial-lap rules of (1) apply: a short audio window is not a clip, and its frames stay in the take file the notice names.

(3) **One pass end for every track.** A pass's laps are cut once, from its start, its loop and its end, so every recorded track of the pass, audio and MIDI, decides from the same end whether the transport wrapped. The pass's end is set at Stop: the furthest of the Stop position, the take files and the last captured MIDI event. When crash recovery places a pass, which has no Stop, the end is the furthest of its take files and its MIDI journal's last event; recovery's audio and MIDI paths read the same files, so they agree. Empty MIDI laps make no clip, and the first MIDI take of a pass goes on the playlist the track plays, as the first audio take does, so it is heard even when the first lap held nothing. Audio keeps the rule below that a last partial lap shorter than an eighth of the loop is no take; the status line now says so, with the frames, their duration and the take file that keeps them, or with their exact count and "see the log" when more than two take files have one (#634). MIDI has no such threshold: a last lap that holds an event is a clip.

(4) **Why there is a limit.** Placing and settling takes includes work quadratic in lap count per track. Each lap adds a clip per recorded track and generally another playlist: 10,000 full laps on 16 audio tracks produce 160,000 recorded clips, before existing material and splits. At Stop, audio and MIDI placement are folded into one Record undo step built on session snapshots; crash recovery places them separately. This is not a disk-space or byte-memory limit, and 10,000 is an engineering choice, not a measured safe maximum. Ableton documents take lanes without a numerical limit and a new lane per recording pass ([Live manual, Comping](https://www.ableton.com/en/manual/comping/)). Logic documents adding successive cycle passes to a take folder ([Logic Pro guide, Record multiple audio takes](https://support.apple.com/guide/logicpro/record-multiple-audio-takes-lgcpb19806af/mac)). Neither documents a limit on the passes of a cycle recording; ending the recording at 10,000 laps is transit's engineering choice. The host work at the limit is unmeasured; the open follow-up is [#636](https://github.com/transit-audio/transit/issues/636) (milestone v0.7.2): measure Stop placement, the autosave and take-lane drawing at 10,000 laps, on a representative multitrack pass.

(5) **The earlier cap.** Until this amendment, the audio and MIDI take builders silently kept only the first 1,000 laps; the later laps got no clip, and their MIDI was lost when the journal was deleted (#522).

(6) **The same cut for audio and MIDI,** including a pass started inside the loop while the transport was already cycling. Audio used to become one clip running past the loop end there (#567). The loop a take recorded under is kept in the recording plan, so crash recovery cuts it the same way.

(7) **Known limits.** The merged final take of (2) runs past the loop end, and as the newest take it plays over whatever follows the loop on the playing playlist. At a limit stop, the last few milliseconds are a short lap on audio (no clip) but a tiny clip on MIDI when an event falls in them.

## Consequences

- One switch decides the behavior, so a recording never stops for a reason the user cannot see.
- A long Cycle recording can stack many takes; stopping remains the user's control. The lap-limit amendment supersedes the original no-cap decision: a pass ends at 10,000 laps, every lap a take, and the status line says why it ended (#522).
- A user who wants a single pass over a region turns Cycle off and punches in by selection.
- `EditState::loop_record` stays as a write-only field, always saved as `false` through v0.3.x so v0.2.0, which requires it, can open our sessions; it is dropped after v0.4.0.
- Stop mid-lap keeps the partial lap as a take (MIDI: passes = offset / lap length + 1). For audio, a partial lap shorter than one eighth of the lap is no take, a rule `add_recording` already had; since the lap-limit amendment the status line says so, and the frames stay in the take file (#634). Making it a take instead is a product choice left open.

## Alternatives rejected

- **Keep Loop Record as a preference defaulting to on.** It keeps the case where two switches disagree, which is what the issue was about.
- **Warn when Record is pressed with Cycle on and Loop Record off.** A warning explains the surprise rather than removing it.

## Related

- Issue #18; `docs/contributing/knowledge/recording.md`.

<!-- transit-index
title: A pre-count zone before bar 1: one capture model (everything from the count-in start is captured, the clip's left edge decides what is heard, a trimmed edge hides and never destroys); when something is played during a MIDI record count-in, the clip covers the whole count-in, at the session start or later alike; positions before bar 1 are signed samples and ticks through the model, the session start is derived (the earliest clip start, never after bar 1) and nothing new is stored; bars count down before bar 1 (0, -1, ...) as Logic does; MIDI first, audio pickups in #442; a `pre_count` need warns older builds
-->
# ADR 0052: A pre-count zone before bar 1

- **Status:** proposed 2026-11-10
- **Date:** 2026-11-10
- **Implementation:** not started. Slices in the Slices section. Issues #441 (this decision), #440 (the stopgap it replaces), #327 (pickups, whose clip-start rule it changes), #442 (audio during the count-in), #330 (retrospective capture).
- **Reopen when:** audio pickups (#442) need an audio clip to start before bar 1; a user asks to drag the session start by hand (Logic's project start marker); a tempo or meter change before bar 1 is asked for; a hardware sync output (ADR 0043) has to start a receiver inside the zone; or the derived session start costs a frame budget on a session with many clips.

## Context

A player records MIDI from the very start of a session with a count-in, plays into the downbeat while the click counts, and loses what they played: nothing exists before the session start. Today three places clamp a pickup to sample 0, and the take builder turns a note played wholly before it into a sliver on the first tick (#440):

- `MidiTake::timed` (`crates/playback/src/player/device.rs`) clamps each offset at `-point.position.sample.max(0)`.
- `Journal::events` (`crates/engine/src/midi_journal.rs`) clamps at `-h.position.max(0)`.
- `build_wrapping` (`crates/engine/src/midi_take.rs`) computes `floor = -cut.start.max(0)`, `lead = earliest.max(floor).min(0)`, and maps every kept event to `e.offset.max(lead)`, so a note-on and note-off before the floor land on the same sample and `close` gives the note length `(b - a).max(1)`.

The clip start today follows #327: `pickup` returns the earliest note-on before the latch, and the first stretch starts there. A punch takes no pickup. With Cycle on, pickups join the first lap only (ADR 0026, amendment of 2026-11-04).

**A MIDI clip's left edge destroys today; its right edge hides.** `trim_start` (`crates/engine/src/edit.rs`), the one start trim for the trim drag, Trim to Selection, Trim Start to Insertion, range copy, retime and split, re-anchors notes by the cut's ticks and, for a clip that is not looped, drops what the cut left before the new start (`sequence.notes.retain(|n| n.start >= shift)`, the same for controllers). A looped clip keeps those notes at negative ticks, because later cycles play them (ADR 0019 decision 4; on the tick grid the shift comes from `LoopFrame::trim_to`). `Clip::trim_end_to` (`crates/model/src/clip.rs`) only shortens `length`: notes past the end stay in the sequence and do not sound (`played_note` in `crates/mix/src/midi.rs` keeps a note only when it starts before the clip's grid end), so dragging the end back out restores them. Playback of a clip that is not looped clamps a note at a negative tick to tick 0 (`played_note`, `n.start.max(0)` when `keep_negative` is false), so it sounds at the clip's first tick, while a controller at a negative tick does not sound (`c.tick >= 0 || looping.is_some()`). `split_clip` partitions the notes of a clip that is not looped between the two halves.

**The time model is already signed.** `Samples` is `i64` (`crates/time/src/lib.rs`), `Range` is a signed half-open pair, and `MAX_POSITION` bounds positions at ±2^40. `TempoMap` (`crates/time/src/tempo.rs`) extends its first tempo and its first meter backward: `tick_to_samples` of a negative tick and `samples_to_ticks_f` of a negative sample both use the tick-0 tempo, `bar_start_tick` documents "bars before 1 extend the first meter backwards", and `bar_beat_at_tick` returns bar 0 for the bar before bar 1 (test `negative_positions`). `format_position` and `parse_position` (`crates/time/src/format.rs`) print and read negative Min:Secs and negative bars. `GridValue::line_at_or_before` (`crates/time/src/grid.rs`) uses `div_euclid`, so snapping is correct on either side of zero. `Clip::sanitize` (`crates/model/src/clip.rs`) bounds a clip start to ±`MAX_POSITION`, not to zero, and MIDI notes already carry negative ticks after a start trim of a looped clip (ADR 0019 decision 4). The player's count-in entry (`count_in` in `crates/playback/src/player/audio.rs`) accepts any start within ±`MAX_POSITION`. `position_param` (`crates/engine/src/cmd/mod.rs`) clamps protocol positions to ±`MAX_POSITION`.

**The floor at zero is policy, spread over call sites.** Commands clamp at zero one by one: `transport.play`, `transport.locate`, `transport.rewind` and `transport.loop` (`crates/engine/src/cmd/transport.rs`); clip moves (`move_clips` in `crates/engine/src/edit.rs`, `c.start.saturating_add(delta).max(0)`) and the ripple floor (`ripple_spans`); the edit selection (`crates/engine/src/cmd/edit.rs`); `view.scroll` (`crates/engine/src/cmd/view.rs`, `clamp(0, MAX_POSITION)`); markers and automation points (`crates/engine/src/cmd/event.rs`, `crates/engine/src/cmd/mix.rs`); MIDI clip creation (`crates/engine/src/cmd/midi_time.rs`, `crates/engine/src/cmd/event_more.rs`); the bounce default range `Range::new(0, content_end)` (`crates/engine/src/cmd/file.rs`, `crates/engine/src/cmd/realtime_render.rs`); `bounded_to_track` (`crates/engine/src/io.rs`). In the UI: `drop_start` (`crates/ui-studio/src/arrange.rs`, "never before zero"), `click_ruler` (`crates/ui-studio/src/actions.rs`), `follow_playhead`, the grid overlay's first bar (`crates/ui-studio/src/arrange/grid_overlay.rs`, `.max(0)` and `.bar.max(1)`), `begin_pass` (`crates/ui-studio/src/host/recording.rs`, pre-roll start `.max(0)`), and `recording_pass_inspect` (`capture_start` clamped at 0). The timeline click (`render` in `crates/mix/src/click.rs`) starts its scan at `.max(0)` ticks, so it never sounds before bar 1. A grep for `.max(0)` finds about 50 non-test sites in `crates/engine/src` and about 60 in `crates/ui-studio/src`; not all are positions.

**Conventions.** Ableton Live does not record during its count-in: its manual says Live "will not begin recording until the count-in is complete", and the count-in "runs from negative bars-beats-sixteenths" up to 1.1.1, shown in the position fields; notes played early are recovered with Capture MIDI. Logic's count-in plays before the playhead and switches to record when it reaches it (Apple's Logic Pro help), and Logic's project start marker can be dragged left in the ruler, giving bars before bar 1 (Apple's manual for the marker; forum reports put the limit at bar -8 and say regions before 1.1.1.1 block moving it). That Logic records notes played during the count-in into a region that starts before the project start was not verified from Apple's documentation; this ADR takes it as the maintainer's decision (#327, #441), not as a cited fact. Where Logic starts a MIDI region when notes were played during a count-in in the middle of a project (at the first note, at the bar line before it, or at the count-in start) was not verified either; the maintainer's rule (decision 1.2) does not depend on it. For audio, Apple's Logic Pro 8 manual says "Any audio present during the count-in will be recorded"; that the recorded audio region then starts at the record position and its left edge can be dragged back to reveal the count-in audio was not found in Apple's documentation reachable while drafting, and is not cited as verified. Whether Live's Arrangement can hold a clip before 1.1.1 was not verified.

## Decision

### 1. Decided by the maintainer (fixed)

1. People play into the downbeat while the click counts them in. What they play during the count-in is saved into the clip.
2. **One rule everywhere.** When something was played during the count-in, the clip covers the whole count-in, whether the recording starts at the session start or later in the session. One bar of click gives one bar before the record start; a longer count-in gives as many bars as it lasts. At the session start this is the pre-count zone: the zone's length is derived from the count-in length. This changes #327, where the clip starts at the first pickup (decision 9 lists the tests that change).
3. **Only when something was played.** The zone, and the early clip start, appear only when something was played during the count-in. A take with nothing played early starts at the record start, as today.
4. **Bars count down before bar 1, as Logic does:** bar 0, bar -1, and so on. Bar 1 stays where the song starts.
5. **MIDI first.** This ADR covers MIDI pickups only. Audio played during the count-in is wanted and is tracked as #442 (decision 14 says what would have to change).
6. Workflow follows what Logic and Ableton agree on unless transit can do clearly better; no inherited Pro Tools construct is binding.
7. Alpha: no migration shims. A session with content before bar 1 is a clean break, said loudly (decision 13).
8. **One capture model for MIDI and audio.** Everything from the start of the count-in is captured; the clip's left edge decides what is heard. Only the default edge differs. MIDI (this ADR): when something was played during the count-in, the edge opens to the count-in start, so an early note is seen and heard at once. Audio (later, #442): the edge stays at the record start, because count-in audio is mostly room bleed and floor noise; dragging the edge back reveals it (decision 14).
9. **Trimming a MIDI clip's left edge hides the early notes; it does not destroy them.** Dragging the edge back out restores them (decision 17).
10. **No catch range.** Nothing moves slightly early notes onto the clip start: notes stay where they were played.

"Something was played" means a note-on during the count-in, which is what `pickup` keys on today (proposed; decision 8 says what happens to controllers).

### 2. Representation: signed positions, a derived session start

**Positions before bar 1 are negative samples and negative ticks, through the model and the saved file.** Sample 0 stays bar 1 and tick 0 stays bar 1. Nothing new is stored: the zone is where the earliest clip starts.

**`Session::start()`** (new, `crates/model/src/session.rs`) is the earliest `start` of any clip on any playlist of any track, or 0 when that is later than 0. It is never after 0. Every playlist counts, not only the active one, so choosing a take (ADR 0039) never moves the session start. The zone is `[Session::start(), 0)`; it is empty in every session that exists today. It sits beside `content_end()`.

**The floors move from 0 to `Session::start()`** where a position may address the zone (decisions 5 to 8, 10, 12), and stay at 0 where it may not (decision 11). Each site is changed on purpose, never by a global search and replace.

**Why this one.** The brief asked for an honest comparison of three:

- **(a) Signed positions.** The types, the tempo map, the bar numbering, the formats, the grid, clip sanitizing and the player's position check are already signed (Context). ADR 0019 and its 2026-11-09 amendment hold across zero: before the first tempo event the tempo map is one linear segment through tick 0, so `tick_to_samples` stays monotonic and one conversion per position; `grid_start` and `grid_end` round a clip edge to the nearest tick on either side of zero (`to_samples` rounds half away from zero symmetrically, which keeps monotonicity); a looped clip's `LoopFrame` origin is a tick, negative or not; half-open ranges compare the same way. The cost is the floor sites above, each a one-line change with a test per class, plus the UI drawing that starts at bar 1.
- **(b) A session-start offset** (sample 0 is the zone start, bar 1 is a displayed offset, every stored position non-negative). `TempoMap` places bar 1 at tick 0 and sample 0 (`sanitize` forces a tempo and a meter at tick 0), so either every conversion gains an origin term or bar 1 moves off tick 0. Creating a zone after content exists rewrites every stored sample position (clips, automation points, markers, key signatures, loop range, selections, playhead, `timecode_start`, the recording plan and journal), which is (c) in another name, and changes every Min:Secs reading of the song. A fixed headroom given to every session from the start would change every existing file and is a Pro Tools-style construct.
- **(c) Shift content later and renumber.** The code already has this: `event.move_song_start` (`crates/engine/src/cmd/event_more.rs`) rebuilds the tempo map and moves musical material through `retime::move_material`. It moves only ticks-timebase tracks, so audio and samples-timebase material would part from MIDI, one record pass would become a whole-session edit inside the "Record" undo step, and time readings (Min:Secs, timecode, a bounced file's sync) would move under the user.

(a) keeps the invariants with the least risk. It is chosen.

### 3. Tempo and meter in the zone

The zone has the tempo and meter in effect at bar 1: the first tempo event and the first meter event, extended backward, as `TempoMap` already does. No tempo or meter event can be placed before bar 1 (`set_tempo` and `set_meter` clamp to tick 0, `sanitize` drops negative ticks; unchanged). A record start at bar 1 counts in at bar 1's tempo and meter (`count_in_for` in `crates/ui-studio/src/metronome.rs` measures "the bar containing `at`"), so the count-in and the zone agree. A change of bar 1's tempo re-places the zone's clips through `retime` like any other ticks-timebase clip; their samples change, their ticks do not (proposed test under Evidence).

A count-in from a later record start whose bar has another tempo can reach back across bar 1 (for example a two-bar count-in from bar 2 after a tempo change at bar 2). Its clicks were at that bar's tempo; the zone's grid is at bar 1's. The notes stay where they were played in time, as every pickup does today; they may sit off the zone's grid lines. Accepted.

### 4. The ruler, bar numbers, grid and snapping

- The ruler, the grid lines and the arranger lanes start at the session start. `grid_overlay::lines` starts from the bar containing `Session::start()` instead of `.max(0)` and `.bar.max(1)`; bar labels come from `bar_beat_at_tick`, which already counts 0, -1, ... (decision 1.4).
- Snapping in the zone uses the same `GridValue` functions, which already handle negative positions.
- The view can scroll to the session start (`view.scroll` floors at `Session::start()`, as do `follow_playhead` and the scroll gestures), and no further.
- The counter shows bar 0 and negative bars in Bars|Beats and a leading minus in Min:Secs (both already formatted).
- Proposed: the zone is drawn on a slightly darker lane background so bar 1 reads as where the song starts. The color is a token added under ADR 0045 in the same landing.

### 5. Transport

- **Return to Zero** (`transport.rtz`, Home) goes to the session start: the zone start when there is one, else bar 1 as today. Play from there plays the pickups, which is what #441 asks for ("playback from the start plays them"). Proposed; see Open.
- `transport.play {from}`, `transport.locate`, `transport.rewind` and `click_ruler` floor at the session start.
- **The click sounds in the zone** during playback when Click is on: `render` in `crates/mix/src/click.rs` drops its `.max(0)` and steps beats from the session start's bar. Without this, playing from the start would be silent until bar 1 although the music started a bar earlier. Proposed; see Open.
- **Pre-roll** at bar 1 still starts at bar 1 when there is no zone (the pre-roll start floors at `Session::start()` in `begin_pass`). With a zone, pre-roll may start in it. Proposed.
- **Hardware sync** (ADR 0043): Song Position counts sixteenths from 0 and cannot address the zone. A followed Start or Song Position locates at or after bar 1, unchanged. A sync output started inside the zone is not designed here; see Open.

### 6. Loop and punch

- The loop range and the punch range floor at the session start instead of 0 (`transport.loop`, the loop strip, `options.loop_playback`'s four-bar default). With no zone this is today's behavior.
- A loop that starts at bar 1 with Cycle on and a count-in: the count-in clicks before bar 1. When something is played during it, the first lap's clip covers the whole count-in (decision 1.2) and starts in the zone; later laps are whole loops, as `build_wrapping` already does for pickups. The transport loop itself stays where the user set it; playback loops back to bar 1, never into the zone.
- A punch takes no pickups (unchanged): a punch keeps its range.

### 7. Editing

- **MIDI clips may sit in the zone.** A MIDI clip may be moved, trimmed, split, pasted or drawn anywhere at or after the session start. A move or paste never goes before the session start: it stops there, as it stops at sample 0 today. So a drag does not grow the zone (proposed; the user cannot extend the zone by hand; see Open).
- **A move of several clips** stops for all of them when the first one reaches its floor, keeping their spacing (`ripple_spans` already keeps spacing at its floor). An audio clip's floor is bar 1 (decision 11), so a selection with an audio clip stops when that clip reaches bar 1.
- **The zone disappears with its content.** Delete the zone's clips, or move them to bar 1 or later, and the session start returns to 0. Undo brings it back, because it is derived from the clips undo restores.
- Notes inside a MIDI clip keep their clip-relative ticks; nothing in the piano roll changes, since a clip's own tick 0 is its start.
- `event.move_song_start` refuses while the session start is before 0, with a message naming the zone, because its `.max(0)` would crush zone clips onto bar 1. Proposed; see Open.

### 8. Recording

**The capture.** Nothing changes on the audio thread: MIDI capture already runs from the count-in start on the device-frame clock and stamps events against the latch (`crates/playback/src/player/audio.rs`). The three floors go:

- `MidiTake::timed` drops `.max(floor)`.
- `Journal::events` drops `.max(floor)`.
- `build_wrapping` drops `floor`; `lead` no longer clamps at `-cut.start`.

**The count-in length reaches the take builder.** `Cut` (`crates/engine/src/midi_take.rs`) gains `count_in: Samples` (0 when there was none). `Recording` (`crates/engine/src/io/recording.rs`) gains `count_in: Samples` with `#[serde(default, skip_serializing_if = "is_zero")]`, filled by `plan_recording` from the plan's `CountIn::frames` when the transport was started from a stop with a count-in. The journal `Header` (`crates/engine/src/midi_journal.rs`) gains the same field, `#[serde(default)]`; `VERSION` stays 1.

**The clip start.** In `build_wrapping`, when the pass has no punch and a note-on falls before the latch (`pickup` finds one), the first stretch starts at `-cut.count_in`: the clip starts at the record start less the whole count-in, wherever the record start is. When nothing was played during the count-in, the first stretch starts at the latch, as today (decision 1.3). A journal written before this change has `count_in` 0; recovery then falls back to the earliest pickup, as #327 does (proposed: the journal cannot say how long its count-in was).

**Events kept.** Proposed: with a pickup, every event from the count-in start is kept, controllers included, so a sustain pedal pressed before the first note is in the clip. Today `build_wrapping` keeps only events at or after the earliest note-on. Without a pickup, nothing before the latch is kept, as today (`without_a_pickup_nothing_before_the_latch_is_kept_and_the_clip_starts_at_the_latch` stays).

**Where the clip lands.** The clip start is `cut.start - cut.count_in` in samples: the device-clock position of the count-in start. At a record start on bar 1 this is within half a sample per counted beat of bar `1 - N`'s tick, because `CountIn::frames` is a rounded beat times the beats (`count_in_for`). ADR 0019's grid start rounds the clip edge to its tick, so the clip sounds from the bar line. Proposed; see Open on deriving the count-in from the tempo map.

**Record start in the zone.** A recording may start from a cursor in the zone; its count-in reaches further back, and when something is played during it the zone grows to the new clip's start. Positions stay within `-MAX_POSITION` (`Clip::sanitize`). No other cap (proposed; see Open). `wrapped` in `midi_take.rs` drops its `cut.start >= 0` condition, so a record start in the zone that runs into a cycle still cuts its laps.

**Loop recording.** Pickups belong to the first take, whose clip covers the whole count-in; later laps are unchanged (ADR 0026 amendment, item 3).

**Overlap.** The clip's range decides the overlap rules, as today: one MIDI clip in the way merges (overdub); several, or a loop, keep the old ones as the previous take on a duplicated playlist (`place_midi_recording_wrapping`). Because the whole count-in is covered, a take recorded right after an earlier clip now overlaps it for the whole count-in instead of from the first pickup, and so merges into it, or duplicates the playlist, more often than before. This follows from decision 1.2; see Open.

**#440's stopgap.** #440 drops a count-in note that ends before the session start, with a status message, a user guide line and a known issue. Once the floors are gone, nothing falls before the session start, so that drop and its message are removed in the recording slice, the guide line is replaced by one describing the zone, and the known issue is closed. If #440's stopgap lands first, it is a short-lived fix; if this ADR's recording slice lands first, #440 closes with it.

### 9. The tests that change because of decision 1.2 (one rule everywhere)

Every `Cut` literal gains `count_in` (mechanical). These change their expectations:

- `crates/engine/src/midi_take.rs`: `a_pickup_before_the_latch_starts_the_clip_early_with_every_note_where_it_was_played` (the clip starts at the count-in start, not at 72,000; note ticks move by the difference); `pickups_join_the_first_lap_only_and_later_laps_are_unchanged` (the first lap starts at the count-in start, not 84,000); `a_pickup_before_the_session_start_is_clamped_at_sample_zero` (replaced: the pickup is placed in the zone where it was played).
- `crates/engine/src/midi_journal.rs`: `a_journal_with_a_pickup_before_its_latch_recovers_the_clip_from_the_pickup` (a header with `count_in` recovers the clip from the count-in start; kept as is for a header without it).
- `crates/ui-studio/src/midi_pickup_tests.rs`: `a_pickup_one_beat_early_starts_the_clip_a_beat_before_the_record_start_in_one_undo_step`, `cycle_recording_puts_the_pickup_in_the_first_take_and_leaves_the_second_unchanged`, `a_pickup_before_the_session_start_is_clamped_at_sample_zero` (replaced), and `the_inspected_pass_reports_where_capture_began` (`capture_start` is no longer clamped at 0).
- `crates/ui-studio/src/midi_record_tests.rs`: `events_on_other_tracks_are_not_recorded_and_a_count_in_pickup_starts_the_clip_before_the_latch`.
- `crates/playback/src/player/audio/midi/tests.rs`: `a_pickup_before_the_session_start_is_clamped_at_sample_zero` (replaced: the offset is the full distance from the latch).

These keep their expectations: `without_a_pickup_nothing_before_the_latch_is_kept_and_the_clip_starts_at_the_latch`, `a_punch_keeps_its_range_and_takes_no_pickup`, `a_pass_with_no_pickup_places_exactly_as_before`, `stop_during_the_count_in_or_with_nothing_played_records_nothing`, and the player's count-in tests in `crates/playback/src/player/audio/tests/count_in.rs`. The implementer confirms the list by running the suite on the slice, and records any other test whose expectation moves.

### 10. Export and bounce

- The default bounce range starts at the session start instead of 0: `Range::new(s.start(), content_end)` in `bounce_job`, `bounce_paths_job` and the real-time render (`crates/engine/src/cmd/file.rs`, `crates/engine/src/cmd/realtime_render.rs`). The pickups are in the file. With no zone this is today's range. Proposed; see Open.
- Stems share that start, so they line up with each other and with the mix.
- `bounded_to_track` (`crates/engine/src/io.rs`) floors at the session start, so bounce in place and the offline track renders reach the zone.
- The session length shown anywhere (`crates/engine/src/io/session.rs` derives seconds from `content_end`) is `content_end - start`.
- `transport.go_to_end` is unchanged.

### 11. Audio clips, automation, markers and the other lanes

Narrowest rule for now (proposed): **only MIDI clips may start before bar 1.**

- Audio clips keep their floor at bar 1 (`clip.place_source`, imports, moves of audio). #442 decides otherwise for audio pickups.
- Automation points stay at or after bar 1 (`automation.set_point`, `l.set_point(at.max(0), ...)`). In the zone a lane reads its value at bar 1 (its first point's value), so a MIDI pickup plays through the mixer as bar 1 sets it.
- Markers, key signatures, tempo and meter events stay at or after bar 1.
- A MIDI clip in the zone plays through its track's instrument and inserts like any clip; nothing in `crates/mix/src/midi.rs` assumes a non-negative clip start (`base` is the clip start's tick, positive or negative; the `.max(0)` sites there are clip-relative ticks).

### 12. The control protocol

- Positions are already signed within ±`MAX_POSITION` and parse as negative bars (`"bars:-1|1|000"`) or negative Min:Secs.
- Floors at the session start: `transport.play`, `transport.locate`, `transport.rewind`, `transport.loop`, `transport.rtz` (to the session start), `view.scroll`, the edit selection, clip moves and pastes of MIDI clips, MIDI clip creation in `midi_time.rs` and `event_more.rs`, and the default `start` of `file.bounce_mix`, `file.bounce_paths` and real-time renders.
- Floors at bar 1 (unchanged): `clip.place_source`, imports, `automation.set_point`, markers, tempo and meter commands, `event.move_song_start` (which refuses while a zone exists).
- `ui.inspect`'s recording pass reports `capture_start` unclamped and gains `count_in` as the plan carries it.
- A read that reports the session's extent reports `start` beside the end (proposed: the same reply that gives the content end today).
- `docs/` protocol pages say which commands accept the zone.

### 13. The saved file and older builds

- No new field on the session, no `format_version` change, no migration: a clip's `start` is already a signed number in the file.
- **A `pre_count` need (ADR 0023).** `Session::needs()` lists `pre_count` when `Session::start()` is before 0, and `KNOWN_NEEDS` gains it. A build with the marker and without this entry warns on load that parts of the session may be missing or sound different, and that saving can drop them. The marker is a warning, not a shim: alpha's no-migration rule stands.
- **What an older build does with a session that has content before bar 1.** It loads the file; `Clip::sanitize` keeps the negative start. Its ruler and scroll stop at bar 1, so the zone is out of sight and out of reach; Return to Zero and Play go to bar 1, so the pickups do not play (a clip starting in the zone plays only its part after bar 1). Its bounce starts at bar 1 and leaves the zone out. Saving without touching the clip keeps it whole. Moving the clip, or anything else that runs `move_clips` over it, clamps its start to bar 1 and shifts every note later by the zone's length: silent damage. Builds since the marker (ADR 0023) warn first; builds before it do not. The release notes say this loudly.
- A clip with hidden notes (decision 17) plays them at its first tick in an older build; the `midi_hidden_notes` need warns builds since the marker.

### 14. Audio during the count-in (#442, not decided here)

Under the one capture model (decision 1.8) audio differs only in its default edge: the take file holds everything from the count-in start, and the clip's left edge stays at the record start, so the count-in audio (mostly room bleed and floor noise) is hidden until the user drags the edge back. An audio clip's left edge already hides rather than destroys: a start trim moves the clip's offset into its file (`trim_start`, "audio clips advance into the file"), so the hidden audio is still in the file. What #442 would have to change: the audio recorder keeps input from the count-in start (today the audio take starts at the latch, and the input may not be rolling before it); the take file then begins before the latch, so the clip is placed with a positive source offset equal to the count-in; dragging that edge back before bar 1 needs audio clips in the zone (decision 11's bar-1 floor lifted for audio); and disk-backed sources (ADR 0035) prime from that offset. Audio has no note-on, so decision 1.3 does not apply: the edge does not open by itself. #442 decides these.

### 15. Retrospective MIDI capture (#330)

#330 places a captured phrase at the cursor with the first note on the nearest beat. With the cursor at bar 1, notes before that first beat land before bar 1; they go into the zone under the same rule as pickups (MIDI clips may start before bar 1, the session start follows the clip). Nothing in #330's design changes.

### 16. Real-time rules

Nothing here adds allocation, locking or blocking on the audio thread. Positions on the audio thread are already `i64` and the player already accepts any start within ±`MAX_POSITION`. `Session::start()` is computed on the control and UI threads, over the clip lists they already walk, and never reaches the audio thread except as a position it can already receive. The click change in `crates/mix/src/click.rs` is integer and float arithmetic in the existing loop. The capture changes only remove clamps in code that runs on the control thread (`MidiTake::timed` on the main-thread facade, the journal and the take builder). Decision 17's playback change replaces a clamp by a skip in `played_note`, which allocates nothing.

### 17. A MIDI clip's left edge hides; it never destroys

Decision 1.9 is not true today for a clip that is not looped (Context): a start trim drops the notes it cuts off, and a note left at a negative tick sounds at the clip's first tick. The right edge already hides. The rule:

1. **The sequence keeps every note; the edges decide what sounds.** A note or controller of a MIDI clip that is not looped sounds only when it starts at or after the clip's grid start and before its grid end (ADR 0019's amendment, item 4). The rest is hidden: kept in the file, not played, not sent to an external output, not chased, not consolidated, not bounced.
2. **`trim_start` keeps what it cuts off.** It drops its `retain` for clips that are not looped, so the cut-off notes and controllers stay at negative ticks, as a looped clip's already do. Dragging the edge back out (a start trim with a negative delta, which already shifts notes later by the same ticks) brings them back where they were played.
3. **`played_note` skips a note that starts before tick 0 in a clip that is not looped**, instead of clamping it to tick 0 (`keep_negative` false). The same function serves playback and the external output and chase (`crates/mix/src/external_midi.rs`), so both follow. Controllers already skip negative ticks. A note that starts before the edge and would sound past it is hidden whole (proposed; see Open).
4. **The piano roll draws and edits the visible notes only.** Hidden notes are not drawn, not selected by Select All and not touched by quantize, transpose or delete. Proposed: a later view option may draw them dimmed outside the edges.
5. **Split stays a partition.** `split_clip` gives each half the notes on its side, as today; a split is a cut, not a trim (proposed; see Open).
6. **Consolidate** (`consolidate_midi`) already keeps only notes inside `0..len` and so writes the visible notes only; unchanged.
7. **ADR 0019's amendment, item 4,** says a trim that clears a loop under `MIN_CYCLE` leaves the notes it moved to negative ticks "where playback starts them at its first tick, all at once". Under this rule they are hidden instead. That clause is updated in the same change.
8. **Older builds.** A build without this rule plays a hidden note at the clip's first tick. `Session::needs()` lists `midi_hidden_notes` when any MIDI clip that is not looped holds a note or controller before tick 0, and `KNOWN_NEEDS` gains it (ADR 0023), so builds since the marker warn.

**How much work.** Small in the engine: two `retain` lines removed in `trim_start`, a clamp turned into a skip in `played_note`, the need, the ADR 0019 clause. The piano roll (`crates/ui-studio/src/keys/`) needs an audit of every place that walks a clip's notes for drawing, hit-testing, selection and the edit commands, and a filter to the visible range; its size is not known until that audit. It is its own slice (Slices, slice 2), and it lands before recording, because a take whose clip opens to the count-in start must be trimmable without losing the early notes.

**Recording does not create hidden notes.** A take's clip opens to the count-in start (decision 1.2), so every captured note is visible. Hidden notes appear only when the user trims. No catch range (decision 1.10): a note played a few ticks before the downbeat stays a few ticks before it.

## Evidence an implementation must carry

Deterministic tests on the existing fixtures, no wall-clock assertions, no device access:

- **Model.** `Session::start()` is 0 for an empty session and for one without negative clips, the earliest clip start otherwise, and counts clips on inactive playlists. `needs()` lists `pre_count` exactly when it is negative; a session with a zone round-trips through save and load with the clip start and its notes unchanged; a build that removes `pre_count` from `KNOWN_NEEDS` warns on load (the ADR 0023 pattern).
- **Take builder** (`crates/engine/src/midi_take.rs`). A pickup at the session start places the clip at `-count_in` with every note where it was played; a note played wholly during the count-in keeps its length (the #440 symptom, inverted: negative control at the base commit gives a one-tick sliver); a pass with nothing played early starts at the latch; a later record start with a pickup covers the whole count-in; a punch takes no pickup; loop recording gives the pickup to the first lap only, at the session start and later; a record start in the zone grows the zone; controllers from the count-in start are kept with a pickup and dropped without one.
- **Journal.** A header with `count_in` recovers the same clip the live path placed; a header without it falls back to the earliest pickup; nothing is clamped at sample 0.
- **Player** (`crates/playback/src/player/audio/midi/tests.rs`, `count_in.rs`). A pickup before the session start keeps its full offset from the latch. Playing from a negative position renders a zone clip's notes on their samples at block sizes 1 and 512; the click sounds on the zone's beats with Click on. Negative control: the base commit is silent before bar 1.
- **ADR 0019 across zero.** A looped MIDI clip that starts in the zone plays every repeat on `tick_to_samples(t + k·C)`, across bar 1, at two rates and four tempos; a split at bar 1 of a clip spanning it sounds what the unsplit clip sounded.
- **Retime.** A tempo change at bar 1 keeps a zone clip's notes on their ticks.
- **Edits.** A move of a zone clip by a positive delta keeps it in the zone (negative control: the base commit's `move_clips` clamps it to 0); a move earlier stops at the session start; a mixed MIDI and audio move stops at the audio clip's bar-1 floor with spacing kept; deleting the zone's clip returns the session start to 0 and undo restores it; `event.move_song_start` refuses with a zone.
- **Transport and protocol.** `transport.rtz` goes to the session start; `transport.locate` to a negative position is kept with a zone and floored at 0 without one; `view.scroll` floors at the session start; `file.bounce_mix` without a range starts at the session start, and its stems share it.
- **UI.** The grid overlay labels bars 0 and -1 in the zone and starts at bar 1 without one; a ruler click in the zone sets the cursor there.
- **Hidden edges (decision 17).** A start trim of an unlooped MIDI clip past two notes, then a trim back to the original start, gives back the same sequence and the same rendered events (negative control: the base commit loses both notes); while trimmed, playback, the external output's chase and a bounce sound neither hidden note (negative control: a note left at a negative tick sounds at the clip's first tick at the base commit); consolidate writes the visible notes only; Select All and quantize in the piano roll leave hidden notes untouched; a session with hidden notes lists `midi_hidden_notes` and round-trips; ADR 0019's cleared-loop trim leaves its negative-tick notes silent.
- **No catch range.** A note played 10 ticks before the downbeat during the count-in lands 10 ticks before it.

## Slices

Each builds, passes the gates, and is useful or invisible on its own.

1. **Session start and floors (invisible).** `Session::start()`, the `pre_count` need, and every floor of decisions 4 to 7, 10 and 12 moved to it; the grid overlay, ruler and scroll from the session start; Return to Zero to the session start; the click in the zone. Invisible because no build creates content before bar 1 yet: `start()` is 0 for every session. Useful for a session that carries a negative clip from the protocol or a hand-edited file: it shows, plays and exports whole. Tests: model, edits, transport and protocol, UI, player playback and click, ADR 0019 across zero, retime.
2. **The left edge hides (decision 17).** `trim_start` keeps cut-off notes for unlooped clips; `played_note` skips notes before tick 0; the piano roll audit and visible-range filter; the `midi_hidden_notes` need; ADR 0019's amendment clause updated. Useful on its own: a trimmed MIDI clip gives its notes back when the edge is dragged out. Tests: hidden edges.
3. **Recording into the zone (the feature).** `Cut`, `Recording` and the journal header carry `count_in`; the three floors go; the one-rule clip start; controllers from the count-in start; `wrapped` without its non-negative condition; #440's drop and message removed; the tests of decision 9 changed. Tests: take builder, journal, player capture, the UI pickup tests, no catch range.
4. **Docs.** The user guide's recording and editing sections (the zone, one rule everywhere, the hiding left edge, what an older build does), the protocol pages, a known issue for older builds that predate the marker, the ADR 0026 amendment pointer to this ADR, the README Features line, and the zone's lane token in the design docs (ADR 0045).

Slices 1 and 2 are independent and may land in either order; both land before slice 3. Slice 4 may ride with slice 3.

## Consequences

- A count-in take keeps what the player played, where they played it, at the session start and later alike, and playing from the start plays it.
- Takes started later in the session now start a whole count-in early when something was played early, so they overlap the previous clip more often and merge into it, or start a take stack, more often than under #327.
- Every position floor becomes a choice between 0 and the session start; a new command that takes a position must make that choice. The review checklist gains "does this position floor at 0 or at the session start?".
- A file with a zone is damaged by moving its zone clips in a build that predates this ADR (decision 13).
- A MIDI clip's left edge becomes non-destructive, matching its right edge and audio clips' left edge; a clip file can carry notes nobody hears, which the piano roll does not show until a view option does.
- The zone's grid can differ from the clicks a count-in played when the record bar's tempo is not bar 1's (decision 3).

## Alternatives rejected

- **A session-start offset with non-negative stored positions** (decision 2, b): moves the tempo map's origin or rewrites every stored position when a zone appears.
- **Shifting content later and renumbering** (decision 2, c): `event.move_song_start` already does it for ticks-timebase material; it parts audio from MIDI, makes a record pass a whole-session edit, and moves time readings.
- **A stored zone length** (a `pre_count` field on the session): a second source of truth beside the clips, a field to keep in step on delete, move and undo, and a zone that can outlive its content. Deriving it from the clips gives decision 1.3 for free.
- **A zone from the first pickup only** (#327's rule at the session start): the maintainer decided the whole count-in (decision 1.2).
- **A catch range** that moves notes played slightly before the clip start onto it: notes stay where they were played (decision 1.10).
- **Keeping the destructive left trim** for clips that are not looped: a take that opens to the count-in start could then not be tidied without losing the early notes (decision 1.9).
- **Ableton's model** (no recording during the count-in, Capture MIDI afterward): the maintainer chose Logic's for pickups (#327); Capture MIDI stays planned as #330.

## Open for the maintainer

These are gaps found while drafting; each has a recommendation and the narrowest rule this draft uses until it is settled.

1. **Return to Zero with a zone.** Recommend the session start (the zone start), so Home and Play play the pickups; the alternative is bar 1, with the zone reachable only by scrolling. Logic's Go to Beginning behavior relative to a moved project start was not verified.
2. **The click in the zone during playback.** Recommend it sounds when Click is on (the zone is musical time and the take was played against a click). Alternative: silent before bar 1, as today.
3. **Default bounce start.** Recommend the session start, so an export holds the pickups. Alternative: bar 1, with the zone exported only by a selection.
4. **The overlap rules and the longer lead.** One rule everywhere makes a take merge into, or stack over, the previous clip across the whole count-in. Recommend keeping today's rules on the clip's range (merging loses nothing). Alternative: decide merge and take stacking from the first captured event, at the price of the new clip overlapping the old one without merging, which `place_clip` resolves by trimming the old clip.
5. **Controllers during the count-in.** Recommend keeping every event from the count-in start once a note-on was played (a pedal pressed before the first note is kept); a count-in with only controllers makes no zone.
6. **Extending the zone by hand.** Recommend no handle for now (the zone follows its content; a drag stops at the session start), with Logic's draggable project start as a later decision if asked for.
7. **A cap on the zone.** Recommend none beyond `MAX_POSITION`; the alternative is the longest count-in (16 bars, `count_in_for`'s clamp) before bar 1.
8. **`event.move_song_start`** (an inherited Pro Tools construct). Recommend it refuses while a zone exists, and a separate decision on retiring it.
9. **Count-in clicks from the tempo map.** The count-in's beats are a rounded beat length times the beat count, so a long count-in drifts up to half a sample per beat from the zone's grid. Recommend a follow-up that places count-in beats by `tick_to_samples` of their ticks, so the click and the zone's grid agree exactly; not needed for this ADR.
10. **A sync output started in the zone** (ADR 0043). Song Position cannot express it. Recommend: an output sends Start when the transport crosses bar 1, and nothing before, as an ADR 0043 amendment when its output slices land.
11. **The zone's lane color.** Recommend a darker lane background token, added under ADR 0045.
12. **A note that starts before the left edge and runs past it.** Recommend it is hidden whole (it starts outside the clip; a partial note would sound with no attack). Alternative: it sounds from the edge for its remaining length. Logic's behavior here was not verified.
13. **Split and hidden notes.** Recommend a split stays a partition (each half gets the notes on its side), because a split is a cut, not a trim. Alternative: each half keeps the other's notes hidden, so either half's edge can be dragged back over them.
14. **Showing hidden notes.** Recommend none in the first slice; a later view option draws them dimmed outside the clip's edges in the piano roll.

## Related

[ADR 0019](0019-clip-internal-loops.md) and its 2026-11-09 amendment (tick grid, grid start and grid end), [ADR 0023](0023-session-compatibility-markers.md) (the `needs` list), [ADR 0026](0026-cycle-recording-stacks-takes.md) and its pickups amendment, [ADR 0039](0039-takes-and-comping.md) (take stacks), [ADR 0043](0043-hardware-sync-transport-and-clock.md) (Song Position), [ADR 0045](0045-design-language-in-code.md) (tokens). Issues #441, #440, #327, #442, #330.

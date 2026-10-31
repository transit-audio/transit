# Recording

A lost take is a merge-blocking defect. Treat every change here as high risk.

- **Capture:** `crates/playback/src/record.rs` opens the input stream with the most channels the
  device offers and hands frames to the plan bridge (`plan_bridge.rs`) for monitoring and takes.
- **Record-start latch:** `crates/playback/src/record/start.rs`. A start is a prepared operation
  with a `Ticket` (engine epoch plus a non-wrapping sequence). The audio side marks it started when
  the applied action runs, not when the UI asks, so the take's start frame is the real one. A
  delayed or cancelled ticket can never start a later take. `start/mapping.rs` maps the start to
  the session position.
- **No-clock latch:** a record pass does not need a validated output clock map. MIDI-only passes latch on the output frame; duplex audio aligns by input frame ordinal within one block.
  With no validated clock after 1 s the start fails with a plain-English message, never raw Debug text.
- **Placement:** takes are placed from the latched device frame and the clock fit, not from the
  time the user pressed the key (`clocks.md`).
- **Input selection:** monitoring and recording must resolve an input path the same way
  (`crates/engine/src/io.rs`; known issue list for gaps).
- **Data safety:** take data is the user's work. Never add a path that can discard captured frames
  on stop, device loss or a full buffer without surfacing it; cap allocations, but never by dropping
  a take silently.
- **MIDI recording:** `crates/playback/src/midi_capture.rs` is a preallocated lock-free ring (64Ki
  events, a reserve kept for note-offs) the audio thread fills with each admitted live event and its
  device frame; the ring is shared by all tracks, and a full ring drops the event and counts it
  (surfaced as events lost). The same record-start latch and `start/mapping.rs` map frames to the
  session, so a MIDI take lines up with an audio take. `crates/engine/src/midi_take.rs` assembles
  takes at Stop: pairs note-ons with note-offs, converts to ticks, merges an overdub into the one
  clip under the pass, duplicates the playlist when several clips are in the way, and makes one
  playlist per loop pass. With Cycle on, Record always loops and stacks takes (one per lap until Stop; no
  separate Loop Record option, ADR 0026). A recording that started before the loop end and ran past it while
  Cycle was on is cut at each wrap into one clip per lap; a note held across the loop end is cut
  there and resumes at offset zero in the next lap with the same pitch and velocity, repeating
  at each wrap until the real release. Each loop lap stays exactly loop-length; a release exactly
  at the loop end produces no remainder. Journal recovery uses the same rule.
- **Take stacks** (#25, [ADR 0039](../../adr/0039-takes-and-comping.md)): after placing passes,
  both recorders call `crate::takes::settle_new_takes`: the track plays the playlist it played
  before the passes (never the last pass's playlist alone, which silenced the rest of the track),
  the passes get take numbers (`Clip::take`) after the material they cover, and each pass is made
  to play in turn so the newest is on top. Material a take covers is numbered by
  `takes::number_covered` right after the playlist duplicate, inside the same "Record" step (never
  before it: `squash_undo` keeps the oldest snapshot). Audio cycle recordings over clips duplicate
  the playlist like MIDI ones. Choosing a take moves clips between playlists (`takes::activate_clip`);
  the mix still reads only the active playlist.
- **Cycle changed during a take** (#81): a pass keeps the loop it started with until Stop
  (`StudioApp::take_cycle`); laps and MIDI clips are cut by it, and the player is not retargeted
  meanwhile. Cycle turned on mid-take plays straight through with no lap cuts; turned off, the
  take keeps looping. The change reaches the transport when the take ends, if it is still running.
- **MIDI take journal:** `crates/engine/src/midi_journal.rs`. The control thread appends each drained event (newline JSON, header with the latch and plan first, flushed every 250 ms and at Stop) to `<Audio Files>/<session>_<stamp>.midijournal`, never the audio thread; a clean Stop deletes it.
  A crash leaves it named in the autosave's `recording.json`; the next launch's recovery prompt rebuilds `<Track>_recovered` through `midi_take::build`, and Discard deletes it.
  The header's `loop_range` is the loop the take recorded under (`take_cycle`, #85), `None` if it played straight through. Recovery cuts one clip per lap by that loop (ADR 0026: one take per lap, the partial last lap kept, a held note continued across each wrap) and never reads the session's Cycle state at recovery time, so a Cycle or region edit mid-take before a crash changes nothing. Tests: `recovery_cuts_laps_by_the_loop_the_take_recorded_with_not_the_cycle_state_now` and the controller and straight-through cases beside it in `midi_journal.rs`.

Tests: `crates/engine/tests/recording.rs`, `crates/playback/tests/monitor_contract.rs`,
`crates/playback/tests/publication_contract.rs`.

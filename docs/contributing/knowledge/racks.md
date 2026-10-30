# Racks

Design: [ADR 0034](../../adr/0034-racks-layering-instruments-through-track-groups.md) and its slice 1
amendment. A rack is Ableton's Instrument Rack and Logic's Track Stack (summing stack) built from a
routing folder: the folder sums its members through its bus as any group does, and its own MIDI
plays every member through the member's key and velocity zones.

## Where things are

- **Model** (`crates/model/src/rack.rs`): `Rack`, `RackMember`, `Zone` (the fade rule in `Zone::gain`,
  the velocity in `RackMember::velocity`), `Macro`, `MacroMapping` (`travel` and `plain`: range,
  curve, the captured `ParamRange`). `Session::rack_of` and `Session::rack_members` decide which
  members play (listed, inside the folder, MIDI or instrument, at most `MAX_RACK_MEMBERS`).
- **Clips** (`crates/mix/src/midi.rs`, `schedule_notes`): a member plays the rack's clips merged in
  time with its own, as a separate source in its note table (`events.rs`, `RACK_CLIP_SOURCE`).
- **Live input** (`crates/mix/src/rack.rs`): playback routes a monitored or armed rack to
  `rack_binding`; `MixEngine::queue_event` fans each event out to the members (fixed array, no
  allocation). An off goes only to members that hold the note (`InstrumentEvents::holds_note`).
- **Macro lanes** (`crates/mix/src/automation.rs`, `apply_params` with the session): a mapped
  parameter follows the rack's macro lane while the lane has points.
- **Commands** (`crates/engine/src/cmd/rack.rs`): `rack.new`, `rack.member_add`,
  `rack.member_remove`, `rack.zone`, `rack.macro`, `rack.macro_map`, `rack.macro_unmap`,
  `rack.macro_name`, `rack.info`. Joining or leaving the folder (`track.move_to_folder`,
  `track.remove_from_folder`) keeps the member list in step. A macro's MIDI Learn source lives on the
  rack (`Macro::midi`), merged into the mappings in force by `MappingSet::with_racks`.
- **UI** (`crates/ui-studio/src/rack_panel.rs`): the rack card in the device chain; the badge in
  `arrange/tracks.rs` and `mixer/strip.rs`; Group into Rack in `groups.rs`.

## Rules

- A note-off follows the note-on that was delivered, never the zones of the moment.
- Zone fades scale velocity, not level (Ableton's rule for key and velocity zones).
- Nothing on the audio thread allocates: the fan-out is a fixed array, the clip merge a fixed
  scratch. `crates/mix/tests/rack.rs` has the allocation probe.
- A rack field or variant must load in older builds: the rack is skipped when absent, its macro
  lanes use a plain automation slot, its MIDI sources are not in `midi_mappings`.

Tests: `crates/model/src/rack.rs`, `crates/mix/tests/rack.rs`, `crates/engine/tests/rack.rs`,
`crates/ui-studio/src/rack_panel_tests.rs`, the corpus fixture `v0.6.0-rack.transit`.

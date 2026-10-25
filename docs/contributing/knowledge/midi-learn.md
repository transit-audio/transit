# MIDI Learn

Issue #97 slice 1; decision in [ADR 0028](../../adr/0028-midi-learn-mappings.md). Map a controller knob, fader or
button to a mixer or track control: fader, pan, mute, solo, record arm, send level. Plug-in parameters
(with automation write) are slice 2; transport and MMC are slice 3. Input side: `midi.md`; the Monitor
whose Learn spotlight and table style this builds on: `midi-monitor.md`; thread rules: `audio-thread.md`.

## Shape

| Piece | Where | Thread |
|---|---|---|
| Mapping types, the two stores, `resolve` | `crates/model/src/midi_map.rs` | any |
| Session store | `Session::midi_mappings` (absent in old files, skipped when empty) | Main |
| Controller profile | `Engine::midi_profile`, saved in `ui.json` as `midi_profile` | Main |
| Commands `midi.learn`, `midi.learn_cancel`, `midi.map`, `midi.unmap`, `midi.mappings` | `crates/engine/src/cmd/midi_map.rs` | Main |
| Snapshot, matching, pickup and jump, learn candidate | `crates/playback/src/player/midi_map.rs` | ingress worker |
| Pump wiring (`map_pass`), event ring | `player/ingress.rs` | ingress worker |
| Port opening, snapshot swap, retired snapshots | `player/native.rs`, `midi_map::Wanted` | Main |
| Player API | `player/device/midi_map_api.rs` | Main |
| Menu, marker, event application, Window > MIDI Mappings | `crates/ui-studio/src/midi_learn.rs` | Main |

## Mapping

Source: port name (the Monitor's name, `PortKey` display), channel (0 to 15 inside, shown 1 to 16; `any`),
CC or note, number. Target: `track:<id>:volume|pan|mute|solo|arm|send:<slot>` in a session; the profile
uses `strip:<n>:...` (n counts from 1 in session order) because track ids belong to one session.
Mode `pickup` (default) or `jump`; `min` and `max` are the part of the control's travel (0..1) the knob
spans; `toggle` (default true) flips a button on a note-on or a CC rising through 64, false follows the key
or CC >= 64. A target has one source and a source one target, per store (`MappingSet::set` replaces).
Fader and send position is the mixer's own curve (`db_to_k`), pan is `(balance + 1) / 2`.

`resolve(tracks, session, profile)` lists session mappings whose track exists, then profile mappings
whose strip exists, flagging a profile mapping `overridden` when a session mapping has its control or an
overlapping source. Only those not overridden are matched; the window shows the rest dimmed.

## Realtime contract

- Main builds `Snapshot { epoch, entries }` (an entry per wanted mapping, in order, with the ingress input id
  the port name resolved to, or none while the port is closed) and swaps its `Arc` in with `Update::Maps`.
  Replaced snapshots stay in `Wanted::retired` until the worker has let go, so the worker never frees one.
  An equal list of specs changes nothing (the per-entry cells, and so pickup state, survive).
- Per step the worker does `learn.load()` and a check of the snapshot: with no mappings and no armed learn that
  is all (test `without_mappings_or_a_learn_the_worker_does_no_mapping_work`). Otherwise `map_pass` reads every
  open port's pending events without popping them (`for_each_pending`), once each (`Mon::map_next`), so a
  controller no track listens to still maps and a route attached later keeps its backlog. An unrouted port is
  trimmed past 512 pending, as for the Monitor. The first step after the matcher turns on skips what was
  already pending.
- `handle` matches by port id, channel, kind and number; no allocation or lock (`matching_and_publishing_cost_the_worker_no_heap_work`).
  It publishes `MapEvent`s (`Copy`) into a 256-slot SPSC ring; a full ring counts `map_dropped`.
- **Pickup** lives in the worker, against a per-entry `Cell` of atomics: Main writes the control's value and how
  many events of that entry it has applied once a frame; the worker keeps the last published value and count. A
  knob engages when it lands within 1/127 of the control or crosses it between two messages; it lets go when
  Main has applied everything published and the control then differs by more than 1 % (the mouse, undo, automation
  moved it). Jump publishes every message. A note on a continuous control sets it from the velocity.
- **Learn** is Main-side: `Engine::learn` holds the armed control; the UI sets `Shared::learn`; the worker
  forwards the first CC (below 120) or note-on it sees with `learn.swap(false)`, so exactly one candidate
  leaves, and that message does not also move a mapped control. While a learn is armed every present input
  port is open. Main answers a candidate with `midi.map`, keeping the old mapping's mode, range and toggle.

## Main side

`StudioApp::midi_learn_feed` runs first in every frame: copy the profile between preferences and engine,
resolve both stores, hand the worker the specs, take the events, apply them, write the cells. Events carry the
epoch of the spec list; a stale one is dropped. A move uses `set_value_merged` with the key `midi:<target>`, the
same setter a mouse drag uses, so a sweep is one undo step; it ends when the controller has been still for
`GESTURE_END_S` (0.4 s of UI time, injected in tests) or a write pass holds the merge. A button press runs
`mix.mute`, `mix.solo` or `mix.record_arm`. Session `midi.map` and `midi.unmap` are undoable commands; the
profile is per machine and makes no undo step.

## UI

Right-click a control: unmapped offers *MIDI Learn* (session) and *MIDI Learn for this controller (all
sessions)*; mapped shows *Mapped: KeyStep ch 1 CC 74*, *MIDI Learn* (re-learn in the same store) and *Forget
MIDI Mapping*; the control being learned offers *Cancel MIDI Learn (Esc)*. A learning control gets a pulsing
accent outline; a mapped one a small dot at its top right (dimmed when overridden). Escape cancels first
(shortcut row `LearnCancel`, consumed only while armed). Window > MIDI Mappings (`window.midi_mappings`) lists every
stored mapping: port, channel, source, target, mode (editable), range (editable), scope, Learn and Delete.

## Tests

`model/src/midi_map.rs` (stores, conflict, serde), `engine/src/cmd/midi_map.rs` (commands, undo),
`playback/src/player/midi_map.rs` and `ingress/map_tests.rs` (matching through the fake backend and the real
pump), `device/midi_map_tests.rs` (through the Player), `ui-studio/src/midi_learn_tests.rs` (menu, marker,
sweep undo, profile in the preferences, the window).

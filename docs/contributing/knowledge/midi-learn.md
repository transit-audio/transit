# MIDI Learn

Issue #97 slices 1 and 2; decisions in [ADR 0028](../../adr/0028-midi-learn-mappings.md). Map a controller knob, fader or
button to a mixer or track control (fader, pan, mute, solo, record arm, send level) or to a plug-in parameter, built-in or hosted
(slice 2, with automation write). Transport and MMC are slice 3. Input side: `midi.md`; the Monitor
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
CC or note, number. Target: `track:<id>:volume|pan|mute|solo|arm|send:<slot>|plugin:<slot>:<param id>` in a session; the profile
uses `strip:<n>:...` (n counts from 1 in session order) because track ids belong to one session.
Mode `pickup`, `jump` or `scaled` (stored per mapping but ignored: the per-machine takeover setting rules, default `scaled`); `min` and `max` are the part of the control's travel (0..1) the knob
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
  A continuous control's value goes to the mapping's `Cell` (last value wins, with the publish count); Main takes it once a
  frame (`Wanted::take_moves`), so a slow UI loses no final value. Button presses and learn candidates go into a 256-slot
  SPSC ring; a full ring counts `map_dropped`, which the mappings window shows.
- **Consume.** A CC or note-on that a mapping without `pass_through` matches is not delivered to the track's routes. On a routed
  port the match runs where the event is popped (`match_one`), on an unrouted one in the scan. A note-off is consumed exactly when
  its note-on was (`Mon::held`), whatever the mappings are by then. Logic and Ableton both consume; where they agree transit follows.
- **Pickup** lives in the worker, against a per-entry `Cell` of atomics: Main writes the control's value and how
  many events of that entry it has applied once a frame; the worker keeps the last published value and count. A
  knob engages when it lands within 1/127 of the control or crosses it between two messages; it lets go when
  Main has applied everything published and the control then differs by more than 1 % (the mouse, undo, automation
  moved it). Jump publishes every message. A note on a continuous control sets it from the velocity.
- **Takeover** (#122, ADR 0028 amendment) is one per-machine setting, `ui.json` field `midi_takeover` (`scaled` default, `jump`,
  `pickup`), copied into every `MapSpec` in `midi_learn_sync`; the window's selector and a mapped control's menu set it. **Scaled**
  is `scaled_step(at, from, to)` in `crates/playback/src/player/midi_map.rs`: the control moves by the knob's step times the control's
  remaining distance over the knob's remaining distance to the same end, so both reach the end together, no jump; within half a knob
  step it follows exactly, and never against the knob's direction. The first message only records the knob. The control value is
  the worker's own last publish while Main is behind (`applied != published`), else Main's. Scaled and jump set `engaged` so the
  "waits for pickup" status appears only under Pickup. Tests: `scaled_*` in the same file.
- **Learn** is Main-side: `Engine::learn` holds the armed control; the UI sets `Shared::learn`; the worker
  forwards the first CC (below 120) or note-on it sees with `learn.swap(false)`, so exactly one candidate
  leaves, and that message does not also move a mapped control. While a learn is armed every present input
  port is open. Main answers a candidate with `midi.map`, keeping the old mapping's mode, range and toggle.

## Main side

`StudioApp::midi_learn_feed` runs first in every frame: copy the profile between preferences and engine,
resolve both stores, hand the worker the specs, take the events, apply them, write the cells. Events carry the
epoch of the spec list; a stale one is dropped. A move uses `set_value_merged` with one key for the whole gesture (`midi:gesture`), the
same setter a mouse drag uses, so a sweep, even of several controls together, is one undo step; it ends when the controller has been still for
`GESTURE_END_S` (0.4 s of UI time, injected in tests) or a write pass holds the merge. A button press runs
`mix.mute`, `mix.solo` or `mix.record_arm`. Session `midi.map` and `midi.unmap` are undoable commands; the
profile is per machine and makes no undo step.

## Plug-in parameters (slice 2)

Target `track:<id>:plugin:<slot>:<param id>` (profile: `strip:<n>:plugin:...`), the picker's addressing from #65: `<slot>` is an insert
letter `a` to `j` or `instrument`; `<param id>` is the plug-in's stable id (the rest of the text, so it may hold colons). `Control::Plugin
{ slot, param }` in `model/src/midi_map.rs` is never a button; the engine refuses a slot with no plug-in and an unknown built-in parameter
(`check_device`). The playback side is unchanged: a plug-in parameter is a continuous control whose cell holds the knob's 0..1.

Main applies a move in `StudioApp::mapped_param_move`: `plugin_value` (the knob's 0..1 to the parameter's own units, the taper honored: the
same `knob::value_at` a panel knob uses), then `touch_param` and `mix.insert_param` / `mix.instrument_param` under the gesture's merge key.
Because it is a touched parameter, Write, Latch and Touch record it while playing and Read does not; the touch is held `HOSTED_HOLD_S` past
the newest value (no pointer). A mapping whose device is gone is stored, shown, and left out of the snapshot.

Entry points: right-click a built-in editor's control (panel knob, toggle, choice, generic row) for the menu below, with the marker on the
control. Hosted editors draw their own widgets, so the header strip's menu has *MIDI Learn: next touched parameter* (`HeaderAction::LearnNext`;
`EditorHeader::learning` checks it): `StudioApp::learn_next_touched` arms (again, or Escape, cancels), `apply_edits` hands each changed edit
the plug-in reports to `learn_touched`, the first of this editor's becomes the target and `midi.learn` waits for the CC or note.
`check_param` refuses a hosted parameter the host registry does not list (hidden, read-only and meter parameters are never listed).

A hosted instrument's parameter goes through `mix.instrument_param` like a hosted insert's through `mix.insert_param` (it took only
built-in instruments until the fix for K83, so a mapped move of one changed nothing): the session holds the value, the mix hands it to the
plug-in with `set_param`, and the plug-in host tells the controller (`setParamNormalized`) on its main thread, so the editor's knob follows.

## Hosted VST3 parameters

A hosted VST3 parameter follows the plug-in's own curve: the knob's 0..1 goes through the curve the host sampled for a live instance (`soundcraft_vst3_host::param_to_plain` / `param_to_normalized`), not through the registry's Taper approximation (#123). Built-in and CLAP parameters, and a VST3 parameter whose curve is not sampled yet, keep the Taper path.

## Diagnosis in the log

At info level, a few lines per change and none per idle frame: a learn armed, learned (`mapped ... to <port> ch <n> CC <n>`) and armed from a
touched hosted parameter; once the worker has a new list, how many mappings are in force and each one whose port is not open; the first and
last value of a plug-in parameter sweep, or why its first move was not applied (no table, no such parameter, the command's error); and a
knob the worker hears while pickup holds it back (`... at 9% waits for pickup at 70% ...`, also in the status line), once until it moves
its control. The worker counts every message that matches a continuous entry (`Cell::heard`), published or not, for that last line.

## UI

Right-click a control: unmapped offers *MIDI Learn* (session) and *MIDI Learn for this controller (all
sessions)*; mapped shows *Mapped: KeyStep ch 1 CC 74*, *MIDI Learn* (re-learn in the same store) and *Forget
MIDI Mapping*; the control being learned offers *Cancel MIDI Learn (Esc)*. A learning control gets a pulsing
accent outline; a mapped one a small dot at its top right (dimmed when overridden). Escape cancels first
(the mixer's `ReadoutCancel` row, consumed only while armed and no readout is being typed). Window > MIDI Mappings (`window.midi_mappings`) lists every
stored mapping: port, channel, source, target, mode (editable), range (editable), scope, Learn and Delete.

## Tests

`model/src/midi_map.rs` (stores, conflict, serde), `engine/src/cmd/midi_map.rs` (commands, undo),
`playback/src/player/midi_map.rs` and `ingress/map_tests.rs` (matching through the fake backend and the real
pump, plug-in targets included), `device/midi_map_tests.rs` (through the Player), `ui-studio/src/midi_learn_tests.rs` (menu, marker,
sweep undo, profile in the preferences, the window), `ui-studio/src/midi_learn_plugin_tests.rs` (normalized range, learn from the built-in
editor, learn next touched, refusal, automation modes).

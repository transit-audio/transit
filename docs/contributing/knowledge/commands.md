# Commands

Every user action is a command: `crates/engine/src/cmd/` (one file per area), registered with a
`cmd!` row: id, label, menu path, shortcut, params doc, enabled check, run function.

- **Execute:** `Engine::execute(id, params)` runs a command, catches an escaped panic, and pushes one
  undo snapshot (the previous `Arc<Session>`) if the document changed. One command is one undo step.
- **Merged gestures:** continuous gestures (a fader drag) use `execute_merged` so they form one step.
- **`noundo`:** commands that do not change the document (`cmd!(noundo ...)`, such as undo itself,
  or analysis) are registered with `noundo`. A document-changing command must not use it.
- **Refusals are results:** a command that cannot apply returns an error that reaches the status line
  and changes nothing.
- **Reachability:** a command is not done until the UI can reach it (menu, shortcut or control) and a
  test shows it; see `ui.md`.
- **Control channel:** the same commands are exposed to scripts and agents by `crates/automation`
  (`docs/control-protocol.md`, `docs/mcp.md`), so command params are a public contract.
- Menus come from `crates/engine/catalog/menus.txt`; a command lights up a menu item when the menu
  path and label match.

Tests: `crates/engine/tests/` (random edits, hostile params, route cycles).

Vertical track zoom slice 1 ([ADR 0044](../../adr/0044-vertical-track-zoom.md), #314) retains
`track.height {tracks?, height: points | preset}` and its `{"height": <TrackHeight serde value>}`
response, plus selected-track `track.height_up/down {tracks?}` returning `{"changed": n}`.
It adds all-track `track.height_zoom {direction: "in" | "out"}` and `track.height_reset {}`,
both returning `{"changed": n}`, and read-only `query.track_heights {}` returning session-order
`{"tracks":[{"id":1,"height":"Small","points":46}]}`, including hidden tracks.
Numeric requests clamp to 16..440 and round halves up before preset snapping. An unchanged numeric
height preserves the stored variant; an explicit preset or Reset can replace an equal-size Custom/Fit.
Requests validate all targets and values before editing. No-ops do not borrow the mutable session.
Slice 2's drag/wheel will use explicit track IDs and
local UI previews, then one ordinary `Engine::execute` at the gesture boundary; keys/reset each
use one ordinary undoable command. Session replacement/reset/reopen drops previews without a
command. Slice 1 makes `view.zoom_toggle` undoable, caps Fit at 440, and clears its pending
restore record on an actual explicit height change.
The ADR defines input-based commit boundaries, no-op/atomic-refusal behavior and UI reachability.
Height/toggle commands and their undo/redo retain ordinary revision, dirty and history behavior.
Heights use no merge keys: during automation write or MIDI Learn, a changed ordinary command
creates its own undo step, splits the current merge group and leaves the pass/sweep and merge
hold running. Playback synchronization and capture/save remain unchanged; the final height edit
has the same plan-compilation cost as a track rename.
The commands and format are implemented. UI gestures, shortcuts, menu entries and per-track
drawing remain slice 2; the arrangement still draws uniform 46-point rows.

Low Latency adds UI-owned options.low_latency {value?: bool, limit_ms?: number}, returning {value, limit_ms}. Omitting value toggles unless only limit_ms is supplied. Invalid limits are refused, and an engine-only host reports unavailable rather than editing the session. query.path_latency {} is a read-only runtime query with adopted plan/device generations and shared/Main latency components; see routing.md.

Middle C (ADR 0051) adds UI-owned options.middle_c {value?: "C3" | "C4"}, returning {value}; without value it answers the setting. The app saves `middle_c` in ui.json and sets the engine's message convention (`Engine::set_message_middle_c`) in the same step; an engine-only host reports unavailable. No parser reads either: commands that take note names parse them with the fixed C3 = 60 (`transit_midi::parse_note(text, MiddleC::C3, ..)`).

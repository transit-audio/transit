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

Proposed vertical track zoom ([ADR 0044](../../adr/0044-vertical-track-zoom.md), #314) retains
`track.height {tracks?, height: points | preset}` and its `{"height": <TrackHeight serde value>}`
response, plus selected-track `track.height_up/down {tracks?}` returning `{"changed": n}`.
It adds all-track `track.height_zoom {direction: "in" | "out"}` and `track.height_reset {}`,
both returning `{"changed": n}`, and read-only `query.track_heights {}` returning session-order
`{"tracks":[{"id":1,"height":"Small","points":46}]}`. Drag/wheel use explicit track IDs and
`execute_merged`; keys/reset use one ordinary undoable command. The proposal makes `view.zoom_toggle`
undoable, caps Fit at 440, and clears its pending restore record on an actual explicit height change.
The ADR defines input-based merge boundaries, no-op/atomic-refusal behavior and UI reachability.
Height/toggle commands and their undo/redo retain ordinary revision, dirty and history behavior.
Before a player update, the proposed control-side layout comparison verifies every field except
track heights, toggle bookkeeping and horizontal zoom/scroll; a matching snapshot rebinds to the
existing request without audio-plan compilation/publication. Capture/save still checks document
identity and confirmed generation. Mixed audio edits and explicit session replacement keep the
normal request path; neither command names nor suppressed revisions are the bypass criterion.
These additions and behavior changes are not implemented yet.

Low Latency adds UI-owned options.low_latency {value?: bool, limit_ms?: number}, returning {value, limit_ms}. Omitting value toggles unless only limit_ms is supplied. Invalid limits are refused, and an engine-only host reports unavailable rather than editing the session. query.path_latency {} is a read-only runtime query with adopted plan/device generations and shared/Main latency components; see routing.md.

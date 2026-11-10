# Control protocol

transit can be driven completely by other programs: scripts, test harnesses and AI agents.
Every menu item, button, mouse gesture and keystroke maps to a **command** (or to synthetic
input), and every command is reachable from:

| Surface | How |
|---|---|
| Desktop app control channel | `transit --control <port>` (`0` = pick a free port; printed as `TRANSIT_CONTROL_PORT=…`) |
| CLI against a running app | `transit-cli app --port <port> <method or command id> [JSON]` |
| Headless CLI | `transit-cli run --demo --cmd 'mix.volume={"track":"Kick","db":-6}' --bounce out.wav` |
| MCP (headless) | `transit-cli mcp [--demo]` |
| MCP (bridged to the app) | `transit-cli mcp --connect <port>` |

## Authentication (transit)

`transit --control <port>` (or `TRANSIT_CONTROL_PORT`) binds to `127.0.0.1` only. At start it
generates a random 128-bit token (32 hex characters from the operating system's generator), writes
it to `control-token` in the settings folder (`TRANSIT_CONFIG_DIR`; see the README) and logs the
file's path. Set `TRANSIT_CONTROL_TOKEN_FILE` to use another path, for the app and for clients.
The file is readable by the current user only where the platform allows (mode 0600 on Unix, the
folder's per-user permissions on Windows), and a new token replaces it at every start.

Every connection must send `auth <token>` as its first line. A connection whose first line is
anything else, or whose token is wrong, is closed without a reply, and so is one that sends no
first line within 10 seconds. After that the line protocol is unchanged, except that a line that
is not JSON gets one error reply and then the connection is closed. `transit-cli app` and
`transit-cli mcp --connect` read the token file (same path rule) and send the line themselves.
By hand: `printf 'auth %s\n{"id":1,"method":"session.inspect"}\n' "$(cat "$TOKEN_FILE")" | nc 127.0.0.1 $P`.

## Isolated automation profiles

Set `TRANSIT_CONFIG_DIR` to a fresh directory to isolate UI preferences, `Autosave`, `Presets`, and `logs`.
The nonempty value is used as the complete settings directory. Unset retains the normal platform paths;
an explicit empty value disables the settings directory instead of falling back to a personal profile. Save synthetic sessions to explicit scratch paths. Do not use `TRANSIT_NO_PREFS`
when testing preference persistence across restarts.

Run UI acceptance with `--no-audio`. This disables the output player; it does not disable recording
input opened by arm/monitor commands. An automated UI smoke run must never arm a track, enable input
monitoring, or start recording. Control input injects egui events; it does not establish operating-system
input delivery or screen-reader acceptance. Native screenshots use the presented viewport frame.

## Wire format

Newline-delimited JSON over TCP on `127.0.0.1`. One request per line, one reply per line:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "track.new", "params": {"count": 2, "format": "Stereo"}}}
{"id": 1, "ok": true, "result": {"tracks": [12, 13]}}
```

Errors: `{"id": 1, "ok": false, "error": "bad parameters for `mix.volume`: no track named `Nope`"}`.

## Methods

| Method | Params | What it does |
|---|---|---|
| `engine.execute` (alias `command`) | `{command, params}` | Run a command programmatically. **Never opens a dialog**; empty params use defaults or the current selection. |
| `engine.commands` | `{filter?}` | Every command: id, label, menu path, shortcut, params doc, enabled + reason. Includes UI-layer commands (`window.*`). |
| `engine.parity` | `{}` | Parity report against the incumbent's menu catalog. |
| `session.inspect` | `{detail?: "summary"|"full"}` | The document: tracks, clips (with times), inserts (+params when full), sends, routing, automation, markers, groups, busses, sources (`loaded` true when decoded audio is in the pool), selection, zoom (`samples_per_px`, `scroll`), transport, undo label. |
| `ui.inspect` | `{}` | UI state (`window`, panels, dialogs), window size, playing/position, audio device, and `edit_layout` (timeline rect and every track row's rect, for clicking). `editors_open` lists the open plug-in editors as `[track, slot]`; `editor_windows` (issue #173) has `follow`, `main` (`active`, `inactive` or `minimized` as last seen), `posted` (the grouping requests sent to the plug-in host threads, newest last: `follow`, `no_follow`, `park`, `unpark`, `front`) and `editors`, one `{track, slot, state}` per wanted editor with `state` one of `opening`, `not_responding`, `shown`, `parked`, `minimized`, `hidden`, `open` (no window state known), `closed` or `builtin`. |
| `ui.window` | `{minimized?, focus?}` | Minimizes (`minimized: true`) or restores the main window, or asks for it to be focused (`focus: true`; Windows may only flash the taskbar button when transit is not in front), so the editor grouping's transitions can be driven from outside (issue #173). |
| `ui.set` | any `UiState` fields | e.g. `{"window": "Mix", "narrow_mix": true}`. |
| `help.check_updates` | none | Help > Check for Updates…: checks GitHub for a newer release now and reports in `status` (`ui.inspect` also shows `update_notice`). Refused when `TRANSIT_NO_UPDATE_CHECK` is set. |
| `help.debug_logging` | `on?` (boolean) | Help > Debug Logging: raises transit's own log messages to debug for the rest of the session (`on: false` puts them back; without `on` the setting flips). `status` names the log file; the result has `debug_logging` and `log_file`. Not saved. |
| `window.close_editors` | (none) | Window > Close All Editors (326-M): closes every plug-in editor window, built-in and hosted. Answers `{closed: n}`. |
| `window.collapse_editors` | (none) | Window > Collapse All Editors (326-V): folds every open device window to its title bar. Answers `{collapsed: n}`. |
| `window.expand_editors` | (none) | Window > Expand All Editors (326-V): unfolds every open device window. Answers `{expanded: n}`. |
| `window.editors_follow` | `value?` (boolean) | Window > Plug-in Windows Follow transit (issue #173, ADR 0030): plug-in editor windows hide with the minimized main window, come back with it, come to the front when it is activated and have no taskbar button. Off keeps the behavior from before (own taskbar button, no grouping). Without `value` it flips; saved with the preferences (`editors_follow`). |
| `window.midi_monitor` | `value?` (boolean) | Window > MIDI Monitor: opens or closes the window (without `value` it flips). Also clicked from the "MIDI" light in the transport bar. `ui.inspect` shows `midi_monitor`: rows, port header lines, dropped counts, the window and LED rects. |
| `mix.monitor` | `mono?`, `dim?`, `ref?`, `store_ref?` (booleans), `level_db?` (number, `"-inf"` to 0) | The main bus monitoring set (#366), each field optional: Mono (an equal-power mono sum), Dim (-20 dB), Ref (play the stored reference level) and the monitor level; `store_ref: true` stores the monitor level (after `level_db`) as the reference. Monitor path only: never in a bounce, a freeze, a stem or a send. Not undoable: monitor state is a per-machine preference (`monitor` in `ui.json`), not a session edit. Answers the state; `ui.inspect` shows it under `monitor` with `lufs` (integrated LUFS of the mix since the last reset, null before the first gated block) and `lufs_text`. |
| `options.middle_c` | `value?` (`"C3"` or `"C4"`) | Options > Middle C (ADR 0051): the name MIDI 60 carries in every transit window, C3 by default. Without `value` it answers the setting. Saved per machine in `ui.json` (`middle_c`; a missing key reads C3), never in the session or its undo history, and it never changes how a command parses or what it returns: commands read note names as C3 = 60. Engine message text that names a note follows it. Answers `{value}`; `ui.inspect` shows `middle_c`. The engine alone refuses it (it needs the app). |
| `ui.menu.list` | `{}` | Every catalog menu path and the command that implements it (or null). |
| `ui.menu.invoke` | `{path}` or `{id}` | Like clicking the menu item: may open its dialog (e.g. `"Track > New..."`). |
| `ui.click` | `{x, y, button?, count?, cmd?, shift?, alt?, ctrl?}` | Synthetic click (window points). |
| `ui.drag` | `{x, y, to_x, to_y, steps?}` | Synthetic drag with the primary button. |
| `ui.scroll` | `{x, y, dx?, dy?, unit?: "point"\|"line", steps?, cmd?, shift?, alt?, ctrl?}` | A two-finger trackpad swipe over `(x, y)`: `dx`/`dy` points spread over `steps` frames (default 8). Positive values move the content right/down (natural scrolling). `unit: "line"` sends mouse-wheel notches instead. With `cmd` it zooms; with `shift` it scrolls sideways. |
| `ui.zoom` | `{x, y, factor, steps?}` | A trackpad pinch around `(x, y)`: `factor` > 1 spreads the fingers (zooms in). |
| `ui.move` | `{x, y}` | Move the pointer. |
| `ui.key` | `{key, cmd?, shift?, alt?, ctrl?}` | Key press (`Space`, `Enter`, `A`, `F7`, `=`…). |
| `ui.text` | `{text}` | Type text into the focused field. |
| `ui.screenshot` | `{path?}` | PNG of the window (base64 in `png_base64` when no path). |
| `app.quit` | `{}` | Quit. |

## Positions and references

- Tracks: id (number) or name (`"Kick"`).
- Clips: id (from `session.inspect`).
- Positions: samples (`96000`), seconds (`{"seconds": 2.5}`), or a time string in any timebase,
  optionally prefixed: `"bars_beats:5|1|000"`, `"min_secs:0:12.500"`, `"timecode:00:00:10:00"`,
  `"samples:48000"`.
- Ranges: `start` + `end` (or `start` + `length`); omitted = the current edit selection.

## Device pages

The macro strip's pages ([ADR 0047](adr/0047-device-macro-strip.md)) are read and written through
two commands (run them with `engine.execute`). A device is addressed on its track by an insert
letter `a` to `j`, `instrument`, `midi1` to `midi8` (the MIDI effect at that position in the chain,
plus one, when the command runs) or `rack` (a rack card).

| Command | Params | What it does |
|---|---|---|
| `mix.device_pages` | `{track?, device, pages: [{name, slots: [id \| null, ...]}]}` | Replaces the device's user pages, one undo step. At most 16 pages; each page is read by the session file's rules (name trimmed and cut to 32 characters, eight slots, empty, non-string and over-long ids read empty, a repeated id keeps its first slot). `[]` clears them. A rack card, a hardware insert, an External Instrument and a device without parameters refuse it. |
| `mix.device_pages_list` | `{track?, device}` | The device's pages: the factory tier, then the user tier. Each page has `tier` (`factory` or `user`), `name` and eight `slots`, each `{id, name, state}` with `state` one of `ok`, `empty`, `missing` (the id is not in the device's table, or the plug-in is not installed) or `pending` (a hosted plug-in's table is still loading), and `enable` on a factory slot that dims while that toggle is off. |

## Sampler slicing

The Sampler's one slice loop per patch ([ADR 0041](adr/0041-sampler-one-instrument-three-views.md),
#269) is edited through these commands (run them with `engine.execute`). Each edit is one undo
step and a refused edit changes nothing. Frames are the loop source's frames; marker `id`s are
the stable slice ids of the showing set. `track` defaults to the selected instrument track.

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_slice_loop` | `{track?, source, start?, end?, root?}` | Makes the patch's slice loop over the source region (the whole source by default), mode Auto, both sets at the loop start only. `root` (0 to 127) defaults to the pad start note. Refuses a second loop, an unknown source and `end <= start`. |
| `mix.sampler_slice_mode` | `{track?, mode: auto\|manual}` | Shows that mode's own marker set; neither set changes. |
| `mix.sampler_slice_marker` | `{track?, op: add\|move\|remove\|label, frame?, id?, label?, snap?}` | Edits the showing set. `snap` defaults to the loop's setting; a snapped add or move uses the nearest zero crossing within 5 ms (a move is clamped to its neighbors first) and refuses when the source audio is missing. In Auto an accepted edit first copies Auto into Manual (new ids, labels kept, the previous Manual set replaced) and switches to Manual, all one step; an Auto id names its copy. A move or remove of the loop start in Auto copies and switches and reports `fixed: true`; in Manual it is refused. The 65th slice is refused with `Cannot add slice: 64 slices per loop`. A remove frees the zones and pads bound to that slice in that set (`freed_pads`, `freed_zones`). Answers `{id, frame, requested, snapped, clamped, set, mode, copied_auto, fixed, freed_pads, freed_zones}`. |
| `mix.sampler_slice_manual_from_auto` | `{track?}` | Start from Auto's markers: Manual becomes a copy of Auto (labels kept, new ids) and shows. |
| `mix.sampler_slice_manual_clear` | `{track?}` | Clear: Manual keeps only the loop start; Auto is unchanged. |
| `mix.sampler_slice_set` | `{track?, snap?, root?, beat_length?}` | The shared settings (`beat_length` in quarter notes, `null` for the default). The detector settings travel with Detect and are refused here. |
| `mix.sampler_slice_state` | `{track?, at?}` | Read-only: the loop, both sets (per slice its id, frame, end, label, pad, key and zones), the settings, `grid_bpm`, the grid tempo and its origin and the default beat length (both read the session tempo at the placement `at`, and are `null` without it when they need it), and `mapping` (`ascending`, `stale` with the first two `out_of_order` slices, or `none`). |
| `mix.sampler_slice_detect` | `{track?, source?: auto\|cues\|transients\|grid, sensitivity?, division?, bars?, grid_bpm?, at?}` | Auto's detection: prepares a candidate marker set on a worker thread and answers at once with `{candidate_id, state: "pending"}`. Each setting defaults to the loop's stored one (`bars` and `grid_bpm` take `null` to clear) and travels with the candidate. `auto` takes the first nonempty result of the file's cue points, the transient detector and the beat grid; an explicit `cues` without cue points is an empty candidate with a note. The grid's tempo is, in order: an explicit `bars` count (the derived BPM), `grid_bpm`, the file's `acid` tempo (5 to 1000 BPM), the session tempo at `at`. Without `at` the candidate is refused when the grid reads the session tempo or Bars is explicit (`Detect needs a placement (at) to read the session tempo.`). A result of more than 64 slices is refused (`over capacity: N slices, at most 64`), never truncated. One candidate per track: a newer Detect cancels the older one. Not an edit: no undo step, never saved. |
| `mix.sampler_slice_candidate` | `{track?}` | Read-only: the track's candidate, `null` or `{candidate_id, state: pending\|ready\|refused, stale, settings, current_count}` plus, when ready, `source_used`, `markers` (`[{frame, label?}]`), `count`, `note?`, `bpm?`, `tempo_origin?` (`bars`, `grid_bpm`, `acid` or `session`) and `derived_bpm?`, or, when refused, `error`. |
| `mix.sampler_slice_replace` | `{track?, candidate_id}` | Replace markers: Auto takes the candidate's markers and the loop stores the candidate's settings, one undo step. A marker at a frame the old Auto set had keeps its id (and its bindings); the bindings of removed Auto markers are freed (`freed_pads`, `freed_zones`). Manual and the mode are unchanged. Refused on a pending candidate (`Still analyzing.`) and on one detected before any change to the track's patch, undo and redo included, or replaced, discarded or dropped with the instrument or the session (`The patch changed after this detection: detect again.`). Answers `{source_used, markers, kept, new, removed, freed_pads, freed_zones, settings}`. |
| `mix.sampler_slice_discard` | `{track?, candidate_id?}` | Keep current: drops the track's candidate (only that one when `candidate_id` is given). Not an edit. |
| `mix.sampler_slices_to_pads` | `{track?, root?, preview?}` | Slices to pads: slice i of the showing set (never the hidden set or a candidate) plays on trigger note `root + i`, one pad-bound zone per slice; the loop stores `root` (default: the loop's root). Pads bound to the showing set are free for the run; the pad already on note `root + i` takes slice i when it is empty or slice-bound, the rest take the lowest free pads (bank order A1 to D16) and their notes become `root + i`; showing-set pads left over are unbound (their zones empty, their notes stay). A new pad's zone covers its slice with the new-pad defaults; a taken occupied pad keeps its zone and settings. Refused before any change, never truncated: `root + count > 128`, a loose sample pad on one of the run's notes (`Pad A5 already plays E3`), too few pads (`Slices to pads needs 9 pads; 6 are free.`), too few empty zones. `preview: true` answers the same plan (and the refusal as `ok: false, reason`) and edits nothing. Answers `{set, root, count, cells: [{pad, note, slice, id, replaces: null\|{set, slice, id}}], loose_collisions, unbound: [{pad, note, slice, id, zone}], pads_after, ok}`. |
| `mix.sampler_map_slices` | `{track?, root}` | Map slices to keys, the repair for a `stale` mapping: re-notes only the showing set's slice pads so slice i plays `root + i`; every other pad and every zone is untouched, and the loop stores `root`. Refused when a slice has no pad (`Slice 3 has no pad: use Slices to pads.`), when `root + count > 128`, and when a pad outside the run plays one of its notes (`Pad A10 already plays A3`). Answers `{set, root, count, pads: [{pad, note, slice, id}]}`. |

## Examples

```sh
P=$(transit --demo --control 0 | sed -n 's/TRANSIT_CONTROL_PORT=//p')   # or read stderr
transit-cli app --port $P mix.solo '{"track":"Bass"}'
transit-cli app --port $P transport.play
transit-cli app --port $P ui.screenshot '{"path":"/tmp/shot.png"}'
transit-cli app --port $P ui.menu.invoke '{"path":"Track > New..."}'
```

There is no offscreen mode: screenshots come from a running app through `ui.screenshot`.

Appearance defaults to Dark. `ui.theme {"mode":"system"}` follows the appearance supplied by
the native or web integration at launch and on later frames, while the saved choice stays System.
When the integration cannot report an appearance, System uses Dark. Manual Light and Dark stay
fixed when the OS appearance changes. An offscreen render without `--system-theme` supplies no
OS appearance and therefore resolves System to Dark.

## Panel docking

Tracks and Groups, Clips, MIDI Editor, Memory Locations, and Undo History use movable panels. Commands target the active Edit or Mix view; floating panels stay inside the current viewport. Edit and Mix retain separate arrangements; Clips and MIDI Editor belong to Edit. Transport, plug-in, and specialist utility windows retain their existing hosts. The document/session view is protected. Layouts, active tabs, panel visibility, and floating rectangles are saved with the app's existing UI preferences. These operations do not edit document contents.

Panel IDs: `tracks`, `clips`, `midi`, `memory`, `history`. The protected center is `canvas`.

`ui.dock` accepts `operation` and `panel`. Operations are `open`, `close`, `activate`, `float` (with finite positive `rect: [x, y, width, height]`), or `move` (with `target` and `zone: center|left|right|top|bottom`). Center groups with the target; side zones split beside it. `ui.dock.reset` restores the default arrangement. Invalid moves return an error without changing the layout.

Invoke these UI commands through `engine.execute`, for example `{"command":"ui.dock","params":{"operation":"float","panel":"tracks","rect":[100,100,320,440]}}`.

Advanced `ui.dock` operations use the same validated transaction as pointer gestures:
`moveFloating` updates `rect`, `resizeSplit` accepts a boolean child `path` and
`size` (`{"Ratio":0.5}`, `{"FixedFirst":280}`, or `{"FixedSecond":280}`),
`setStackOpen` takes `open`, and `resizeStack` takes a point `height` or null.
A center `move` can include `before` (panel ID or null to append) for tab order.
The equivalent `{"action": ...}` form accepts a serialized shared docking action;
this is also the form emitted by the renderer. Protected-canvas and app-specific panel
rules apply equally to both forms.

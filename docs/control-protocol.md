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
| `mix.monitor` | `mono?`, `dim?`, `ref?`, `store_ref?` (booleans), `level_db?` (number, `"-inf"` to 0) | The main bus monitoring set (#366), each field optional: Mono (an equal-power mono sum), Dim (-20 dB), Ref (play the stored reference level) and the monitor level; `store_ref: true` stores the monitor level (after `level_db`) as the reference. Monitor path only: never in a bounce, a freeze, a stem or a send. Not undoable: monitor state is a per-machine preference (`monitor` in `ui.json`), not a session edit. Mono acts on a stereo main bus only: on a wider one `mix.monitor` refuses `mono: true`, and the state answers `mono: false` (what is heard) beside `mono_preference` (the stored choice, back when the bus is stereo). Answers the state; `ui.inspect` shows it under `monitor` with `lufs` (integrated LUFS of the mix since the last reset, null before the first gated block) and `lufs_text`. |
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

## Sampler import rates

`mix.sampler_load` and `mix.sampler_clear` are removed in 0.7.0 with the one-sample Sampler (ADR
0041 amendment 2026-11-11, #455): every Sampler is a kit. Give a zone its sample with
`mix.sampler_zone_source` (a `path`, a session `source` or a staged `import`), or fill free pads
from a folder with `mix.sampler_load_kit`. `mix.sampler_zone_source` with `source` refuses a session
source whose audio is held at a rate other than its stored one (the old one-sample Sampler's
sample, opened at the session rate): load it from its file.

| Command | Rate and frame policy |
|---|---|
| `file.prepare_import` | `rate: "native"` prepares Sampler PCM without conversion; omitted or numeric rate keeps timeline preparation. Progress includes `sample_rate`, frames at that rate, and duration in seconds. |
| `mix.sampler_zone_source`, `mix.sampler_slice_loop`, `mix.sampler_load_kit` | New file and byte imports retain native PCM through the shared preparation. A staged Sampler import must have been prepared with `rate: "native"`; a converted job is refused and remains staged. A staged import is used up only when the command succeeds: any refusal (an end past the file, a key-map group rule, a memory bound) leaves it staged for a corrected call (#491). Existing source ids keep their admitted PCM and rate. All start/end, loop, crossfade, slice and cue positions and reported lengths count admitted source frames. |
| `mix.sampler_slice_state`, `mix.sampler_state` | Source `rate` is the admitted PCM rate. Milliseconds use that rate; envelope and choke durations use output frames. Older kits keep already-converted PCM and its stored rate, with no migration. |
| `clip.place_source` | `{source, track?, at?}` places a session source on the timeline, where it plays at the session rate. It refuses a source whose stored rate differs from the session rate (a Sampler source keeps its native rate), naming both rates and pointing to `file.import_audio`, with no change and no undo step. It does not convert. A session whose Sampler source stores rate 0 (a damaged file) reads that source at the session rate on open, with a load note. |

## Sampler slicing

The Sampler's one slice loop per patch ([ADR 0041](adr/0041-sampler-one-instrument-three-views.md),
#269) is edited through these commands (run them with `engine.execute`). Each edit is one undo
step and a refused edit changes nothing. Frames are the loop source's frames; marker `id`s are
the stable slice ids of the showing set. `track` defaults to the selected instrument track.
Every edit that changes a set's boundaries or binds zones to its slices (a marker add, move or
remove, Start from Auto's markers, Clear, Replace markers, Slices to pads) also writes each zone
bound to that set's slices with its slice's source, region and loop (`start`, `end`, `loop_start`,
`loop_end`) in the same undo step, so the saved file holds what plays. A zone whose slice is
removed is freed, as each row says.

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_slice_loop` | `{track?, source: id \| path \| import: id, copy?, start?, end?, root?}` | Makes the patch's slice loop over the native-source-frame region (the whole source by default), mode Auto, both sets at the loop start only. A `root` (0 to 127) is stored and absolute; without one the loop follows the pad start note, so slice 1 maps to pad A1. Answers the root in use and `root_stored`. Refuses a second loop, an unknown source and `end <= start`. |
| `mix.sampler_slice_mode` | `{track?, mode: auto\|manual}` | Shows that mode's own marker set; neither set changes. |
| `mix.sampler_slice_marker` | `{track?, op: add\|move\|remove\|label, frame?, id?, label?, snap?}` | Edits the showing set. `snap` defaults to the loop's setting; a snapped add or move uses the nearest zero crossing within 5 ms (a move is clamped to its neighbors first) and refuses when the source audio is missing. In Auto an accepted edit first copies Auto into Manual (new ids, labels kept, the previous Manual set replaced) and switches to Manual, all one step; an Auto id names its copy. A move or remove of the loop start in Auto copies and switches and reports `fixed: true`; in Manual it is refused. The 65th slice is refused with `Cannot add slice: 64 slices per loop`. A remove frees the zones and pads bound to that slice in that set (`freed_pads`, `freed_zones`). Answers `{id, frame, requested, snapped, clamped, set, mode, copied_auto, fixed, freed_pads, freed_zones}`. |
| `mix.sampler_slice_manual_from_auto` | `{track?}` | Start from Auto's markers: Manual becomes a copy of Auto (labels kept, new ids) and shows. |
| `mix.sampler_slice_manual_clear` | `{track?}` | Clear: Manual keeps only the loop start; Auto is unchanged. |
| `mix.sampler_slice_set` | `{track?, snap?, root?, beat_length?}` | The shared settings (`root` 0 to 127 stores it, `null` clears it back to following the pad start note; `beat_length` in quarter notes, `null` for the default). The detector settings travel with Detect and are refused here. |
| `mix.sampler_slice_state` | `{track?, at?}` | Read-only: the loop, both sets (per slice its id, frame, end, label, pad, key and zones), the settings, `grid_bpm`, the grid tempo and its origin and the default beat length (both take the loop file's `acid` tempo when the WAV file carries a usable one, else read the session tempo at the placement `at`, and are `null` without it when they need it), and `mapping` (`ascending`, `stale` with the first two `out_of_order` slices, or `none`). `root` is the root in use and `root_stored` says whether it is stored. `zones` lists every slice-bound zone (either set) exactly as `mix.sampler_state` reports it, with the same keys: the stored `loop_mode`, `crossfade` and `fades` beside the values playback uses, `loop_used`, `crossfade_used` (source frames) and `fades_used`, and the zone's `slice` and derived `region`. |
| `mix.sampler_slice_detect` | `{track?, source?: auto\|cues\|transients\|grid, sensitivity?, division?, bars?, grid_bpm?, at?}` | Auto's detection: prepares a candidate marker set on a worker thread and answers at once with `{candidate_id, state: "pending"}`. Each setting defaults to the loop's stored one (`bars` and `grid_bpm` take `null` to clear) and travels with the candidate. `auto` takes the first nonempty result of the file's cue points, the transient detector and the beat grid; an explicit `cues` without cue points is an empty candidate with a note. The grid's tempo is, in order: an explicit `bars` count (the derived BPM), `grid_bpm`, the file's `acid` tempo (5 to 1000 BPM), the session tempo at `at`. Without `at` the candidate is refused when the grid reads the session tempo or Bars is explicit (`Detect needs a placement (at) to read the session tempo.`). A result of more than 64 slices is refused (`over capacity: N slices, at most 64`), never truncated. One candidate per track: a newer Detect cancels the older one. Not an edit: no undo step, never saved. |
| `mix.sampler_slice_candidate` | `{track?}` | Read-only: the track's candidate, `null` or `{candidate_id, state: pending\|ready\|refused, stale, settings, current_count}` plus, when ready, `source_used`, `markers` (`[{frame, label?}]`), `count`, `note?`, `bpm?`, `tempo_origin?` (`bars`, `grid_bpm`, `acid` or `session`) and `derived_bpm?`, or, when refused, `error`. |
| `mix.sampler_slice_replace` | `{track?, candidate_id}` | Replace markers: Auto takes the candidate's markers and the loop stores the candidate's settings, one undo step. A marker at a frame the old Auto set had keeps its id (and its bindings); the bindings of removed Auto markers are freed (`freed_pads`, `freed_zones`). Manual and the mode are unchanged. Refused on a pending candidate (`Still analyzing.`) and on one detected before any change to the track's patch, undo and redo included, or replaced, discarded or dropped with the instrument or the session (`The patch changed after this detection: detect again.`). Answers `{source_used, markers, kept, new, removed, freed_pads, freed_zones, settings}`. |
| `mix.sampler_slice_discard` | `{track?, candidate_id?}` | Keep current: drops the track's candidate (only that one when `candidate_id` is given). Not an edit. |
| `mix.sampler_slices_to_pads` | `{track?, root?, preview?}` | Slices to pads: slice i of the showing set (never the hidden set or a candidate) plays on trigger note `root + i`, one pad-bound zone per slice. `root` defaults to the loop's stored root, else the pad start note, so a default run puts slice 1 on pad A1. A given `root` is stored (absolute), and every pad of the run stores its note, so a later start-note change cannot split the run. A run without `root` leaves the loop's root as it is: on a loop that stores a root it uses that root and every pad of the run stores its note, exactly like a run given that root; on a loop without one it stores no note for a pad already on its default note, so the run follows the start note (a pad the run re-notes stores its note). Pads bound to the showing set are free for the run; the pad already on note `root + i` takes slice i when it is empty or slice-bound, the rest take the lowest free pads (bank order A1 to D16) and their notes become `root + i`; showing-set pads left over are unbound (their zones empty, their notes stay). A new pad's zone covers its slice with the new-pad defaults; a taken occupied pad keeps its zone and settings. Refused before any change, never truncated: a set without markers (`The Auto set has no slice markers yet: detect or add slices first.`), `root + count > 128` (`Root 121 is too high for 8 slices: the highest root is 120.`), a loose sample pad on one of the run's notes (`Pad A5 already plays E3`; note names in refusal text and in `reason` follow the Middle C setting, `options.middle_c`, while every `note` field is a MIDI number), too few pads (`Slices to pads needs 9 pads; 6 are free.`), too few empty zones (`Slices to pads needs 3 empty zones; 2 are empty.`). `preview: true` answers the same plan (and the refusal as `ok: false, reason`) and edits nothing. Answers `{track, set, root, count, cells: [{pad, note, slice, id, replaces: null\|{set, slice, id}}], loose_collisions, unbound: [{pad, note, slice, id, zone}], pads_after, ok}`. |
| `mix.sampler_map_slices` | `{track?, root}` | Map slices to keys, the repair for a `stale` mapping: re-notes only the showing set's slice pads so slice i plays `root + i`; every other pad and every zone is untouched. The loop stores `root` and every pad of the run stores its note (absolute), so a later start-note change leaves the run where it is. Refused when the set has no markers (as Slices to pads), when a slice has no pad (`Slice 3 has no pad: use Slices to pads.`), when `root + count > 128`, and when a sounding pad outside the run (a loose sample, or a pad bound to the hidden set) plays one of its notes (`Pad A10 already plays A3`, named in the Middle C setting). An empty pad outside the run on one of its notes sounds nothing and is moved instead: empty pads in ascending pad order take the notes the run vacates in ascending order, in the same step. Answers `{track, set, root, count, pads: [{pad, note, slice, id}], moved: [{pad, from, to}]}`. |
| `mix.slice_to_midi` | `{track?, timing: fit\|keep, at, beat_length?, root?, preview?}` | Slice to MIDI: one note per slice of the showing set (key `root + i` from the slices' ascending pad mapping, velocity 100, channel 1, each to the next boundary, the last to the loop end, at least one tick) written as a MIDI clip on the instrument track at tick `at`, one undo step. `at` is required and never guessed: the UI passes the arrangement selection's start when it includes the track, else the edit cursor, else bar 1 (`Slice to MIDI needs a placement.`). `fit` spans the loop over `beat_length` quarter notes (default: the loop's stored beat length, else its duration at the WAV file's usable `acid` tempo, else at the session tempo at `at`), nearest tick with ties upward; `keep` converts native source frames to session samples using the source rate, then converts each boundary through the tempo map, tempo changes inside the range included. Notes are stored as ticks. Over existing material the active playlist is duplicated and the clip becomes a new take (ADR 0039): the original playlist stays active with its clips (the old take cut around the new one), and the duplicate, the alternate playlist, keeps the old clips whole; with no overlap the clip is placed directly. Refused before any change: a placement past the timeline (`The placement is past the end of the session's timeline.`), `beat_length` with keep (`` `beat_length` is for fit timing; keep timing follows the source's duration ``), no slice markers (`The Auto set has no slice markers yet: detect or add slices first.`), no pads (`The slices have no pads yet: use Slices to pads.`), a stale mapping (`The slices' keys no longer run in order: slices 3 and 5 swapped pads.` when two slices exchanged pads, else the first break, `... slice 3 on pad A1 plays key 60, not 63 after slice 2.`; `mix.sampler_map_slices` is the separate repair, never applied here), a `root` that does not match the mapping (`Root 61 does not match the slices' keys, which start at 60: use Map slices to keys.`), `root + count > 128`, a key past 127 (`Slice 4 would play key 128, past 127.`), two slices on one tick (`Slices 2 and 3 land on the same tick; zoom or remap.`), a frozen track, missing source audio the timing needs, a clip past the timeline (`The clip would end past the end of the session's timeline.`). `preview: true` answers the same fields with `clip: null` plus `overlap`, and edits nothing. Answers `{track, clip, notes, first_key, last_key, velocity, channel, at_tick, position, start, end, ticks, timing, beat_length, placement: explicit, takes, overlap? (preview)}`. |

## Sampler pads

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_load_kit` | `{track?, folder: path, preview?: false, replace?: false}` | Loads the folder's immediate WAV, AIFF and FLAC files into free pads in bank order, without overwriting loose or slice-bound pads. With `replace: true` (the caller states the user's choice; the engine never guesses) the files fill pads from A1 as they would an empty kit instead: each pad they fill is first cleared as Clear pad clears it (its zone and any slice binding, name, choke group, mute and solo, and the slot controls of the zones removed and filled; its trigger note stays), pads past the file count, the slice loop and its markers stay, and each replaced pad's referenced source that nothing else names leaves the session (kit recall's rule), all in the same undo step. Without `replace` a folder with more files than free pads is refused; with it, more than 64 files is. Filenames sort case-insensitively, with original spelling breaking ties. Each new zone takes the pad's effective note as its root and the new-pad defaults. Files retain native-rate PCM and remain referenced by path; reported frames are native source frames. The entire load is one undo step ("Load Kit Folder"). Success answers `{track, loaded, pads: [{pad, zone, file, frames, channels}], ignored, first_pad, last_pad, replace, replaced, replaced_slices}` (`replaced`: the loaded pads replaced, `replaced_slices`: how many of them were bound to a slice; both 0 without `replace`); pad and zone numbers start at 1. Admission refusal answers data `{loaded: 0, skipped, needed, free, offender?, notice, files, ignored}`, with no document or undo change. `preview: true` returns the same proposed assignment or admission refusal using headers only; it never decodes audio or edits the document. Full loading additionally verifies decodability. |

Relative folders resolve against the saved session's folder. Hidden entries, directories,
symbolic links and other files (including SFZ mappings) are listed under `ignored` and never
opened. An empty kit, insufficient pads, zones or sources, an unreadable file, more than two
channels, or any source/patch memory limit refuses the whole kit. PCM WAV/AIFF and FLAC with a declared frame count support header-only
admission; a codec without such a header is refused with its reason. Preview cannot certify a
compressed audio body without decoding it, so a later decode error still refuses the whole load.

Pad notes ([ADR 0041](adr/0041-sampler-one-instrument-three-views.md), amendment of 2026-11-09).
A pad without a stored trigger note plays the patch's pad start note plus its index (A1 is the
start note, bank B starts 16 above it); a stored note is absolute and never moves. A new patch's
start note is 36; a patch saved without one reads 60. Parameters and results are MIDI numbers;
refusals and notices name notes in the Middle C setting (ADR 0051), so a script must not parse
them.

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_pad_start` | `{track?, note: 0-64}` | Sets the pad start note, one undo step. Pads without a stored note move with it and the root of each zone bound to such a pad moves by the same distance, so no pad changes pitch; stored notes, their zones and unbound zones stay. No clip, take or automation changes. Answers `{track, note, from, semitones, notice}`, the notice naming how far to transpose pad clips (`Pad start note C1 (down 24 semitones): transpose pad clips by -24 to keep them on the same pads.`). An unchanged note records no step. Refused with nothing changed: `note must be an integer from 0 to 64`; a moved pad onto a stored pad's note (`Pad B1 plays C1: a start note of C1 would put pad A1 there.`); a moved bound pad that would start more than eight voices for one note with the unbound zones over it (`Pad A5 would be a ninth layer on E3.`); a moved pad's zone root outside 0 to 127 (`Pad A3's root would leave 0 to 127.`). |
| `mix.sampler_pad_set` | `{track?, pad: 1-64, note?, choke?, mute?, solo?, name?}` | A `note` (0 to 127, unique among the pads' notes) is stored and absolute, even when it equals the pad's default; `note: null` returns the pad to its default. A note edit on a bound pad is refused like the start note when it would be a ninth layer. Answers the pad's effective `note`. |
| `mix.sampler_state` | `{track?}` | Read-only: the patch as stored (`patch`), the normalized `seed`, `pad_start_note`, per pad `{pad, note, note_stored, zone, mute, solo, choke, name}` (`note` is the effective note), the loop's `root` and `root_stored` (`loop` is `null` without one), per zone `{zone, pad, keys, velocities, root, mode, membership, loop_mode, loop_used, crossfade, crossfade_used, fades, fades_used, slice, region, rate}` (stored values beside the ones playback uses; `region` is the region a slice-bound zone derives, `null` otherwise; `rate` is the zone source's native rate, for the crossfade in milliseconds) and per group `{group, round_robin, zones}`. |

## Sampler kit presets

A kit preset is the Sampler's whole patch (pads, zones, slices and both marker sets, groups and
round robin, every zone and pad setting, the pad start note) with each sample referenced by its
file path; no audio is copied ([ADR 0041](adr/0041-sampler-one-instrument-three-views.md),
amendment of 2026-11-11, #467). The file is `{"format": "transit-sampler-kit", "version": 1,
"kit": ..}`, pretty-printed JSON; `kit` is the session's patch serialization with each `sources`
entry carrying `file: {path, name, frames, channels, sample_rate, format}` in place of the
session's source id. A sample in or below the kit file's folder is written relative with `/`
(the Windows `\\?\` prefix removed first); any other is written absolute. On recall a relative
path is taken from the kit file's folder, never the session's; a path to another computer, a
device or a URL is refused and the sample is missing; a network share counts as another computer
unless the kit file is on that same share. Another system's absolute path is missing too: on
Windows a path with no drive (`/Users/me/Kick.wav`) is never taken onto the kit file's drive, and
on macOS and Linux a drive path is never read. Unknown fields at the top of the kit and in its
entries are kept through recall and a later save; unknown fields inside a `file` object are not.
The user's kits live in the settings folder under `presets/sampler/<name>.tkit`, where the
Sampler's title-bar preset browser lists, recalls, saves and deletes them; the commands take any
path.

Track presets carry a Sampler instrument's kit in the same portable form (#458):
`track.save_preset` writes `instrument.sampler_patch` with `file` entries (paths relative to the
`.tpreset` file under the same rules) and is refused, nothing written, for a sample with no file of
its own; no insert slot's Sampler patch is written (only the instrument slot plays one).
`track.load_preset` resolves, admits and decodes the samples as `mix.sampler_kit_load` does, once
per target track, in one undo step; a sample not found, changed or unreadable takes the missing
state and the result's `notice` (also the status line) names it; any refusal (a kit this build does
not read, a memory bound, eight voices per note) refuses the whole load with nothing applied. An
entry naming a session `source` id (a preset written before 0.7.1) becomes a new missing source
named `source <id>` with no path, never a source of the loading session, and `notice` says
`track preset "<name>": its Sampler samples were saved by session id and cannot be found: load
them again.` The preset's instrument and inserts go through the retire step a session load runs
(the retired Subtractive Synth, the inherited effects, the one-sample Sampler, the old note
mapping), with the same notes.

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_kit_save` | `{track?, path}` | Writes the track's kit to `path` with `.tkit` appended unless it already ends in `.tkit` (any case); another extension is never replaced (`808 v1.5` saves as `808 v1.5.tkit`). Not undoable, not journaled. Written as a session file is: to `<name>.tkit.tmp` beside it, flushed to disk and renamed over it; a failed write removes the temporary file and leaves an existing kit as it was. A sample missing at save time is written with its stored path and listed. Answers `{track, path, pads, zones, sources, relative, absolute, missing: [{zone, pad?, path}], notes}` (`notes` names samples in the session's `Audio Files/`, on which the kit then depends, and samples on another network share, which will be missing on recall). Refused with nothing written: an opaque patch (`this version cannot save the track's Sampler kit (it is kept as saved)`); a sample with no file of its own (`Pad A3's sample "Hit.wav" has no file yet: save the session, or load the sample onto the pad from a file.`); an existing file whose tag, version or kit schema this build does not read (`"808.tkit" was saved by a newer transit: Save As under another name`). |
| `mix.sampler_kit_load` | `{track?, path, replace?: false}` | Recalls a kit file onto the track's Sampler as one undo step ("Load Sampler Kit"), not journaled. The whole patch is replaced (stored pad notes as stored, the kit's pad start note); the slot's preset label becomes the file's stem; clips, automation and mappings are untouched. When the current patch has any zone or a loop, the command without `replace: true` changes nothing and answers `{track, kit, loaded: 0, replace_needed: true, pads, zones}` (the bound pads and the zones that would be replaced, numbered from 1). Each sample is admitted against the new kit (one source's and the kit's 128 MiB bounds, from the header before decoding) and decoded at its native rate. A sample that is not found, changed since the save (its rate or frame count differs from the kit's record) or unreadable takes the missing state: a session source with the path looked for, the kit's recorded facts and no audio; its zones and pads load as saved and play silence, and the rest of the kit loads. A referenced Sampler source already in the session at the same path, with its audio present and matching the record, is reused. Each referenced source of the replaced kit that nothing else names afterwards (no clip, freeze, Sampler patch or clipboard clip) leaves the session and the source selection; the file is never touched and undo brings it back. Answers `{track, kit, loaded, pads, zones, sources, missing: [{zone, pad?, path} or {loop: true, path}], notice}`; `notice` names missing and changed samples (the first 16) and a changed pad start note. Refused with nothing changed, answering `{track, kit, loaded: 0, notice}`: an opaque patch or a frozen track as the target; a file that is not a kit this build reads (another tag, `version` above 1, `kit.schema` above 1, an invalid patch); a memory bound; more than eight voices on one note (judged on the kit itself, not against the kit it replaces); an import worker that cannot start or fails (`Kit "808": import worker failed; nothing was loaded.`). |

## Sampler zones and groups

The Instrument view's commands ([ADR 0041](adr/0041-sampler-one-instrument-three-views.md),
amendments of 2026-11-08 and 2026-11-09). Each is one undo step, refused whole with the document
unchanged. Values are **checked when set, clamped when used**: an edit refuses an over-limit value
it sets, a later edit of something else is never refused because of it, and `mix.sampler_state`
reports the stored and the used value. Zones are numbered 1 to 256; groups, layers and
alternatives from 0. Refusals that name a note follow the Middle C setting (ADR 0051).

| Command | Params | What it does |
|---|---|---|
| `mix.sampler_zone_edit` | `{track?, zone, region?, playback?, pad?: null}` | As before, plus `region.crossfade` (source frames), `playback.membership` (`{group, layer, alternative}` or `null`) and `playback.fades` (`{key_lo, key_hi, vel_lo, vel_hi}`, widths 0 to 127). `pad: null` unbinds the zone's pad in the same step (Make group). Refuses: a loop on a one-shot, or One-shot on a looping zone, unless the same edit sets Gate or turns the loop off (`A one-shot cannot loop: choose Gate, or turn the loop off.`); a crossfade over half the effective loop (`Crossfade is at most 4.7 ms (226 frames) for this loop.`, milliseconds at the zone source's rate); a crossfade on a ping-pong loop; fade widths wider than the range on either axis, or any on a pad's zone; membership on a pad's zone without `pad: null`; a loop on a one-frame slice (`Slice 4 is one frame long, too short to loop.`); region or loop points on a slice-bound zone (its loop spans its slice, and setting its loop mode writes the slice's region and loop into the zone); an unbound zone's loop outside its region (a trim moves the loop in the same edit or is refused); a group shape fault; a ninth voice on a note. Answers the zone's state as `mix.sampler_state` reports it, with `track`. |
| `mix.sampler_zone_map` | `{track?, zones: [{zone, keys, velocities, root, membership, fades?, pad?: null}]}` | The key map's range edits and Make group in one step, admitted whole before anything is stored: a group has one key range, at most 8 layers, each layer 1 to 8 alternatives numbered from 0 without a gap and with one velocity range, no duplicate place, no pad's zone (`group 3 would have 9 layers (at most 8)`); at most eight voices start for one note, a bound pad counting one and a group layer one whatever its alternatives (`Zone 9 would be a ninth layer on C3.`). Answers the zones' states. |
| `mix.sampler_group_set` | `{track?, group: 0-255, round_robin: sequential\|random}` | The group's round-robin mode (Sequential is the default and is not stored). An unchanged mode records no step. A mode change reaches the playing instance without restarting its counters. |
| `mix.sampler_seed` | `{track?, seed}` | `seed` 0 to 4294967295; 0 is stored as 1 and the stored value is answered (`seed must be an integer from 0 to 4294967295 (0 is stored as 1)`). `mix.sampler_new_patch` takes `seed` the same way and answers it. A file whose seed is 0 opens unchanged and plays as 1. |

Every command that stores a patch refuses an edit that would start more than eight voices for one
note at some velocity where the patch did not already, so a loaded map over the bound still plays
and still takes other edits.

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

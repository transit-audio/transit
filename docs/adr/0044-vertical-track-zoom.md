# ADR 0044: Vertical track zoom: per-track heights in the session, resize gestures and lanes that follow

- **Status:** proposed 2026-11-04 (#314)
- **Date:** 2026-11-04
- **Reopen when:** real editing sessions need heights above 440 points, independent take-lane sizes, or a different bulk-resize rule; or saved lane proportions need to travel between machines. Treat recompilation for non-audio session edits as a separate future issue if its cost warrants changing the shared session/player path.

## Context

A Discord tester (2026-11-03, #314) will edit timing elsewhere until the arrangement has vertical
track/lane zoom. The request is a lower-header-edge drag, Alt-drag for every track, a wheel gesture,
keys for all rows, session persistence, Reset, proportional lanes and redrawn waveforms. Companion
#299 concerns translucent crossfades; it does not change this decision's geometry.

Evidence below is against `fb1aa595`; line numbers identify the inspected implementation:

- `crates/model/src/track.rs:74` already has `Custom(u16)`, seven named presets and `Fit(u16)`.
  `points` at line 99, `label` at 112, `from_id` at 125, `from_points` at 129 and `step` at 134
  define the ladder. `Track.height` at 234 is serialized; new tracks use Medium at 318.
- `crates/engine/src/cmd/track.rs:63` registers undoable `track.height`; its handler at 686 accepts
  points or a preset and returns `{"height": h}`. `track.height_up/down` at 72 step selected tracks.
  `crates/engine/src/cmd/zoom_toggle.rs:9` registers `view.zoom_toggle` as `noundo`, although its
  handler writes selected tracks' Fit heights and a restore snapshot. `cmd/view.rs` zoom commands
  change the horizontal scale, not track height.
- `crates/ui-studio/src/arrange.rs:338` ignores those values and draws one
  `app.ui.track_height.clamp(24.0, 200.0)`. `ui_state.rs:37` stores that machine preference;
  line 143 defaults it to 46. No UI control changes it. `arrange.rs:295` has `RowGeo` with one `rh`,
  total row heights and cumulative tops. Take lanes use `TAKE_H = 22` at line 291.
- `crates/ui-studio/src/automation.rs:47` defines 56-point automation lanes, bounded to 40..240;
  `LaneView.height` at 71 is view state. Layout at 637 and 673 and the bottom handle at 1051
  use that unscaled height; `AutoGesture::Height` at 259 owns the lane drag. `auto_shown` and
  `auto_lanes` have `#[serde(skip)]` at `ui_state.rs:124`: their bases are transient, not in ui.json.
- `crates/ui-studio/src/arrange/clips.rs:162` already bounds the name strip by the clip rect and
  passes the remaining body to `waveform::draw` at 191. `waveform.rs:50` divides that body's height
  by channel count; line 155 derives peak amplitude from the band height. Peaks do not assume 46
  points. The fixed parent row geometry and header controls are the missing connection.
- Height commands currently also trigger audio-plan work: `crates/engine/src/lib.rs:391` advances
  the revision and changes the document Arc; `crates/ui-studio/src/host/plugin_state.rs:21` checks
  both and calls `Player::update_session` at line 36. `crates/playback/src/player/runtime.rs:124`
  requests a plan, `crates/playback/src/player/mod.rs:263` takes that request, and
  `crates/playback/src/plan_bridge.rs:450` calls `compile_with_mode`. Undo merging alone does not
  prevent this. Previewing locally and committing once limits a gesture to one session edit and
  its normal plan compilation, the same cost as a track rename today. This ADR leaves the
  session/player synchronization and capture/save path intact.

## Convention

transit follows what Ableton Live and Logic Pro agree on unless it can do clearly better.
Both provide individual track sizing, bulk sizing and project-specific track layouts. Their input
bindings differ; these are the concrete references, checked 2026-11-04:

- [Live 12, Arrangement View, Editing and Navigation](https://www.ableton.com/en/manual/arrangement-view/)
  describes dragging a track's lower split line and Alt/Option-resizing all tracks.
  [Live 11, Arrangement View](https://www.ableton.com/en/live-manual/11/arrangement-view/)
  explicitly documents Alt/Option-wheel in the track display for a selected track.
- [Logic Pro, Zoom tracks](https://support.apple.com/en-asia/guide/logicpro/lgcpc79f5bcd/10.7/mac/11.0)
  documents bottom-left dragging, **Command**-drag for all tracks, Shift-drag to reset all,
  and individual zoom/reset commands. Thus the issue's Option-drag attribution is not the
  modifier documented by this Logic guide. The agreement is on individual and all-track scope,
  not on Option versus Command. transit chooses Live's Alt/Option bulk modifier as requested.
- [Live 12, Keyboard Shortcuts](https://www.ableton.com/en/live-manual/12/live-keyboard-shortcuts/)
  uses Alt/Option plus/minus for selected track heights. transit uses that key family with
  **all-track** scope for #314; the scope is an explicit transit choice. Reset uses Logic's
  all-track reset concept, exposed as a menu command without another key.
- [Logic Pro, Track header controls](https://support.apple.com/en-hk/guide/logicpro/lgcpe53c68af/10.7/mac/11.0)
  documents configurable header controls. Live's Arrangement View reference also hides fade
  controls when tracks are too small. transit uses compact headers; the exact automatic hiding
  thresholds and proportional lane formulas below are transit decisions.

## Decision

1. **One stored track height.** The arrangement draws `H = Track.height.points()` in logical
   points, independent of display DPI and interface zoom, except for the temporary UI preview
   in rule 5. Retire `UiState.track_height`, including its serialization. An old `ui.json` key
   is ignored by serde, never imported into a session,
   and disappears on the next settings save. The same session opens identically with that key
   absent, 46, 200 or any other value. Automation lane bases remain transient view state of the
   open session: retain `auto_shown`/`auto_lanes`' skip attributes; save neither in ui.json nor in
   the session. Closing and reopening the session resets those bases to 56 when lanes are opened.

   New tracks and **Reset Track Heights** use `Small` (46 points), preserving today's new-project
   density. Keep the existing meaning of `Medium`: **88 points**. A session from a build that
   never set heights therefore opens at 88 instead of the former 46; other stored presets and
   custom values likewise become visible. This is an intentional visual compatibility change,
   to be called out in release notes. We cannot distinguish an untouched Medium from an intentional
   `track.height` command. Do not guess or rewrite either. A missing height deserializes as Medium
   through an explicit serde default; creating a new track explicitly chooses Small. Loading
   heights alone does not mark the session modified. Reset is the reversible route to 46.
   [ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md)'s break-now rationale supports
   retiring the unused preference in alpha, not deleting stored session choices.

2. **Keep the ladder and bound every read.** Presets remain Micro 16, Mini 24, Small 46, Medium 88,
   Large 132, Jumbo 220 and Extreme 440 points. Keep `Custom(u16)` and `Fit(u16)` with their existing
   serde spellings. `points()` clamps both payload variants to **16..440 inclusive**, including Fit;
   440 is the hard maximum of the main track row, not of its whole stack of lanes. The current
   Fit escape to 65535 points ends. A file containing `Fit(65535)` still loads and retains that
   payload on an unedited save, but draws at 440. Numeric setters clamp before rounding to the
   nearest whole point (positive halves up); exact preset points use the named variant, other
   values use Custom. Reject nonnumeric/nonfinite command inputs. No 24..200 UI clamp remains.
   `label`, case-insensitive `from_id`, and strict next-higher/next-lower `step` remain; stepping
   at an endpoint is a no-op. The UI need not expose a seven-item preset picker in this issue.

   Keep `view.zoom_toggle {width_px?, height_px?}` and its `{"on": boolean}` response. It keeps
   its selected, non-hidden-track scope and horizontal zoom behavior; its computed Fit heights
   now clamp to 16..440. It fits main rows only; open lanes can make the stack exceed the viewport.
   Toggling off restores the saved height variants for surviving IDs, never creates deleted tracks.
   Remove `noundo`: each toggle is now one ordinary undoable session edit because those heights
   are visible and persistent. An actual explicit height change clears the entire pending toggle
   restore record in the same undo step, leaving other tracks and horizontal zoom as they stand;
   the next toggle starts afresh. Undo restores that record too. A height no-op does not clear it.

3. **Drag one or all tracks.** A 4-point strip inside the bottom of each main track header is the
   resize handle, above any take or automation lanes. It owns primary-button drags ahead of track
   reorder, selection and header controls, shows a vertical resize cursor and reports the current
   point height. Plain drag targets only that track, regardless of selection. Capture the track ID,
   starting height, pointer y and modifier at press; preview
   `height = start_height + current_y - press_y`, clamped and rounded by rule 2. Use absolute
   displacement, not accumulated rounded frame deltas. On release, even outside the header,
   commit once through `track.height {"tracks": [id], "height": points}`; loss of window focus
   commits the last preview. This follows the single-track convention above and rule 5.

   Alt-drag (Option on macOS) captures **every session track ID** at press, ignores selection,
   and commits the same command with that full list. Each previews the dragged track's resulting
   height; unequal starting heights become equal on the first effective movement. A press/release
   with no effective movement changes nothing. Returning the anchor to its starting effective
   height restores the captured per-track preview heights and commits nothing.
   This is the agreed all-track operation using Live's modifier, not a selected-track shortcut.
   One undo restores every original height, including unequal variants.
   Changing modifiers after press does not change scope. An unrelated command ends the drag first;
   do not resume it until another press. No gesture writes the model directly.

4. **Wheel, keys and Reset.** Alt alone plus vertical wheel over a visible track's main header or
   main clip row changes **that hovered track only**, without selecting it. Wheel-up increases
   by 8 points per notch; down decreases by 8. This adopts Live's wheel gesture with an explicit
   hovered-track target to avoid changing a remote selection. Each notch updates the local preview
   from its current clamped height plus the delta; rule 5 commits the final height once.
   Read raw wheel events: one line unit is one notch; for point-unit devices accumulate
   40 logical points per notch, retaining signed fractional
   remainder until the gesture ends; page units count as one notch per signed page. Process
   each event's whole-notch total in order, discarding outward excess at a clamp. Bound the
   accumulator and use saturating arithmetic rather than looping once per notch for a hostile
   large input. Consume this event before
   ordinary scrolling. Do not use smoothed deltas that continue after release. Wheel over a
   child lane, ruler, empty area or covered/modal area does not start track zoom. Alt with Ctrl,
   Command or Shift does not match; Ctrl-wheel/pinch and Shift-wheel keep their existing behavior
   (`arrange.rs:1103`). No new Alt-pinch behavior in this slice.

   In Arrangement scope, **Alt+=** (unshifted equals) and **Alt+-** call new undoable
   `track.height_zoom {"direction":"in"}` and `{"direction":"out"}`. Each steps every session
   track independently to the next preset via `step`; it does not equalize them. Also accept
   Alt+Plus with no Shift for keyboards that emit Plus directly. Do not bind Alt+Shift+= or
   Alt+Shift+Plus: the inherited `view.nudge` spec reserves Alt+Shift+=/- (`cmd/view.rs:97`).
   Inspection of all `cmd!` specs and `shortcuts.rs` finds Alt+=, Alt+Plus without Shift and
   Alt+- free. Plain +/- are horizontal zoom (`shortcuts.rs:365`); Cmd/Ctrl +/- scale the interface
   (253); Alt+Up/Down reorder tracks (371); Ctrl+Up/Down are inherited `track.height_up/down`
   command metadata (`cmd/track.rs:72`). Leave those command APIs intact. Add exact-modifier
   `TABLE` rows, with normal text-entry/dialog/piano-roll focus exclusion and no raw key polling.
   A physical press is one action; ignore key-repeat events until release.

   View > Track Height has Increase All, Decrease All and **Reset Track Heights**. Reset invokes
   new undoable `track.height_reset {}` and sets every track to Small, with no new shortcut.
   The menu must pass the same direction values as the keys. Reset changes no lane base heights,
   selection or horizontal zoom. It follows Logic's all-track reset concept; the menu and 46-point
   target are transit choices. One key press or menu invocation is one undo step, except a no-op.

5. **Gestures preview locally and commit once.** A drag or wheel gesture captures its target IDs,
   original variants and effective heights in transient UI gesture state. During the gesture,
   paint and hit tests use its bounded preview heights; no height command runs, and neither
   session heights nor the pending zoom-toggle restore record changes. Preview state is never
   serialized. A drag commits on release or focus loss. A wheel gesture commits when the pointer
   leaves that track's main header/row, Alt is released, focus is lost, or a modal opens. Idle
   frames do not end it; there is no elapsed-time threshold. Both gestures also commit before
   any other command is dispatched, including undo, save, selection, horizontal zoom, another
   gesture, automation/MIDI commands and commands from the control channel. Query/inspection
   requests only observe and do not split a gesture.

   At that boundary, remove the pending gesture from UI state, then call ordinary
   `Engine::execute("track.height", {"tracks": [captured IDs], "height": final_points})` once.
   Clearing it first prevents recursive flushing. An unchanged preview commits nothing: compare
   effective heights against the captured starting heights, preserving the original variants.
   A refused commit clears the preview, displays the normal command error and redraws stored
   heights; do not retry it. Session replacement, reset or reopen overrides every commit boundary:
   drop the preview before that lifecycle action, commit nothing, and never apply captured IDs
   to a different document. Use the session lifecycle boundary, not an Arc/revision change alone,
   to identify replacement. Reset Track Heights is an ordinary command, not session replacement.

   One changed gesture is one command, one undo step and one plan compile through the existing
   session path, the same cost as a track rename. A key press or menu action is one command each.
   Undo restores the pre-gesture variants and pending zoom-toggle record; redo restores the result.
   Heights use neither `execute_merged` nor merge keys. Leave `crates/playback` and the merge
   machinery in `crates/engine/src/lib.rs` untouched; the UI idle-frame `end_merge` needs no height
   owner. There is no special refusal during automation write or a MIDI Learn sweep: ordinary
   `execute` pushes its own undo snapshot and clears the active merge key on a document change
   (`crates/engine/src/lib.rs:681`, 686), retaining `merge_hold`. The next merged automation/MIDI edit
   starts a new undo group under its existing key (`lib.rs:748`, 806); the pass/sweep continues.
   A no-op or refusal leaves that group intact, as any ordinary command does today.

6. **Lanes follow their own parent.** Let `D = 46` and `H` be the parent's bounded main height,
   using the preview while rule 5 is active and the stored height otherwise.
   An automation lane's displayed height is `clamp(B * H / D, 40, 240)` points, where `B` is its
   `LaneView.height` base (56 for a new lane). A take lane is `clamp(22 * H / D, 22, 220)`.
   Keep fractional points for layout; do not round at each zoom. This replaces ADR 0039's fixed
   take-lane-height rule when implemented. Collapsing/reopening lanes does not change their bases.

   The existing automation bottom handle stays: if its displayed height at press is `L`, dragging
   by `dy` computes `Lnew = clamp(L + dy, 40, 240)` and stores `B = Lnew * D / H` as view state.
   Thus the dragged lane follows later parent changes from the user's new proportion. Do not
   clamp that base back to 40..240: at H=16 a 240-point lane needs B=690. Validate in-memory bases
   as finite positive numbers bounded to `40*46/440 .. 240*46/16`, substituting 56 for invalid
   nonpositive/nonfinite values. Existing 40..240 bases remain valid. Lane dragging still makes
   no session undo entry and never invokes `track.height`. Take lanes gain no separate handle.

   Folder heights affect the folder's own row and lanes only; children use their own heights.
   All-track commands include hidden tracks, collapsed descendants, frozen/inactive tracks and
   any Master track in `Session.tracks`; each ID occurs once. Individual gestures only address
   visible rows. The mixer master strip is not an arrangement row and does not resize. Resizing
   never expands folders, reveals hidden tracks, changes routing, or changes take/automation data.

7. **One geometry for paint and hit tests.** Replace `RowGeo.rh` with per-track main heights,
   per-lane heights and cumulative tops. The main header rect ends at H, not at the full stack
   height as it does in `arrange.rs:383`; each child lane owns its separate header. All consumers
   use the same geometry: headers, clips, takes, automation,
   drag previews/drop targeting, selection rectangles, overlays, recording previews and scroll
   bounds. Rebuild geometry on every preview change, cancellation or committed height command
   before the next rendered frame/hit test; clamp scroll against the new total.
   Keep the dragged/hovered main row's top at the same screen
   y by adjusting scroll as earlier rows change, except where scroll bounds prevent it.

   At H < 24 show the track color, clipped name, folder disclosure and resize handle; hide all
   other header controls. At 24 <= H < 46 add mute, solo, arm and monitor where applicable in
   a single line; hide subtitle, routing/MIDI/rack chips, volume/pan, freeze and take buttons.
   At H >= 46 show the existing controls; extra height expands the content and meter, not fonts
   or button sizes. Hidden controls have no active hit boxes and remain available in the track
   menu or mixer. Retain armed/monitor/unresponsive state in the compact name tooltip and a
   compact status indicator; a small row must not conceal that it is recording or unresponsive.

   Clip names use a strip no taller than the clip; at H < 24 draw only that name/color strip,
   with no waveform, MIDI preview, loop glyph, fade handle or chip. At H >= 24 keep the bounded
   name strip and draw content only when its body is taller than 4 points; all controls are clipped
   to the clip and viewport. The same bounds rule applies to take clips. Normal, looped, ghost
   and recording waveforms use the new body rect and redraw cached peaks at its new amplitude
   scale, preserving channel separation and horizontal sampling. Height changes must not decode
   audio again. #299's crossfade paint uses these rects too. No audio gain, clip timing, DSP,
   recording buffer, callback or plug-in behavior changes.

8. **Format and observable command contract.** No new session field or serde variant is needed;
   `format_version` stays 1 and no `needs` entry is added. An older reader already understands
   these variants and retains them even if its arrangement ignores them. Keep variants rather
   than adding a `legacy.rs` removal pre-pass. Follow the field and corpus rules in
   [session-format.md](../contributing/knowledge/session-format.md); old fixtures are not regenerated.

   Preserve `track.height {tracks?, height: number | preset-name}` and its response
   `{"height": <TrackHeight serde value>}`, for example `{"height":"Small"}` or
   `{"height":{"Custom":57}}`. Missing `tracks` still means the existing selection; the UI
   sends explicit captured IDs for drag/wheel. Resolve/validate the entire request before editing;
   one invalid ID or value fails atomically. Numeric requests whose clamped points equal a track's
   current effective points preserve its stored variant (including Fit); the response still reports
   the normalized requested height. Thus outward wheel/drag input at a clamp cannot accidentally
   convert Fit to Extreme or clear its restore record. Explicit preset requests and Reset may
   replace an equal-size Custom/Fit with that preset; this is an actual stored change and is
   undoable. Avoid a mutable session borrow when nothing changes.
   Keep `track.height_up/down {tracks?}` returning `{"changed": n}`. The two new commands
   `track.height_zoom {direction:"in"|"out"}` and `track.height_reset {}` also return
   `{"changed": n}`, counting tracks whose stored heights change. Invalid direction fails;
   an empty session returns zero. All use the same clamp, no-op and toggle-invalidation rules.

   Add read-only `query.track_heights {}` returning
   `{"tracks":[{"id":1,"height":"Small","points":46}]}` in session order, including hidden
   tracks. Extend existing `ui.inspect` arrangement rows with `height`, `height_handle` and
   `stack_height`; existing row rects plus take/automation lane rects report the actual rendered
   geometry. Missing/hidden handles are absent, not stale rects. Tests must inspect paint and
   command state together: a correct query alone cannot prove the arrangement stopped using
   the global preference. During a gesture the query reports committed session values, while
   `ui.inspect` reports preview geometry; after commit they agree.

## Implementation / slices

Two jobs; acceptance of this ADR does not mean either is implemented.

1. **Model, commands and format.** Touch `crates/model/src/track.rs`, new model height tests,
   `crates/model/tests/corpus.rs` and `serde_defaults.rs` as needed for missing-field coverage;
   add a separate fixture only if existing corpus coverage lacks a named preset. Preserve old
   corpus bytes. Pure-move height specs/handlers/tests out of `crates/engine/src/cmd/track.rs`
   into new `cmd/track_height.rs` first; register it in `cmd/mod.rs`. Put the new commands and
   query there, with tests in a separate height test file. Update `cmd/zoom_toggle.rs` for the
   bounded, undoable toggle and invalidation contract. Gate: model round-trip/corpus/defaults,
   engine height/clamp/atomic-refusal/no-op/toggle tests, including ordinary-command behavior
   during automation/MIDI merge holds. The UI still draws its old height until slice 2; do not
   claim #314 complete at this boundary.
2. **Geometry, gestures and drawing.** Pure-move `RowGeo` and related layout helpers from
   `crates/ui-studio/src/arrange.rs` to new `arrange/geometry.rs` before changing behavior. New
   `arrange/track_height.rs` owns height input, wheel accumulation and gesture state; tests live
   in `arrange/track_height_tests.rs`, registered from that module. Pure-move frame/command-dispatch
   lifecycle code from `ui-studio/src/lib.rs` into `frame_logic.rs` before adding preview commit/drop
   hooks at UI/control dispatch and session replacement/reset/reopen boundaries.
   Keep only delegation/module wiring in the old files: `arrange.rs` starts at 1137 lines and
   `lib.rs` at 1249, and neither may grow toward or cross 1500. Extract automation lane sizing
   into `automation/lane_height.rs` before extending `automation.rs`. Update `arrange/tracks.rs`,
   `arrange/clips.rs`, `takes.rs` and its lane modules, `ui_state.rs`, `shortcuts.rs`, menu/control
   dispatch and inspect wiring as needed. Add menu entries in `crates/engine/catalog/menus.txt`
   and command registration for those entries; add resize help through the existing help catalog.
   Keep new behavior out of the large root files and keep every new file below 1500 lines.
   Gate: the complete UI gate below, including preview isolation and command counts through real
   handlers, shortcut reachability/shadowing guards, and settings-load tests. Slice 2 depends on
   slice 1's command and format contract. Neither slice changes playback or engine merge machinery.

For implementation, each slice runs its affected crate tests and the format/layer/timing/file-size
gates; the final chain runs CONTRIBUTING.md's full local gate and Linux compile proof, with the
fresh-context refutation review of ADR 0009. These are future implementation gates; this authoring
round is docs-only and runs no Cargo commands.

## Gate

Use real UI handlers with injected pointer, wheel and key events and explicit frame times; no
wall-clock, scheduling, sleeps or frame-rate assertions. Tests fail on the old uniform-row code.
Follow [gates.md](../contributing/evidence/gates.md); any fixture rendering four or more strips uses
`MixEngine::force_serial(true)` unless it is testing workers. Required cases:

- Drag one of three unequal tracks over N simulated frames: paint/inspect shows its preview,
  but session/query heights, dirty state, undo stack and pending zoom-toggle record remain
  unchanged before release. Assert zero `track.height` journal entries before release and exactly
  one afterward, with the final value and one undo step. One undo restores the exact original
  variant and toggle record; redo restores the result. Use command history/undo observations,
  not playback-runtime counters. A stationary drag and a drag returning to its start commit
  nothing. Release outside and focus loss commit the last preview once.
- Replace, reset and reopen the session mid-drag and mid-wheel in separate cases: discard the
  preview and commit nothing, even when the new session reuses the captured track IDs. No preview
  is serialized. An intervening ordinary command, including a control-channel command, commits
  once before that command; a read-only query does not. A refused commit drops the preview,
  preserves stored heights and reports its error without retrying.
- Alt-drag with a nonempty selection changes all tracks to the anchor height, including a hidden
  child, collapsed folder, frozen track and Master. Undo restores their different originals;
  reopening the folder shows the stored result. Before release only preview geometry changes;
  exactly one command commits all targets. Returning to the anchor's start restores the unequal
  preview heights and commits nothing. Plain folder resizing leaves children unchanged.
- Alt-wheel over an unselected row changes only it, by 8 points per notch. Several events with
  idle frames between remain local: k notches commit exactly one `track.height` with the final
  height at the boundary, then one undo restores the original variant. Inject pointer exit,
  Alt release, focus loss, modal opening and another command separately: each commits once,
  and the next qualifying notch starts a new gesture. A net-zero wheel preview commits nothing.
  Test line and point-unit remainder, positive/negative deltas, clamping and direction reversal;
  a child lane does not capture it.
  Ctrl-wheel/pinch, Shift-wheel and modal capture retain their original effects.
- Keys and all three menu entries reach the documented commands; two presses produce two steps,
  auto-repeat does not. Text editing and piano roll do not resize tracks. Verify exact modifier
  matching and the Alt+Shift nudge reservation. During automation write and a MIDI Learn sweep,
  a changed height command creates its own step, splits the existing merge group, retains the
  hold and lets the pass/sweep continue in a new group; a no-op does not split it.
- Drag, wheel, numeric command, preset stepping and Fit reads respect 16 and 440; values beyond
  both ends clamp. No-op endpoints add no undo; malformed values and missing IDs change nothing.
  Toggle on/off restores heights, and resizing while toggled then undoing obeys rule 2.
- At H=46, default automation/take lanes are 56/22; at H=92 they are 112/44. Test both lane clamps
  and a hand-resized automation lane followed by parent resize, collapse/reopen and parent undo.
  Compare cumulative tops, lane rects, drop targeting and scroll bounds during preview, after
  commit and after cancellation, not only a formula helper.
  Lane collapse/reopen within the open session retains bases; session close/reopen resets them.
  Verify neither settings nor session serialization contains the transient lane bases.
- Save/reload unequal presets, Custom and Fit through file, bytes and bundle open paths. Load an
  existing named-preset corpus session and assert both its retained variant and its rendered points
  (Medium=88); test a missing height. No new `needs`, migration note or dirty flag solely for heights.
  Open the same session with/without the retired `ui.json` key: identical heights, geometry and
  session serialization. Settings saves omit the key.
- Paint at 16, 24, 46, 88 and 440: labels/control hit boxes stay inside their rows, hidden controls
  cannot fire, status remains discoverable, and waveform peak extents follow each body's height.
  Cover mono/stereo, looped/ordinary/take clips and live recording previews without audio hardware.
  Reset restores every main row to 46 while retaining lane bases and horizontal zoom.

## Alternatives considered

- **Per-track heights as view state or in `ui.json`.** Loses the editing layout when a project
  closes or moves machines and duplicates the existing session field. Automation lane bases
  remain transient open-session view state with `#[serde(skip)]`; they are not saved in ui.json.
- **One global row height.** Cannot enlarge a timing-critical track while keeping others compact.
- **Live-style per-track fold toggle only.** Useful for overview, but cannot provide the requested
  continuous resize or lane detail. Micro supplies compact rows without another fold state.
- **Renumber Medium to 46 or migrate every Medium to Small.** Preserves the common old appearance
  but reinterprets intentional stored values and command clients. Keep the ladder's meaning and
  disclose the visible change, with an undoable Reset.
- **Retire the ladder and Fit during alpha.** Possible under ADR 0040, but requires a format
  pre-pass and loses existing scripting/toggle behavior without helping the resize implementation.
- **Timer-based wheel commits.** Make undo depend on pauses and test scheduling. Explicit input
  boundaries are predictable and deterministic.
- **Issue merged height commands every frame.** Undo merging still requests plan compilation
  per edit. A local preview avoids that cost until the one final command.
- **Add a layout-only player snapshot rebind.** A playback-runtime mechanism is outside this UI
  feature. Keep ordinary synchronization/capture/save and consider non-audio edit recompilation
  separately under Reopen when.

## Judging round 1 responses

Historical responses retained below; the round-3 coordinator direction supersedes C1's replacement.

- **C1 — Accepted (revised).** Rule 9 specifies verified control-side snapshot rebinding, save/capture and pending-request lineage, with module allocation in slice 1 and deterministic request/compile/publication gates through commands and UI gestures in both slices.
- **M1 — Accepted (revised).** Context, rules 1/6, alternatives and the gate now state that automation lane bases are transient open-session state; retain the serde skips and test close/reopen and serialization behavior.

## Judging round 2 responses / coordinator direction

- **C1 / coordinator direction — Accepted (revised).** Round 2 closed C1 with the round-1 replacement; the coordinator supersedes that replacement here. Removed decision 9 and its playback/model comparison, rebind modules and runtime counter gates. Rule 5 now previews drag/wheel heights locally and commits once through ordinary `track.height`, with explicit commit, no-op, refusal and session-replacement boundaries; the gate checks preview isolation and command/history counts.
- **Merge ownership consequence — Accepted (revised).** Removed height-specific merge ownership/refusal and `execute_merged`; ordinary height commands split an active automation/MIDI undo group without ending the pass or clearing its hold. Engine merge machinery stays unchanged.
- **M1 — Accepted (retained).** Round 2 closed the lane-persistence finding; transient bases, serde skips and close/reopen/serialization gates remain unchanged.
- **Scope and documentation — Accepted (revised).** Removed playback work from both slices and synchronized the index and knowledge pages; non-audio edit recompilation is a separate future concern under Reopen when. All other round-2 decisions remain; there were no new minor findings.

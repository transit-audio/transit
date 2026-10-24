# UI

`crates/ui-studio` is transit's UI. `crates/ui-egui` is the legacy upstream UI; do not extend it.
UI code follows the same no-panic rule (`#![deny(clippy::unwrap_used, ...)]` in `lib.rs`).

## Conventions

- **Shortcuts:** one table, `TABLE` in `crates/ui-studio/src/shortcuts.rs`. Handlers take their keys
  from the table, never from raw key reads. Tests enforce it:
  `every_handler_takes_its_keys_from_the_table` and
  `every_table_row_is_reachable_and_no_looser_row_shadows_it` in
  `crates/ui-studio/src/slice5_tests.rs`. Menus and Help > Keyboard Shortcuts render from the table.
- **Icons:** Lucide icons through `crates/ui-studio/src/icons.rs`, colored by theme token. The glyph
  guard `symbols_are_icons_not_text_glyphs` fails the build on a symbol typed as text.
- **Theme tokens:** every color comes from `TOKENS` in `crates/ui-studio/src/theme.rs` (dark and
  light sets, flat style). No literal colors in widgets.
- **Commands:** the UI calls `engine.execute` (`commands.md`); it does not edit the session
  directly.
- **Device lifetime:** `crates/ui-studio/src/audio_connection.rs` is the boundary; tests use
  `FakeConnection` to script devices without hardware (`audio_devices_tests.rs`).
- **Long work** (plug-in scan, import) runs off the UI thread. Known gap: a heavy plug-in create can
  still block it (`../../known-issues/README.md`).
- **Transport wait:** Play draws muted and pulses, with the reason as its tooltip, while a Play request is pending or a plug-in is still loading (`transport_wait()` in `transport.rs`, derived from `player_requests.play` and `Player::any_loading`). Clicks while pending are ignored; Stop cancels. The pulse reads as "about to start", so it stops (steady muted) while the wait is on a plug-in that is slow to load or not responding, and the tooltip names it ("Waiting for X to load (slow)" / "X not responding: Retry in the mixer").
- Known issue: `audio_devices_io.rs` reads Enter directly (`key_pressed`) in the rename field; it predates the shortcut gate's scan list and should move to a table row.
- **Loop handle** (`arrange.rs`, ADR 0019): drag the header's upper right. Modifiers: Shift no snap, Alt independent copies (Repeat), Ctrl/Cmd overwrite. Without Ctrl/Cmd a longer loop and its ghosts stop at the next clip; with it the clips under the new span are trimmed or removed (shaded while dragging; an edit-locked clip still stops it) in the same `clip.loop_to` undo step (`overwrite: true`). Shift is already no snap, so overwrite is on Ctrl/Cmd.
- **Confirmations:** an action that destroys something the user cannot undo (deleting a plug-in
  preset) asks through `Services::confirm`, a `Fn(&str) -> bool` in the `Services` struct that the
  app injects (`crates/ui-studio/src/lib.rs`; the app's is a yes/no `rfd::MessageDialog` in
  `apps/transit/src/main.rs`). With `confirm` unset the action is refused with a status line, so
  nothing runs unasked. Tests supply a stub that records the question and answers
  (`plugin_header_tests.rs`); a test never opens a real dialog.
- Strings are American English.

## Clip fades and crossfades

Issue #63; code in `crates/ui-studio/src/fades.rs` (gestures) and `crates/model/src/clip.rs` (data).

- **Data:** an audio `Clip` has `fade_in` and `fade_out`, each a `Fade { len, shape, curvature }`.
  `len` is samples, clamped to the clip's length; `shape` is Linear, Equal Power (the default), S-Curve,
  Exponential or Logarithmic; `curvature` is a bend from -1 to 1, and zero keeps the shape exact.
  A fade of length 0 is no fade.
- **Gain:** `Fade::gain` is the one envelope the engine renders and the arranger draws; a fade-out
  uses `gain(1 - t)`. There is no separate crossfade object.
- **Crossfade:** a range across an abutting boundary (`ops::create_fades`, `edit.fades_create`) sets
  the earlier clip's fade-out and the later clip's fade-in over the overlap, extending each clip
  into the other's side by its unused source handle. Removing the fades trims overlapping
  neighbors back to the midpoint.
- **Gestures** (one undo step each): drag a top-corner handle inward for the length (Shift: no snap),
  Alt-drag to bend the curve, click for the shape menu, double-click to reset, drag to zero to
  remove. Dragging a fade across an abutting neighbor creates a crossfade; crossfade edges adjust
  both sides. The automatic crossfade preference is #72.
- Looped clips: fades follow the arrangement bounds, not each cycle ([ADR 0019](../../adr/0019-clip-internal-loops.md)).

## Tests

`crates/ui-studio/src/*_tests.rs` and `crates/ui-studio/tests/`. A UI feature needs a test through
the real handler, not only a unit test of a helper.

## Cycle and the loop region

Turning Cycle on (`options.loop_playback`, from the transport button, the menu or the shortcut) with
no loop region makes one, in the same undo step as the switch: the time selection when it is
non-empty, else four bars from the bar containing the playhead (the UI passes `at`), cut at the end
of the content when that lies ahead. Turning Cycle off leaves `loop_range`, so the next toggle
reuses it. `EditState::cycle_range()` still falls back to the selection for a session that arrives
with Cycle on and no region; playback reads it unchanged.

The ruler shows the stored `loop_range` (never the selection): an amber block while Cycle is on, a
faint outline while off, nothing without a region. The block is live (`loopbar::block`, sharing
`loopbar::gesture` with the 8 px strip): edges resize, the body moves, double-click toggles Cycle,
all as one `transport.loop`. A plain click on the block, or anywhere else on the ruler, locates.

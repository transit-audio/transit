# Hosted plug-in windows and presets

Issue #68. Read with [ADR 0018](../../adr/0018-plugin-host-thread.md) (the plug-in host thread).

## The editor window

- On Windows both format hosts open editors in `transit_rt::editor_window`: a top-level window
  on the plug-in host thread with a header strip (GDI, no egui: that thread has no egui context)
  above a content container the plug-in parents its view in. CLAP editors open embedded there when
  the plug-in offers it, so the strip shows; floating CLAP editors and macOS windows have none.
- The client area is the plug-in's reported size plus the strip, on open and on every resize
  request (`resize_content`). `EditorWindow::sync`, run from the editor's `idle`, follows the
  plug-in's child window when it settles at another size (`Follow`).
- The strip never takes focus; activating the window gives focus back to the plug-in.
- Scaling (issue #101). Sizes are physical pixels and the window applies no transform of its own.
  The format hosts pass the window's scale (`EditorWindow::scale`, DPI / 96) to plug-ins that
  scale on request, then size the window from what the plug-in reports. VST3
  (`vst3-host` `attach_view`): `IPlugViewContentScaleSupport::setContentScaleFactor` after
  `setFrame` and before `attached`, then `getSize`. CLAP embedded (`clap-host`
  `GuiLink::open_embedded`): the window is made right after `create`, then `set_scale`,
  `get_size`, `set_parent`, `show` (gui.h's order). When the window moves to a monitor with other
  scaling, `WM_DPICHANGED` keeps the plug-in's pixel size and marks the change
  (`take_scale_change`); the editor's `idle` sends the new factor (`follow_scale`), and the
  plug-in answers through `resizeView` / `request_resize` (the host also re-reads the size). A
  VST3 view without the interface gets no call, and a CLAP GUI that returns false from
  `set_scale` scales itself: both keep the size they report, so a self-scaling plug-in (Arturia's)
  is never scaled twice. Floating CLAP windows get no `set_scale` (the plug-in owns the window;
  the old `set_scale(1.0)` forced 100%). macOS gets no call: views work in points there.
- VST3 scale sequence (issue #105), per editor open: `setFrame`; `setContentScaleFactor` before
  `attached` (the window takes the `getSize` that follows); `attached`; the same factor again
  right after `attached` (some plug-ins apply it only to an attached view); on a DPI change, the
  new factor from `idle`. A `resizeView` is honored at any time after `attached` (the window
  takes the requested size, then the view's `onSize`); one that arrives before `attached` is
  kept and applied right after it. `canResize` and `checkSizeConstraint` apply to sizes the host
  or the user propose, not to the plug-in's own `resizeView`. Each scale call (interface found or
  not, factor, `tresult`, `getSize` before and after) and each `resizeView` (size, honored or
  not) is logged at info level with the plug-in's name.

## Presets

- A user preset is the plug-in's full state, the same state a session stores. Files:
  `.vstpreset` (Steinberg's container, `transit_vst3_host::preset`) and `.clappreset`
  (transit's documented container, `transit_clap_host::preset`).
- Folder: `<settings>/presets/<plug-in id made file-safe>/` (`transit_engine::presets`); file
  access goes through `transit_engine::io::presets` (checked roots).
- State reads and live loads are `StateJob`s on the host thread, polled once per frame
  (`PluginStateReader::begin_save`/`begin_load`); the UI side is `ui-studio/src/plugin_header.rs`.
- Each job (and each restore after an undo or redo) starts only once the audio side plays the
  current document (`Player::plays`: its plan confirmed and rendered), so the plug-in has every
  parameter value the session stores. Earlier, a read missed a value still on its way and a value
  that arrived after a load overwrote the loaded state (issue #73). Tests that change a live state
  directly also store the values the plug-in reports (`plugin.state_values`), or the session sends
  its older values over it.
- Known issue: continuous document edits (automation writes, a plug-in streaming parameter
  changes) can keep a header job waiting until the 10 s give-up, because `plays` needs the
  document to stay unchanged for a rendered block. The job then drops with a status line and
  stores nothing (issue #219 covers the wider delivery limits).
- Commands: `plugin.preset_save` (not undoable), `plugin.preset_load` and `plugin.state_load` (one
  undo step each, recorded after the plug-in took the state), `plugin.preset_list`,
  `plugin.preset_delete`. Undoing or redoing a load asks the UI to load the stored state live
  (`Engine::take_state_restores`).
- Factory presets (VST3 program lists, CLAP preset discovery) are not listed yet.

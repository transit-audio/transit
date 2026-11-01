# Hosted plug-in windows and presets

Issue #68. Read with [ADR 0018](../../adr/0018-plugin-host-thread.md) (the plug-in host thread).

## The editor window

- On Windows both format hosts open editors in `transit_rt::editor_window`: a top-level window
  on the plug-in host thread with a header strip (GDI, no egui: that thread has no egui context)
  above a content container the plug-in parents its view in. CLAP editors open embedded there when
  the plug-in offers it, so the strip shows; floating CLAP editors and macOS windows have none.
- On Linux (#147) both format hosts use the same `transit_rt::editor_window` API, an X11 window
  (`x11.rs`) on the host thread's own connection, without the strip yet (K94): the header's title
  names the window, and the strip's commands stay in the device card and the mixer.
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
  current document and confirms that job's own instance (`Player::plays_instance_processed`). The
  callback acknowledges each slot in a bank belonging to the exact plan generation, after its
  parameter pass and pending queue drain; skipped blocks and that slot's strip still on a worker
  cannot acknowledge it. `plays` retains its earlier rendered-document meaning. Earlier, a read missed a value still on its way and a value
  that arrived after a load overwrote the loaded state (issue #73). Tests that change a live state
  directly also store the values the plug-in reports (`plugin.state_values`), or the session sends
  its older values over it.
- Delivery bounds (#219): CLAP reserves one block-start value per supported parameter beside
  its existing MIDI/timed budget, including in the FFI input buffer. VST3 reserves enough
  parameter queues and points for all stored values plus its timed and controller budgets in
  one block. A refused CLAP processing start retains its queue. An error returned by the plug-in after
  receiving the events drains that slice, preventing duplicate note-ons; status and tail recompute. A rejected setter leaves the
  mix cache unchanged for retry and prevents acknowledgment.
- Texture keeps its existing freeze-only idle hook while bypassed: applying a quality or buffer
  edit there would destroy the frozen state being captured. Those edits are applied by its
  normal parameter pass before the first active block. The capability is expressed by
  `Plugin::idle_applies_all_parameters` (true by default, false for Texture), not a name check.
  Only its own state job waits: other instances confirm independently.
- Known issue: continuous document edits (automation writes, a plug-in streaming parameter
  changes) can keep a header job waiting until the 10 s give-up, because `plays` needs the
  document to stay unchanged for a rendered block. The job then drops with a status line and
  stores nothing. Bypassed inserts receive the latest block-start values through the normal
  host queue (#219); the first active block consumes them before rendering, with no further
  lane change needed. A job with a bypassed insert's pending values waits for reactivation;
  inactive own inserts/tracks confirm stored values after document adoption/render, without a new
  parameter pass. Reads go to the live instance (it survives deactivation and holds every edit) and refuse when
  there is none, since a stored state that cannot be proven current is never saved; live
  loads/restores use the host. A failed own slot refuses with the existing invalid-audio failure status.
  An unrelated inactive, failed or bypassed slot never gates the job. If its own bypassed values
  stay queued, the existing give-up says "The audio session is still updating; try again after it
  applies" and saves nothing.
- Commands: `plugin.preset_save` (not undoable), `plugin.preset_load` and `plugin.state_load` (one
  undo step each, recorded after the plug-in took the state), `plugin.preset_list`,
  `plugin.preset_delete`. Undoing or redoing a load asks the UI to load the stored state live
  (`Engine::take_state_restores`).
- Factory presets (VST3 program lists, CLAP preset discovery) are not listed yet.

# ADR 0030: Editor window cohesion without an OS owner

- **Status:** accepted 2026-10-30 (issue #173; Windows. Linux after #147)
- **Date:** 2026-10-28
- **Amends:** [ADR 0018](0018-plugin-host-thread.md) section 7 (Platforms): the editor window still has no owner, but transit now groups editors with the main window by hand.
- **Reopen when:** out-of-process hosting (#9) lands and a request can no longer be posted the way this ADR describes; a platform offers an ownership link with no input-queue coupling; or a tester shows a case the grouping cannot express (for example a plug-in's own extra windows).

## Context

Plug-in editors are top-level windows created on the plug-in host thread with no owner (ADR 0018 section 7; K10 records the cost). The choice is deliberate. On Windows an owner window on another thread attaches the two threads' input queues, so a plug-in that stops pumping messages would stop the UI thread's input with it. The same coupling appears across a process boundary once editors live in another process (#9).

The user-visible cost today:

- Each editor has its own taskbar button and its own Alt-Tab entry.
- Switching away from transit and back (Alt-Tab, or the taskbar) does not bring the editors back with the main window.
- Minimizing transit leaves the editors on screen.
- Editors get lost in the window stack behind other applications.

Ableton Live and Logic Pro both keep plug-in windows with the application: they minimize, hide and return with it. Workflow behavior follows what those two agree on unless transit can do clearly better ([roadmap](../roadmap.md)), and here there is nothing to improve on, only a gap to close.

## Decision

**The invariant: no thread of the UI ever depends on a plug-in's thread to make progress.** The UI may ask an editor to change state; it never links its window or its input queue to the editor's.

Grouping is done by hand, with no ownership link.

1. The UI thread observes the main window's activation, deactivation, minimize and restore (the root viewport's `focused` and `minimized`, which eframe keeps current also while the window is minimized, when only `App::logic` runs) and keeps the list of open editors it already has (`plugin_windows` in `crates/ui-studio`).
2. On each transition it posts a request through every host thread's existing request queue (`Host::editor_windows`): **park** (hide the shown editors and remember them), **unpark** (show the remembered ones), **front** (bring the shown ones above the main window), and, for one editor, **raise**. The host thread applies it to its own windows, as it does open and close.
3. Requests are fire-and-forget with the host thread's usual bounds. A hung plug-in ignores them; the UI thread never waits on the answer, so the main window stays responsive and the "Not responding" path (10 s) is unchanged. A thread given up on (ADR 0018 amendment 2026-10-29) is not sent them. The UI is the only writer of the process-wide state (follow, parked) and records it when it posts; a host thread only applies (`transit_rt::editor_window::apply`), bringing its windows to the state the process is in when it runs the request, so a thread that was stuck and catches up late never undoes a later minimize, restore or setting change (#173 review).
4. Switching away from transit **lowers** the editors: they go behind the other application with the main window, which the OS does by itself, so nothing is posted. Returning (Alt-Tab, the taskbar, a click on the main window) posts front: the shown editors come above the main window, unactivated, in their order. This is what owned windows do on Windows, and Live and Logic keep plug-in windows on screen when another application is in front. Hiding on every deactivation would also hide them when the user clicks into an editor, which deactivates the main window.
5. Minimizing transit **parks** its editors (hidden, remembered); restoring unparks them where they were, in their current state, unactivated (`SW_SHOWNA`). They are hidden rather than minimized because a top-level tool window with no owner minimizes to a stub above the taskbar, not into transit's button; owned windows are hidden with their owner the same way. An editor that finishes opening while transit is minimized is parked with the others. A parked editor still counts as open: the format hosts' "closed by the user" check (`EditorWindow::is_visible`) reads it as open, so its view is not torn down.
6. Selecting a plug-in whose editor is open (its mixer insert, the device card's title, Open Instrument Editor) raises that window, unactivated, so the keyboard stays with the main window. An editor is never duplicated: the format hosts' open already raises an open window instead of making a second one.
7. Closing an editor never closes transit: the windows still have no owner, so nothing is destroyed with another window.
8. Editors that follow the main window use the tool-window style (`WS_EX_TOOLWINDOW`: no taskbar button, not in Alt-Tab). **Window > Plug-in Windows Follow transit** (`UiState::editors_follow`, command `window.editors_follow`, on by default, saved per machine with the other UI preferences) turned off restores the behavior before #173 for the windows already open and for new ones: own taskbar button, no grouping. Turning it off also brings back parked editors.
9. Linux, after #147 (native Linux editor windows): on X11, `WM_TRANSIENT_FOR` may be set directly, because it has no input-queue coupling; on Wayland, parenting stays within the one client. macOS keeps today's behavior until editors can leave the main thread (K10).
10. Out-of-process hosting (#9) keeps this design: the same requests cross a process boundary instead of a thread boundary, and an unresponsive process is ignored the same way.

## Implementation (Windows, 2026-10-30)

| Where | What |
|---|---|
| `crates/ui-studio/src/editor_group.rs` | `drive` (each frame, before the editors are driven): the main window's change, `requests(before, now)`, posted through the editors' connection (the player, or the fake devices' in tests); `show_editor` (raise, or open); `inspect` for `ui.inspect`'s `editor_windows`; `window_command` for `ui.window` |
| `crates/playback/src/player/device.rs` | `Player::editor_windows`, `raise_editor`, `editor_window` |
| `crates/plugin-host/src/editor_windows.rs` | `Host::editor_windows`: records the process-wide part, then one fire-and-forget job per live host thread that was not given up on; the window an editor's open made (`note_opened`), its state published by the tick (`publish`), a raise job |
| `crates/rt/src/editor_window/win/group.rs` | on the window's own thread only: `apply` (park, unpark, front, follow with the restyle, to the current process-wide state; what host threads run), `group` (records, then applies: inline mode and tests), `raise_window`, `window_state`, `take_created` |
| `crates/dsp/src/lib.rs` | `EditorWindowRequest`, `EditorWindowState`, `PluginEditor::raise` and `window_state` (defaults: nothing) |

The Win32 calls are all made by the thread that owns the window: `EnumThreadWindows` with the calling thread's id, `ShowWindow`, `SetWindowPos` (`HWND_TOP` with `SWP_NOACTIVATE`), `SetWindowLongPtrW` for the extended style, `IsIconic` and `IsWindowVisible`. None is made on another thread's window, so none can wait for a hung one; the UI's side is atomics and a channel send.

## Implementation (Linux, 2026-10-30, #147 slice 1)

`crates/rt/src/editor_window/x11.rs` applies the same requests with X11's means, on the host thread's own connection: park unmaps the shown editors, unpark maps them again with `_NET_WM_USER_TIME` 0 (no focus), front restacks them (`StackMode::Above`), and following drops the taskbar and pager entries (`_NET_WM_STATE_SKIP_TASKBAR`, `_SKIP_PAGER`: a property before the first map, an EWMH request after). Point 9's `WM_TRANSIENT_FOR` is not set yet (the host thread does not know the main window's id), so on some window managers activating transit does not lift the editors above it (K94). Tests: `crates/rt/tests/editor_x11.rs` (park, unpark, front, the taskbar state) and `crates/plugin-host/tests/editor_windows.rs`, which runs on Linux under an X server.

## Consequences

- For the user: editors follow the main window on Windows (and on Linux after #147): one taskbar button, Alt-Tab returns to the whole set, minimize and restore are shared, selecting a plug-in shows its editor. Plug-in-heavy sessions stop cluttering the taskbar.
- Unchanged: crash isolation. A plug-in that crashes still takes the process with it; there is none until #9. Hang isolation (the invariant) is kept.
- Cost: transit now carries window-state logic that an OS owner would have given for free, including the ordering corner cases (restore order, focus after restore, an editor the user minimized by hand).
- **Test plan.** State transitions are driven through the control channel and asserted with `ui.inspect`, which reports `editors_open` and each editor's state: activate, deactivate, minimize, restore, select an already-open plug-in (no duplicate), close an editor, and a plug-in whose thread is wedged (the main window still answers). A manual checklist covers what the control channel cannot: multi-monitor placement, per-monitor DPI, focus after restore, Alt-Tab and taskbar behavior, and shutdown order (editors close before the host thread stops, ADR 0018 amendment 2026-10-25).
- **Known limitation.** A plug-in that opens its own extra windows (a preset browser, a licensing dialog) is outside the grouping: transit does not know about them and does not manage them. A CLAP editor that floats (no embedded window of ours) is outside it too.
- **Known limitation.** Returning to transit by clicking an editor (not the main window) activates that editor only; the main window and the other editors come up at the next click on the main window. Owned windows would bring the owner along.
- **Tests.** `crates/rt/src/editor_window/win/group_tests.rs` (real windows, no plug-in): park and unpark in place, an editor opened while minimized waits with the others, nothing parked or restyled with the setting off, the tool style keeps the content size and adds no owner, raise brings one editor above another without a new window, another thread's window is never touched. `crates/plugin-host/tests/editor_windows.rs`: a host thread held inside a plug-in (a channel, E49) leaves every grouping call returning at once and applies the requests in order when it returns (a job queued behind them is the completion signal, E62); a thread given up on is not sent them; a fake plug-in whose editor is a real editor window parks, comes back, raises without a second window and closes. `crates/ui-studio/src/editor_group_tests.rs` (fake host, asserted through `ui.inspect`): activate, deactivate, minimize, restore, raise without a duplicate, close (transit keeps running), the setting off, and a stuck host thread whose requests wait while the frame loop goes on.

## Manual checklist (the coordinator's control-channel run, Windows)

Setup: a scratch config, a session with two hosted plug-ins with editors (one VST3, one CLAP) on a track, both editors open (`ui.set {plugin_windows}`), `editor_windows.follow` true in `ui.inspect`.

1. **Taskbar and Alt-Tab.** Only transit's button is on the taskbar; Alt-Tab lists transit once. With **Window > Plug-in Windows Follow transit** off (`window.editors_follow {value: false}`), each editor has its own button and Alt-Tab entry again (the behavior before #173); back on, they go.
2. **Minimize and restore.** `ui.window {minimized: true}`: both editors disappear, `editor_windows.editors[].state` reads `parked`, `main` reads `minimized`. Restore from the taskbar (or `ui.window {minimized: false}`): both return at the same place and size, unactivated; `state` reads `shown`; `posted` ends with `unpark`, `front`.
3. **Deactivate and activate.** Click another application over the editors: they go behind it with the main window (nothing posted). Alt-Tab back to transit: the editors come above the main window, and the main window keeps the keyboard focus (a transport shortcut works at once).
4. **Raise, no duplicate.** Move one editor behind the other, then click its mixer insert (or double-click its device card's title): it comes to the front, the keyboard stays with the main window, and `editors_open` still lists two.
5. **Close.** Close one editor with its title bar's X: transit stays open, the other editor stays, and the slot's button reads Edit again.
6. **Multi-monitor.** Move one editor to a second monitor; minimize and restore: it returns on that monitor, at its place.
7. **Per-monitor DPI.** With monitors at different scaling (100% and 150%), move an editor between them: the header strip rescales and the plug-in keeps its size (#101). Turn the setting off and on: the frame is resized so the plug-in keeps its pixel size (the tool caption is shorter).
8. **Focus after restore.** After step 2, typing goes to the main window, not to an editor.
9. **A plug-in that stops answering.** With an editor whose plug-in is held (the #205 fixture, or a plug-in's modal dialog left open), minimize and restore transit: the main window answers at once (`ui.inspect` answers, frames go on); the held editor does not follow until its plug-in returns; the 10 s "Not responding" message and Give up behave as before.
10. **Shutdown order.** With editors parked (transit minimized), quit through the control channel (`app.quit`): the editors close before the host thread stops (ADR 0018 amendment 2026-10-25), the exit is clean, and no editor window is left behind.


## Alternatives rejected

- **An OS owner window** (ADR 0018 section 7): attaches the input queues of the two threads on Windows, so a hung plug-in hangs the UI; the same hazard exists across processes. This is the property the project protects.
- **Child-window embedding** (editor as a child of the main window): cross-process embedding is not available everywhere, it shares the input-queue coupling on Windows, and it would pin editors inside the main window, which blocks docking and detaching them later.
- **Doing nothing:** keeps the hang isolation but leaves the daily cost above in place, and it falls short of the convention Ableton and Logic agree on.

## Related

[ADR 0018](0018-plugin-host-thread.md) (plug-in host thread, section 7), issue #173, issue #147 (native Linux editor windows), issue #9 (out-of-process hosting), [K10](../known-issues/README.md).

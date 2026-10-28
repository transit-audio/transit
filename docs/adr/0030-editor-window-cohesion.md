# ADR 0030: Editor window cohesion without an OS owner

- **Status:** proposed 2026-10-28 (issue #173)
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

1. The UI thread observes the main window's activation, deactivation, minimize and restore (eframe/winit window events) and keeps the list of open editors it already has (`plugin_windows` in `crates/ui-studio`).
2. On each transition it posts a request to every open editor through the host thread's existing request queue: show, hide, raise, or minimize. The host thread applies it on the thread that owns the window, as it does for open and close today.
3. Requests are fire-and-forget with the host thread's usual bounds. A hung plug-in ignores them; the UI thread never waits on the answer, so the main window stays responsive and the "Not responding" path (10 s) is unchanged.
4. Switching away from transit hides (or lowers) the editors; returning brings back the ones that were visible, in their places. Minimizing transit minimizes them; restoring restores them.
5. Selecting a plug-in in the device chain or the mixer raises its existing editor. An editor is never duplicated.
6. Closing an editor never closes transit.
7. Once transit manages an editor's visibility it uses the tool-window style (no own taskbar button, not in Alt-Tab). A setting keeps today's behavior (own taskbar button, no grouping) for anyone who prefers it.
8. Linux, after #147 (native Linux editor windows): on X11, `WM_TRANSIENT_FOR` may be set directly, because it has no input-queue coupling; on Wayland, parenting stays within the one client. macOS keeps today's behavior until editors can leave the main thread (K10).
9. Out-of-process hosting (#9) keeps this design: the same requests cross a process boundary instead of a thread boundary, and an unresponsive process is ignored the same way.

## Consequences

- For the user: editors follow the main window on Windows (and on Linux after #147): one taskbar button, Alt-Tab returns to the whole set, minimize and restore are shared, selecting a plug-in shows its editor. Plug-in-heavy sessions stop cluttering the taskbar.
- Unchanged: crash isolation. A plug-in that crashes still takes the process with it; there is none until #9. Hang isolation (the invariant) is kept.
- Cost: transit now carries window-state logic that an OS owner would have given for free, including the ordering corner cases (restore order, focus after restore, an editor the user minimized by hand).
- **Test plan.** State transitions are driven through the control channel and asserted with `ui.inspect`, which reports `editors_open` and each editor's state: activate, deactivate, minimize, restore, select an already-open plug-in (no duplicate), close an editor, and a plug-in whose thread is wedged (the main window still answers). A manual checklist covers what the control channel cannot: multi-monitor placement, per-monitor DPI, focus after restore, Alt-Tab and taskbar behavior, and shutdown order (editors close before the host thread stops, ADR 0018 amendment 2026-10-25).
- **Known limitation.** A plug-in that opens its own extra windows (a preset browser, a licensing dialog) is outside the grouping: transit does not know about them and does not manage them.

## Alternatives rejected

- **An OS owner window** (ADR 0018 section 7): attaches the input queues of the two threads on Windows, so a hung plug-in hangs the UI; the same hazard exists across processes. This is the property the project protects.
- **Child-window embedding** (editor as a child of the main window): cross-process embedding is not available everywhere, it shares the input-queue coupling on Windows, and it would pin editors inside the main window, which blocks docking and detaching them later.
- **Doing nothing:** keeps the hang isolation but leaves the daily cost above in place, and it falls short of the convention Ableton and Logic agree on.

## Related

[ADR 0018](0018-plugin-host-thread.md) (plug-in host thread, section 7), issue #173, issue #147 (native Linux editor windows), issue #9 (out-of-process hosting), [K10](../known-issues/README.md).

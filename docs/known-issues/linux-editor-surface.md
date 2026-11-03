# Linux editor opens but has no visible content (#289)

An accepted CLAP `show` or VST3 `attached` proves the call succeeded, not that a child is mapped
or pixels were painted. Aetheriae by Morphor reports successful opens in both formats on the
tester's v0.6.1 system. Whether that means an empty frame or no visible frame is still unconfirmed.

The leading candidate is an unmapped XEmbed child. The [vendor's Linux download page](https://morphor.io/plugins/aetheriae/)
lists GTK 3 and WebKitGTK 4.1. [GTK's plug implementation](https://raw.githubusercontent.com/GNOME/gtk/gtk-3-24/gtk/gtkplug.c)
sets `_XEMBED_INFO`'s mapped flag from `gtk_plug_map`; the embedder must map that window.
Transit currently observes child creation but does not implement XEmbed mapping or its handshake
(K92 also records missing focus support). This is a concrete compatibility gap, but the exact
toolkit path in the tester's plug-in binary has not been established. No mapping, visual, timer,
or floating-fallback behavior is changed by this diagnostic patch.

## What to collect

Keep the editor open for at least five seconds. Paste both `editor surface:` INFO lines, the
preceding probe/open lines, and any `plug-in editor shows nothing:` WARN. The two samples run on
the plug-in host thread at the first idle after one and five seconds. A stalled host thread can
delay or prevent them; `elapsed_ms` records the actual sample time. Closing early cancels them.

The lines include the frame and content IDs, sizes, depth, visual ID/class, colormap, map state,
root coordinates, screen size, parked/shown state, session hints, and every immediate child's
attributes and `_XEMBED_INFO`. They also include this instance's delivered timer callbacks since
open, its current timer periods/registrations (VST3: IRunLoop timers and event handlers), and
events dispatched on transit's host X11 connection since open. The latter is **not** the count on
the plug-in's separate X11 connection. Timer counts record returned callbacks, not pixel updates.

| Observation | Next investigation |
|---|---|
| Child `map=UNMAPPED`, `_XEMBED_INFO=[0, 1]`, viewable parent | XEmbed map request is unhandled; reproduce with a GtkPlug-style fixture before implementing the embedding lifecycle. |
| No child, no timer callbacks | Check timer registrations/periods, deferred initialization, and toolkit errors. A GUI drawn directly into the parent can also have no child. |
| Callbacks increase, child remains unmapped | Timer delivery works; inspect the plug-in's mapping/embedding path. |
| Child `UNVIEWABLE` | It is mapped, but an ancestor is hidden; inspect frame map state and `parked`. |
| Viewable child with nonzero size, still blank | Inspect renderer/toolkit errors, GPU driver, WebKit subprocesses and the plug-in's own event pump. A mapped window does not prove painting. |
| Frame hidden, off-screen, or larger than the screen | Check desktop/window-manager placement, minimize/follow behavior, and screen resolution. |
| Query failed | A connection/window may have disappeared during sampling; absence is unknown, not zero. |

The WARN is a heuristic emitted once at five seconds only when all queried children are unmapped
(or there are none) and zero timer callbacks were delivered. It does not detect blank pixels and
does not warn solely because a mapped child has a hidden ancestor.

Also report: empty frame versus no frame; exact plug-in version and format; X11 versus Wayland,
desktop/compositor and resolution; whether the same build works in another host; GPU driver and
`glxinfo -B` if installed. Compare the same format/build on the same machine. A vendor dependency
list does not establish that a different release uses the same renderer.

## Why the visual has not changed

Transit uses CopyFromParent for depth, visual and colormap. On a usual 24-bit TrueColor root,
that produces matching 24-bit host windows. X11 allows a child to choose a different supported
visual/depth with its own compatible colormap; a GPU-rendered child does not universally require
an ARGB parent. A plug-in rendering directly to the supplied window may require a compatible
GLX visual, but choosing 24-bit TrueColor alone does not select a double-buffered GLX configuration.
See [Xlib window creation](https://xorg.freedesktop.org/archive/current/doc/libX11/libX11/libX11.html#Creating_Windows).

Both format hosts register timers on the plug-in host thread. `gui_thread` polls independently
of X events, with its timeout shortened to the next timer. E64 already synchronizes host creation
and resize before passing the parent to another connection; raising explicitly flushes. Neither
the header strip (absent on Linux) nor settle reparents a child. The next tester log must decide
which path merits a behavior fix.

The existing VST3 `the_frame_is_the_views_run_loop` test uses a real-clock stall bound before
checking callback counts. Its scheduling dependence predates this hunt; the new diagnostic
deadline, stale-registration and log tests use injected times or synchronous removal. A follow-up
should inject timer delivery into that older run-loop test. Also, the shared run loop prioritizes
X events and descriptors within a 256-call dispatch budget: sustained event traffic or a queued
job stream can delay timers. That is a separate candidate if registrations persist but callback
counts stop; no evidence currently ties it to this instance.

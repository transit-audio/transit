# Linux editor opens but has no visible content (#289)

An accepted CLAP `show` or VST3 `attached` proves the call succeeded, not that a child is mapped
or pixels were painted. Aetheriae by Morphor reports successful opens in both formats on the
tester's v0.6.1 system. Whether that means an empty frame or no visible frame is still unconfirmed.

The leading candidate is an unmapped XEmbed child. The [vendor's Linux download page](https://morphor.io/plugins/aetheriae/)
lists GTK 3 and WebKitGTK 4.1. [GTK's plug implementation](https://raw.githubusercontent.com/GNOME/gtk/gtk-3-24/gtk/gtkplug.c)
sets `_XEMBED_INFO`'s mapped flag from `gtk_plug_map`; the embedder must map that window.
Transit now recognizes valid `_XEMBED_INFO` on direct children, including properties set after
creation and windows reparented into the content window. It sends one `XEMBED_EMBEDDED_NOTIFY`
with protocol version 0. Only children the host had to map enter the XEmbed path: the host
honors their mapped flag, mirrors content size, and supports activation, logical focus and key
forwarding. Editors that map their own window keep direct keyboard focus, their chosen size
and control of their mapping even when they advertise `_XEMBED_INFO`. JUCE advertises
the property and maps itself; Qt's self-mapping behavior is unverified. JUCE may flush its map
only from a later fd/timer callback, so first detection of an unmapped child with bit 0 set
now defers the host map. Editor idle
rechecks after one full following dispatch, giving the toolkit run loop a turn, or after a
100 ms fallback measured from the later of detection and the latest dispatch completion.
Each dispatch completion restarts that fallback clock. Only a still-unmapped
child is then mapped under the server grab. A failed grabbed map other than a departed-child
error stays pending for one retry on the next due idle pass; a second failure clears pending
and logs at debug level. Each pass revalidates the child before trying again.
The completed-dispatch counter increments after
`run_loop::run` returns with a nonzero input callback budget in `crates/rt/src/gui_thread.rs`;
passes whose budget is exhausted by X11 events do not count. The detection dispatch does not count
as the full following dispatch. Late `_XEMBED_INFO` starts the same deferral.
This counter records an opportunity to run callbacks, not proof that a particular toolkit
callback ran. A later timer or a callback starved by other handlers can still miss the grace;
the run-loop budget limitation described below remains a follow-up.
They receive only EMBEDDED_NOTIFY, with no activation or logical focus messages. Non-XEmbed
children still control their own mapping. For host-mapped children,
accelerators, tab traversal, modality and drag-and-drop remain outside this subset (K92).
Ownership is based on actual mapping during this embedding lifetime, not toolkit identity.
If a previously self-mapped child later becomes UNMAPPED with bit 0 set and the host maps it
before the toolkit does, it enters the host-managed XEmbed path from then on. Preserving a
self-mapped classification across that later show cycle is a possible K92 compatibility
follow-up; the current rule deliberately follows which client performed the map.
The exact toolkit path in the tester's binary remains unconfirmed, and mapping a window does
not prove that GTK/WebKit painted it. Visual selection, timers and floating fallback are unchanged.

## What to collect

Keep the editor open for at least five seconds. Paste both `editor surface:` INFO lines, the
preceding probe/open lines, and any `plug-in editor shows nothing:` WARN. The two samples run on
the plug-in host thread at the first idle after one and five seconds. A stalled host thread can
delay or prevent them; `elapsed_ms` records the actual sample time. Closing early cancels them.

The lines include the frame and content IDs, sizes, depth, visual ID/class, colormap, map state,
root coordinates, screen size, parked/shown state, session hints, and every immediate child's
attributes and `_XEMBED_INFO`, plus `xembed=[version, flags, mapped_by_host=true/false,
notify_sent=true/false]` (or `xembed=none` when no valid client is tracked). `mapped_by_host`
records a successful host map of an UNMAPPED child with bit 0 set during this embedding
lifetime, not current visibility. A final map-state check and map under a short X server grab
exclude a child that maps itself before the host's map;
`notify_sent` records a server-accepted handshake, not toolkit receipt. They also include this instance's delivered timer callbacks since
open, its current timer periods/registrations (VST3: IRunLoop timers and event handlers), and
events dispatched on transit's host X11 connection since open. The latter is **not** the count on
the plug-in's separate X11 connection. Timer counts record returned callbacks, not pixel updates.

| Observation | Next investigation |
|---|---|
| Child `map=UNMAPPED`, `_XEMBED_INFO=[0, 1]`, viewable parent | Compare `xembed`, `mapped_by_host` and `notify_sent`. Missing state suggests detection/property failure; a successful host map followed by UNMAPPED suggests a later client unmap. Paste both samples and any X11 errors. |
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
For a JUCE editor, also paste its `mapped_by_host` reading: it should be `false` when the
toolkit maps itself during the deferral, with direct focus and its chosen size preserved.

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
whether the XEmbed fix resolves the editor or toolkit/renderer investigation is still needed.

The existing VST3 `the_frame_is_the_views_run_loop` test uses a real-clock stall bound before
checking callback counts. Its scheduling dependence predates this hunt; the new diagnostic
deadline, stale-registration and log tests use injected times or synchronous removal. A follow-up
should inject timer delivery into that older run-loop test. Also, the shared run loop prioritizes
X events and descriptors within a 256-call dispatch budget: sustained event traffic or a queued
job stream can delay timers. That is a separate candidate if registrations persist but callback
counts stop; no evidence currently ties it to this instance.

## Linux regression gate

Under Xvfb, `editor_x11` covers mapping and one handshake for a plug that never maps itself,
flag clearing/remapping, content resize, absent/malformed/late properties, reparenting, teardown,
activation, click focus requests and key forwarding. The CLAP `xembed_plug` fixture makes a real
child and records the received ClientMessage; its host test reads the child's map state, handshake
and size, then clears the flag and checks UNMAPPED. These tests use bounded host idle passes with
explicit server readouts, not elapsed-time assertions. On the pre-fix host the first mapped
assertion fails because the child remains UNMAPPED. The CLAP test runs on the ambient display
(WSLg in the WSL gate), where a window manager maps the editor's top-level on its own schedule, so
it asserts mapped (VIEWABLE or UNVIEWABLE), not VIEWABLE. `editor_x11` models that window manager
with a client holding SubstructureRedirect on the root: the host maps the child while the frame's
MapRequest is held (UNVIEWABLE), and it turns VIEWABLE once the frame is mapped. Runtime execution is required in the Linux merge/CI
job; Windows cross-target checking alone is not runtime evidence.

The self-mapped regression child advertises `_XEMBED_INFO = {0, 1}` and maps itself at 123x45
inside 300x200 content. Frame activation and reactivation must leave X focus on the child,
its size unchanged, and deliver only one EMBEDDED_NOTIFY. Content resizing, flag clearing and
REQUEST_FOCUS must not opt it into host management. On 28f0d516, the focus assertion fails
because focus moves to the proxy; independently, its size becomes 300x200. The host-mapped
key test also checks that forwarded child and coordinate fields are zero.

The JUCE-shaped deferred-flush regression first creates the child, advertises `{0, 1}` and
flushes, then runs one host dispatch before the client maps itself. After the next dispatch,
X focus must be on the child, size must remain 123x45, the surface readout must report
`mapped_by_host=false`, and only one EMBEDDED_NOTIFY may arrive. On 935d0001 its first focus
assertion sees the proxy: detection already mapped and claimed the child. The GTK-shaped
test instead leaves the child unmapped and checks the host map at the second dispatch's idle;
late property arrival follows the same sequence. Idle uses a frozen readout so these checks
have no fallback-time dependency, including two intervening zero-budget passes that must
not advance the deferral. A separate unit test injects the fallback's stamp and readout.

For children the host had to map, top-level activation and logical focus are independent under
[XEmbed section 5.2](https://specifications.freedesktop.org/xembed/latest-single/).
Deactivation sends WINDOW_DEACTIVATE; logical focus survives so returning restores the same
field. Unmapping, hiding, or leaving the embedding site sends FOCUS_OUT. Toolkit clicks use
REQUEST_FOCUS and receive FOCUS_IN/CURRENT without activating an inactive frame. A mapped,
off-screen sibling of the content window holds X focus and forwards key press/release events
to the focused plug. It has no embedded descendants, so pointer position cannot bypass the
host's keyboard routing; selecting mouse events on the foreign window would interfere with
its toolkit.

Focus transfers still use CurrentTime. A server-focus check rejects already-stale activation
events, but another client can change focus between that query and SetInputFocus. Timestamped
WM_TAKE_FOCUS handling remains a follow-up (K92); this patch does not claim to resolve that race.

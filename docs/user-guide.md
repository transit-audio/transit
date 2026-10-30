# User guide: appearance and interface size

Open **View > Appearance** to choose **System**, **Dark**, or **Light**. System follows
your operating system's appearance. These choices are saved per machine in `ui.json`.

**View > Appearance > Interface zoom** shows the current zoom and offers **100%**, **125%**,
**150%**, **175%**, and **200%** presets. Zoom changes every widget and painted label, including
the arrangement, browser, mixer, and editor panels. It is saved per machine and restored at
startup; it does not change your session or create an undo step.

Press **Ctrl+=** or **Ctrl++** to enlarge the interface by a factor of 1.1, **Ctrl+-** to
reduce it by the same factor, and **Ctrl+0** to reset to 100%. On macOS, use Cmd instead
of Ctrl. Zoom is limited to 75% through 300%. Each change appears in the status line.
Plain **+**, **=**, and **-** still zoom the arrangement's timeline.

Interface zoom multiplies your operating system's display scale: 150% interface zoom on a
200% display uses three physical pixels per UI point. At 100%, transit follows the display
scale as before. Higher zoom leaves less space for panels; choose a smaller zoom if needed.
Plug-in editor windows follow the display scale, not this zoom.

See the [shortcuts reference](shortcuts.md) and **Help > Keyboard Shortcuts** for key bindings.

## Cycle

Cycle (C, the transport button or Transport > Cycle On) only turns looping on and off; it never
moves the cycle region. Turning Cycle on reuses the region you already have, whether you dragged
it on the loop bar or set it earlier. Only when there is no region does Cycle On make one: from
the span of the selected clips (earliest start to latest end), else the selected time range, else
four bars from the bar holding the playhead, in one undo step with the switch. Cycle Off keeps
the region. To move the region to the current selection, use Transport > Set Cycle to Selection
(Ctrl+L, Cmd+L on macOS), which does not toggle Cycle.

## Plug-in windows

On Windows, plug-in editor windows stay with transit's window: minimizing transit hides them and
restoring it brings them back where they were; returning to transit (Alt+Tab, its taskbar button
or a click on its window) brings them in front of it. They have no taskbar buttons of their own and
are not listed in Alt+Tab. Clicking a plug-in that already has its window open (its mixer insert,
its device card's title, or Open Instrument Editor) brings that window to the front instead of
opening a second one. To keep each plug-in window apart, with its own taskbar button, turn off
**Window > Plug-in Windows Follow transit**; the choice is saved per machine. A plug-in that stops
responding does not hold transit up: its window simply stops following until it answers again.
Windows a plug-in opens by itself (its own preset browser or a licensing dialog) are not grouped.
Returning to transit by clicking a plug-in window brings up only that window; the main window and
the other plug-in windows come up on your next click on the main window.

On Linux, VST3 and CLAP editors open in an X11 window of transit's, from the track header, the
device card and the mixer slot, as on Windows. On a Wayland desktop they open through XWayland,
the X11 compatibility layer GNOME and KDE start by themselves: the plug-in sees X11 either way, so
nothing needs setting up. Windows plug-ins run through [yabridge](https://github.com/robbert-vdh/yabridge)
open their editors the same way (yabridge embeds the Windows editor in that X11 window). The
editor window takes the size the plug-in asks for and follows it when the plug-in resizes itself;
its scale is the desktop's (`Xft.dpi`, which GNOME and KDE set from their scaling setting). With
**Window > Plug-in Windows Follow transit** on, minimizing transit hides the editors and restoring
it brings them back, and they have no entry of their own in the taskbar. What does not work yet on
Linux: the editor window has no header strip (presets, Save, Bypass and MIDI Learn stay in the
device card and the mixer), keyboard focus can stay with the main window until you click into the
plug-in, and an editor keeps one scale on every monitor ([known issues K92 to K94](known-issues/README.md)).

## FX channels and sends

+ FX Channel (the mixer button, Track > Add FX Channel, or the arrangement quick-add) adds an FX channel and
its bus and nothing else: no track gets a send, whatever is selected. Sends come from a strip's
**+ Send** slot. Its **New FX Channel...** entry creates the channel and a send to it from that
strip at -12 dB, in one undo step, the way Logic and Live do it. Escape in the arrangement, or a
click on empty space below the tracks or in the mixer, deselects the clips and tracks: no mixer
strip or track header is highlighted. The edit cursor keeps its track, which stays the target of
cursor commands (Paste, Split, New MIDI Clip) and of the Devices pane until you click a track
again.

## A plug-in that stops responding

If a plug-in stops responding (its editor turns white), transit names it in a message: audio keeps playing, **Save now** saves the session with the plug-in's last stored state, and **Give up on <plug-in>** lets transit carry on without it until you restart; End Task on the plug-in's window would end transit ([troubleshooting](troubleshooting.md#a-plug-in-stopped-responding)).

## Multicore mixer render

transit renders mixer strips on several CPU cores (one render worker per core, minus one for the
audio thread, at most eight). Each block gives the strips a deadline: a strip that misses it is
silent for that block and is counted under **Render worker figures** in Copy diagnostics
([troubleshooting](troubleshooting.md)). Sessions with sidechain keys render on the workers too,
through a dependency graph: a keyed strip starts once its key source has rendered. Strips that read
a live input or hardware (monitored audio tracks, External Instruments, hardware inserts) render on
the audio thread as before.

To render every strip on the audio thread instead, start transit with the environment variable
`TRANSIT_RENDER_POOL=0` (for example `set TRANSIT_RENDER_POOL=0` in a Windows command prompt
before `transit`, or `TRANSIT_RENDER_POOL=0 transit` on Linux and macOS). Do this when a plug-in
misbehaves on a worker thread: its track drops out, or keeps going silent while Strip overruns
(busy) keeps rising, as when a plug-in hangs ([known issue K89](known-issues/README.md)). Unset,
or `1`, uses the workers. An app opened from the Start menu, a desktop shortcut or Finder does not
see a shell variable, so set `TRANSIT_RENDER_POOL` as a user environment variable instead (Windows:
System Properties > Environment Variables; macOS: a launchd user environment or a shell launcher)
and restart transit. If only sessions with sidechain keys misbehave, set `TRANSIT_RENDER_GRAPH=0` the
same way instead: those sessions then render on the audio thread, and sessions without keys keep the
workers.

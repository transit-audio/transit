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

## FX channels and sends

+ FX Channel (the mixer button, Track > Add FX Channel, or the arrangement quick-add) adds an FX channel and
its bus and nothing else: no track gets a send, whatever is selected. Sends come from a strip's
**+ Send** slot. Its **New FX Channel...** entry creates the channel and a send to it from that
strip at -12 dB, in one undo step, the way Logic and Live do it. Escape in the arrangement, or a
click on empty space below the tracks or in the mixer, deselects the clips and tracks: no mixer
strip or track header is highlighted. The edit cursor keeps its track, which stays the target of
cursor commands (Paste, Split, New MIDI Clip) and of the Devices pane until you click a track
again.

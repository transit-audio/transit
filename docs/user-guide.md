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

Turning Cycle on (C, the transport button or Transport > Cycle On) with clips selected sets the
cycle region to the span of those clips, from the earliest start to the latest end, and with a
time range selected it sets the region to that range; either replaces a region that was already
there, in one undo step. With nothing selected, Cycle reuses the existing region, or makes four
bars from the bar holding the playhead when there is none. Cycle Off keeps the region, and
Transport > Set Cycle to Selection (Cmd+L) re-sets it from the selection without toggling Cycle.
While clips are selected, every Cycle On and Set Cycle to Selection takes the selected clips' span, so a region you dragged on the loop bar is replaced on the next Cycle On unless you deselect the clips first (Escape or click empty space); Select All counts as a clip selection.

## FX channels and sends

+ FX Channel (the mixer button, Track > Add FX Channel, or the arrangement quick-add) adds an FX channel and
its bus and nothing else: no track gets a send, whatever is selected. Sends come from a strip's
**+ Send** slot. Its **New FX Channel...** entry creates the channel and a send to it from that
strip at -12 dB, in one undo step, the way Logic and Live do it. Escape in the arrangement, or a
click on empty space below the tracks or in the mixer, deselects the clips and tracks: no mixer
strip or track header is highlighted. The edit cursor keeps its track, which stays the target of
cursor commands (Paste, Split, New MIDI Clip) and of the Devices pane until you click a track
again.

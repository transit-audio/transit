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

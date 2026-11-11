# Keyboard shortcuts

**Help > Keyboard Shortcuts** lists every binding, grouped by scope, with a search field.
The keymap is defined in `crates/ui-studio/src/shortcuts.rs`. Ctrl means Cmd on macOS.
Global shortcuts apply anywhere unless a text field or dialog has the keyboard.

## Info panel

| Scope | Action | Keys |
|---|---|---|
| Global | Info Panel | Shift+? |

Ableton's Info View and Logic's Quick Help both use Shift+?. The panel sits bottom-left and explains the control or figure under the pointer, with its key and a More button for the docs page. Help > Info Panel does the same.

## Device editors

| Scope | Action | Keys |
|---|---|---|
| Device editor | Close Editor | Escape |

Escape closes the device editor window you last clicked in or opened; a click outside every
editor window hands the keyboard back to the views. A value being typed in a knob's field, an open
list or a dialog keeps Escape for itself. **Window > Close All Editors** closes every editor.

## Sampler

| Scope | Action | Keys |
|---|---|---|
| Sampler | Clear pad | Delete, Backspace |
| Sampler | Audition slice | Space, Enter |
| Sampler slices | Previous slice / Next slice | Left / Right |
| Sampler slices | Nudge marker earlier / later | Alt+Left / Alt+Right |
| Sampler slices | Nudge marker earlier / later ×10 | Alt+Shift+Left / Alt+Shift+Right |
| Sampler slices | Remove slice marker | Delete |
| Sampler slices | Cancel marker drag | Escape |

The focused Sampler's selected pad is cleared in one undo step. A text field, open menu, or
dialog keeps the keys for itself. Right-click a pad for **Clear pad** and, when it stores its
own note, **Reset note**.

On the Slices view, Space or Enter plays the selected slice's pad at the piano roll's audition
velocity for as long as the key is held, while the Sampler window has the keyboard (the window
you last clicked); elsewhere Space plays the song and Enter returns to zero.

The Sampler slices keys act on the Slices view's bench under the same rule. Left and Right select
the previous and next slice. The nudges move the selected slice's start marker by one frame (ten
with Shift) in Manual, one undo step each. Delete removes the selected slice's start marker, never
the loop start. Escape cancels a marker drag in progress and sends nothing; otherwise it closes the
window as usual. The keys never reach the arrangement while the Slices view has the keyboard.

## Interface and arrangement zoom

| Scope | Action | Keys |
|---|---|---|
| Global | Zoom Interface In | Ctrl+=, Ctrl+Plus, Ctrl+Shift+Plus |
| Global | Zoom Interface Out | Ctrl+- |
| Global | Reset Interface Zoom | Ctrl+0 |
| Arrangement | Zoom In | +, =, Shift+Plus |
| Arrangement | Zoom Out | - |
| Arrangement | Increase Track Heights | Alt+=, Alt+Plus (without Shift) |
| Arrangement | Decrease Track Heights | Alt+- |

Plus means the **+** key (including a numeric keypad); Shift+Plus covers keyboards where
typing **+** takes Shift. Interface zoom multiplies or divides the current size by 1.1,
within 75% to 300%; reset restores 100%. The setting is saved per machine. Plain arrangement
zoom changes the timeline scale, while interface zoom changes all widgets and painted text.

Preset sizes and the current value are in **View > Appearance > Interface zoom**.
See the [user guide](user-guide.md) for display scaling and plug-in editor behavior.

## Piano roll

| Scope | Action | Keys |
|---|---|---|
| Piano roll | Fold to Scale On / Off | F |

F is Ableton Live's key for Fold in the MIDI editor. It runs the same command as **Fold to scale** in the
piano roll's Pitch menu, so the menu, the command and the key always agree; with Scale mode off the
setting is kept and a status line says it applies once Scale mode is on. A typed field keeps F for itself.

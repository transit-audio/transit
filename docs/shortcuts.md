# Keyboard shortcuts

**Help > Keyboard Shortcuts** lists every binding, grouped by scope, with a search field.
The keymap is defined in `crates/ui-studio/src/shortcuts.rs`. Ctrl means Cmd on macOS.
Global shortcuts apply anywhere unless a text field or dialog has the keyboard.

## Info panel

| Scope | Action | Keys |
|---|---|---|
| Global | Info Panel | Shift+? |

Ableton's Info View and Logic's Quick Help both use Shift+?. The panel sits bottom-left and explains the control or figure under the pointer, with its key and a More button for the docs page. Help > Info Panel does the same.

## Interface and arrangement zoom

| Scope | Action | Keys |
|---|---|---|
| Global | Zoom Interface In | Ctrl+=, Ctrl+Plus, Ctrl+Shift+Plus |
| Global | Zoom Interface Out | Ctrl+- |
| Global | Reset Interface Zoom | Ctrl+0 |
| Arrangement | Zoom In | +, =, Shift+Plus |
| Arrangement | Zoom Out | - |

Plus means the **+** key (including a numeric keypad); Shift+Plus covers keyboards where
typing **+** takes Shift. Interface zoom multiplies or divides the current size by 1.1,
within 75% to 300%; reset restores 100%. The setting is saved per machine. Plain arrangement
zoom changes the timeline scale, while interface zoom changes all widgets and painted text.

Preset sizes and the current value are in **View > Appearance > Interface zoom**.
See the [user guide](user-guide.md) for display scaling and plug-in editor behavior.

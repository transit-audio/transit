//! The one rule for typing into a value field (#484; `docs/design/language`, Typing a value):
//! Enter commits, Escape cancels, and a click anywhere else commits and hands the keyboard back,
//! as Ableton's and Logic's value fields do (a typed value is kept when the field is left, Escape
//! is the only way to give it up).
//!
//! The field asks for the keyboard once, in the frame it opens, and never again: a press elsewhere
//! takes the focus away from it (egui's surrender on a press outside the focused widget), and the
//! field ends its typing in the frame it sees that, so the next field, the browser's search or the
//! app's shortcuts get the keys at once. A field that asked every frame took the focus back in the
//! same frame the press gave it away, and kept the whole app's keyboard until Enter or Escape.

/// What the typing does after this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    /// Still typing: the field keeps the keyboard.
    Open,
    /// Enter, or the field let go of the keyboard (a click elsewhere): apply the text. `enter`
    /// says which, for a caller that keeps a refused text open after Enter only.
    Commit { enter: bool },
    /// Escape, or the field came back without the keyboard it never let go of here (it was not
    /// drawn for a while): give the text up.
    Cancel,
}

/// Whether the cancel key (`Action::ReadoutCancel`, Escape) was pressed this frame, read from the
/// raw events: a text field gives its focus up on Escape before a window's keys run, and the
/// window may consume the key. The keys come from the shortcut table.
pub fn escaped(ui: &egui::Ui) -> bool {
    ui.input(|i| crate::shortcuts::hit_raw(i, crate::shortcuts::Action::ReadoutCancel))
}

/// The typing's state after this frame for the field `resp` (a `TextEdit` drawn this frame).
/// `opened`: the typing began this frame; the field asks for the keyboard then, and only then (a
/// caller that opens the field with its text selected asks before drawing it as well: egui drops
/// the selection of a field drawn without the keyboard).
pub fn typing(ui: &egui::Ui, resp: &egui::Response, opened: bool) -> Typing {
    if opened {
        resp.request_focus();
        return Typing::Open;
    }
    if escaped(ui) {
        return Typing::Cancel;
    }
    if resp.has_focus() {
        Typing::Open
    } else if resp.lost_focus() {
        Typing::Commit { enter: ui.input(|i| crate::shortcuts::hit_raw(i, crate::shortcuts::Action::ReadoutApply)) }
    } else {
        Typing::Cancel
    }
}

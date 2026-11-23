//! The decided part of the exceptions list (#734): registered commands the interface does not run
//! by name, each with a checkable one-line reason that says how the interface does the same
//! thing another way (another command, a direct call into the engine or the model, the engine's
//! own record placement) or why the command is for the control channel only.
//!
//! Every reason was checked against the command's handler in `crates/engine/src/cmd/` and against
//! the interface code it names. A reason that stops being true (the interface starts reading or
//! running something else) must be rewritten, or the command moves to `undecided.rs`.

/// `(command id, reason)`.
#[rustfmt::skip]
pub const CONTROL_ONLY: &[(&str, &str)] = &[
    ("edit.select_all", "the arrangement's Ctrl+A (A::SelectAll, actions.rs select_all_clips) selects every clip through edit.select; this form also selects all tracks and the whole time range, and is the control-channel form"),
    ("edit.nudge", "the Nudge menu rows and Alt+Left/Right move the clips by the arrangement grid step through edit.move_clips (edit_tools.rs nudge_selected, which says so); this form moves by the session's own nudge value and is control channel only"),
    ("view.nudge", "sets the session's nudge value, which only edit.nudge reads; the interface nudges by the arrangement grid step (edit_tools.rs), so this setter is control channel only"),
    ("view.grid", "sets the session's engine grid, which engine commands such as edit.separate_on_grid read; the interface keeps its own view-only grid (grid.rs GridView, not saved) and never reads this one, so this setter is control channel only"),
    ("edit.squash_undo", "the interface merges its own undo steps by calling Engine::squash_undo directly (host/recording.rs, automation/pass.rs); this command is the control-channel form of that call"),
    ("engine.history", "control channel query of the undo labels; the Undo History window reads the same list directly with Engine::undo_history (finish.rs history_window)"),
    ("engine.parity", "control channel query; crates/automation and apps/transit-cli answer engine.parity themselves, and the interface has no parity view"),
    ("session.format_time", "control channel query; the interface formats positions by calling Session::format_position directly (transport.rs, clip_inspector.rs), which is all this command does"),
    ("query.track_heights", "control channel query; the arrangement reads each track's height from the model directly (arrange/track_height.rs), which is what this command reports"),
    ("midi.notes", "control channel query; the piano roll and arrangement read the clip's notes from the model directly (arrange/clips.rs reads sequence.notes), and the command lists the same list the note commands index"),
    ("takes.list", "control channel query; the take lanes read the same stacks directly with Track::take_stacks through the take cache (takes.rs), which is all this command does"),
    ("setup.keyboard_shortcuts", "control channel query of the registry's shortcut names; Help > Keyboard Shortcuts draws the interface's own table (shortcuts.rs TABLE), not these strings"),
    ("edit.trim_to_fill_selection", "run by the engine's own record placement (io/recording.rs place_recording, edit.trim_to_fill_selection on punch) when the interface records; no menu or control runs it by hand"),
    ("track.playlist_duplicate", "run by the engine's own take placement (io/recording.rs, midi_take.rs) when a new take lands over existing clips, so the interface's recording uses it; no menu or control runs it by hand"),
];

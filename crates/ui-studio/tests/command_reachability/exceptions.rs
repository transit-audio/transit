//! The only hand-kept list of the gate (#734): registered commands the interface does not run by
//! name, each with a reason. The list only shrinks: a command that gains a surface (or is retired)
//! must leave it, and a new command must not join it without a reason.
//!
//! Two sections, in two files:
//! - `control_only.rs`: decided. Each entry says how the interface does the same thing another way
//!   or why the command is for the control channel only.
//! - `undecided.rs`: not ruled on. Each reason starts with "undecided" and ends with its first-pass
//!   group (stub, duplicate, orphan), so the maintainer can rule by group. When no entry says
//!   "undecided" any more the gate can go into `cargo xtask ci`.

mod control_only;
mod undecided;

/// Every exception, decided first.
pub fn all() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    control_only::CONTROL_ONLY.iter().chain(undecided::UNDECIDED.iter())
}

/// The decided entries.
pub fn decided() -> &'static [(&'static str, &'static str)] {
    control_only::CONTROL_ONLY
}

/// The entries nobody has ruled on.
pub fn undecided() -> &'static [(&'static str, &'static str)] {
    undecided::UNDECIDED
}

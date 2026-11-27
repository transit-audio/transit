//! Code the CLAP and VST3 hosts share (issue #736, package P12): the fade [`gate`] a hosted
//! plug-in's slot uses for blocks the plug-in is not called for, the [`scan_cache`] that
//! remembers what each plug-in file held, and the scan's per-class [`probe`] phase (ADR 0064).
//! Safe code only; the format crates keep the FFI.

pub mod child;
pub mod gate;
pub mod probe;
pub mod scan_cache;

//! Code the CLAP and VST3 hosts share (issue #736, package P12): the fade [`gate`] a hosted
//! plug-in's slot uses for blocks the plug-in is not called for, and the [`scan_cache`] that
//! remembers what each plug-in file held. Safe code only; the format crates keep the FFI.

pub mod gate;
pub mod scan_cache;

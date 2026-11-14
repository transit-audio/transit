//! Audio backend adapters (ADR 0016). The Player opens streams only through the
//! `transit_audio_backend` traits: cpal's adapter is here, the native backends are their own crates
//! (`transit-audio-wasapi`, `transit-audio-asio`), chosen by [`native::factory_for`]. The silent
//! device of a `--null-audio` run ([`null`]) is a backend of its own, chosen by nothing else.

pub mod cpal;
#[cfg(test)]
mod cpal_convert_golden;
pub mod native;
pub mod null;

/// The doors from the Player to a system audio backend ([`native::factory_for`] and
/// [`cpal::CpalFactory`]) count each use on the calling thread in test builds, so a test proves a
/// path never reaches one (the silent device opens and runs without either).
#[cfg(test)]
pub(crate) mod asks {
    use std::cell::Cell;
    thread_local! {
        static ASKS: Cell<usize> = const { Cell::new(0) };
    }
    pub(crate) fn count() {
        ASKS.with(|n| n.set(n.get() + 1));
    }
    /// The uses on this thread so far.
    pub(crate) fn seen() -> usize {
        ASKS.with(Cell::get)
    }
}
#[cfg(all(test, windows))]
mod wasapi_names;

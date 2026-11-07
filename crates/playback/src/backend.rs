//! Audio backend adapters (ADR 0016). The Player opens streams only through the
//! `transit_audio_backend` traits: cpal's adapter is here, the native backends are their own crates
//! (`transit-audio-wasapi`, `transit-audio-asio`), chosen by [`native::factory_for`].

pub mod cpal;
#[cfg(test)]
mod cpal_convert_golden;
pub mod native;
#[cfg(all(test, windows))]
mod wasapi_names;

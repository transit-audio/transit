//! The silent device's Player (`--null-audio`): the one open path ([`Player::open_with`]) over the
//! silent backend ([`crate::backend::null`]), so everything above the device runs as on a real one.
//! Nothing here asks a system audio backend: not [`crate::backend::native::factory_for`], not the
//! cpal factory, and not the input factories ([`Player::open_input`] refuses first).

use super::*;
use crate::backend::null::NullBackend;
use crate::monitor_path::MonitorSettings;

impl Player {
    /// The Player of a `--null-audio` run: the silent device at 48 kHz, stereo, 512 frames, paced
    /// at real time, its output discarded; no input and no MIDI ports. Should the open itself fail,
    /// the silent runtime runs with the error, still marked as the silent device.
    pub fn with_null_device(session: Arc<Session>, monitor: &MonitorSettings) -> Self {
        Self::open_null(session, &NullBackend::new(), monitor)
    }

    /// [`Self::with_null_device`] paced by `clock` (tests step it: [`crate::backend::null::StepClock`]).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn with_null_device_clock(session: Arc<Session>, clock: Arc<dyn crate::backend::null::PaceClock>) -> Self {
        Self::open_null(session, &NullBackend::new().with_clock(clock), &MonitorSettings::default())
    }

    fn open_null(session: Arc<Session>, backend: &NullBackend, monitor: &MonitorSettings) -> Self {
        let mut player = match Self::open_with(Arc::clone(&session), &AudioConfig::default(), backend, monitor) {
            Ok(player) => player,
            Err(error) => {
                log::warn!("the silent device did not open: {error}");
                Self::silent_runtime(session, error)
            }
        };
        player.null_device = true;
        player
    }
}

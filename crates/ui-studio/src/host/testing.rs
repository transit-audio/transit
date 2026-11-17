//! The window-free adapter (ADR 0062 section 2, slice S2b; test only): an app on the device-free
//! Player, stepped one audio block and one host step at a time on a stepped clock, with no window
//! and no wall clock.
#![allow(clippy::expect_used)]

use crate::{Services, StudioApp};
use std::time::Duration;

/// A host step's frame interval on the stepped clock.
pub(crate) const FRAME: Duration = Duration::from_nanos(16_666_667);

/// An app on the device-free Player (no audio device, no window), with its stepped clock.
pub(crate) struct DeviceFree {
    pub(crate) app: StudioApp,
    frames: u32,
}

/// A new app on the device-free Player over the default session. Nothing is written outside the
/// test (no autosave folder is set).
pub(crate) fn device_free_app() -> DeviceFree {
    let engine = transit_engine::Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).expect("the device-free Player");
    DeviceFree { app: StudioApp::new(engine, Some(player), Services::default()), frames: 0 }
}

impl DeviceFree {
    /// The stepped clock's time after `frames` host steps.
    pub(crate) fn now(frames: u32) -> Duration {
        FRAME * frames
    }

    /// Host steps taken so far.
    pub(crate) fn frames(&self) -> u32 {
        self.frames
    }

    /// One audio block on the device-free Player, then one host step.
    pub(crate) fn step(&mut self) {
        if let Some(player) = &self.app.player {
            player.step_device_free();
        }
        self.frames += 1;
        let now = Self::now(self.frames);
        self.app.step_host(now.as_secs_f64(), FRAME.as_secs_f64(), now);
    }
}

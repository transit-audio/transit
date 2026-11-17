//! The host's runtime-fault entry point (ADR 0062 section 4): when a Stop the host owes stays
//! unsent past its deadline, the host faults the runtime before its fault cleanup. Unused until
//! the lifecycle activates (slice S2d).

use super::Player;
use std::sync::atomic::Ordering;

impl Player {
    /// Fault the runtime as a device failure does: publish the runtime failure signal (closing
    /// admission for Main and the holder) and the device-gone flag (the next callback faults its
    /// audio and renders silence). Safe to call while the owner or the audio holder is borrowed:
    /// it borrows neither, only stores two atomics. Idempotent.
    pub fn fault_runtime(&self) {
        if let Some(signal) = &self.failure_signal {
            signal.fault();
        }
        self.fault.gone.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::super::{DeviceOutput, Player, PlayerError};
    use std::sync::Arc;
    use transit_model::Session;
    use transit_rt::testing::measure;

    #[test]
    fn fault_runtime_closes_admission_without_borrowing_the_owner() {
        let (player, access, _clock) = Player::prepare(Arc::new(Session::default()), 48_000, transit_model::IoMap::default()).unwrap();
        let mut callback = DeviceOutput::new(access, &player, 2);
        callback.fill(&mut [0.0f32; 128]);
        player.service().unwrap();
        player.play(0, None, None).unwrap();
        {
            // Main's owner and the audio holder are both borrowed: the fault still publishes.
            let owner = player.owner().unwrap();
            let held = callback.access.enter().unwrap();
            let ((), heap) = measure(|| player.fault_runtime());
            assert!(heap.is_zero(), "{heap:?}");
            assert!(owner.faulted(), "admission is closed through the runtime signal");
            assert!(held.plans.faulted());
        }
        assert_eq!(player.stop(), Err(PlayerError::Faulted));
        assert_eq!(player.play(0, None, None), Err(PlayerError::Faulted));
        let mut out = [0.75f32; 128];
        callback.fill(&mut out);
        assert_eq!(out, [0.0; 128], "the output is silent");
        assert!(!player.is_playing());
        assert!(player.trouble().is_some(), "the trouble is said once, as a lost device's is");
        player.fault_runtime();
        assert_eq!(player.stop(), Err(PlayerError::Faulted));
    }
}

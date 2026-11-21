//! The MIDI inputs outlive an audio device reopen (#760). The Player being replaced hands its MIDI
//! side over ([`Native::hand_off`]): the device thread with the open input handles, the ingress
//! worker with their consumers, and the per-port state (storm mutes, transport buttons, MIDI Learn)
//! stay; what belongs to the old runtime goes (its routes and sources, and the output thread, whose
//! drop releases what the echo held on external destinations, as a reopen did before). The
//! replacement ([`Native::adopt`]) gives the carried side its own runtime: its action lane, output
//! thread and monitor. Routes are built again at its first service, each with a cutoff taken then,
//! so a note that arrived while no runtime was open is cancelled and counted, never played by the
//! new engine; transport buttons get the same cutoff. Host time has one process-wide origin
//! (`HostOrigin::default`), so arrival stamps need no bridge across the reopen.
use super::*;

/// What ties a Native to its Player's runtime: the action lane into the audio thread, the output
/// thread's echo producer and the monitor's input tap.
pub(super) struct Unbound {
    pub(super) lane: crate::action_lane::Control<ingress::Packet, ActionResult>,
    pub(super) echo: midi_out::Producer,
    pub(super) tap: monitor::Tap,
}

impl Native {
    /// Gives the worker this Native's runtime, once, at the first service (before any route).
    pub(super) fn bind(&mut self) {
        let Some(Unbound { lane, echo, tap }) = self.unbound.take() else { return };
        let bound = self.worker.set_lane(lane).and_then(|()| self.worker.set_echo(echo)).and_then(|()| self.worker.set_monitor(tap));
        if let Err(error) = bound {
            self.status.error = Some(error.into());
        }
    }

    /// Detaches this Native from the runtime of the Player being replaced. Every open input stays
    /// open and attached to the worker; its routes are revoked here (the worker routes nothing until
    /// the replacement's routes arrive), the old runtime's sources are forgotten with it, and the
    /// output thread closes now, with the old device.
    pub(in crate::player) fn hand_off(&mut self) {
        for input in &mut self.inputs {
            for binding in input.routes.drain(..) {
                binding.port.invalidate();
            }
            input.routes_applied = None;
            input.dirty = true;
        }
        self.retired.clear();
        let (idle, _, _) = midi_out::Out::new();
        drop(std::mem::replace(&mut self.out, idle));
        self.playback = None;
    }

    /// The Native the replacement Player installs: `carried` on `fresh`'s runtime, or `fresh` when
    /// nothing can be carried (the carried device thread or worker ended, or the replacement runs
    /// no MIDI: not enabled, as for the silent diagnostic device, or already serviced). Dropping
    /// `carried` closes its inputs, as every reopen did before.
    pub(in crate::player) fn adopt(mut fresh: Native, mut carried: Native) -> Native {
        let alive = carried.io.as_ref().is_some_and(|io| !io.finished.load(Ordering::Acquire)) && !carried.worker.finished();
        if fresh.io.is_none() || fresh.unbound.is_none() || !alive {
            return fresh;
        }
        let (idle, _, _) = midi_out::Out::new();
        carried.out = std::mem::replace(&mut fresh.out, idle);
        carried.playback = fresh.playback.take();
        carried.monitor = std::mem::replace(&mut fresh.monitor, monitor::Hub::new().0);
        carried.unbound = fresh.unbound.take();
        // The fresh worker and device thread hold nothing yet; they end here.
        drop(fresh);
        // A transport button pressed while no runtime was open is not acted on (as a closed port's
        // was not): the same cutoff a configuration change takes.
        carried.cancel_buttons();
        carried.status.error = None;
        let kept = carried.inputs.iter().filter(|input| input.attached.is_some()).count();
        log::info!("MIDI: {kept} open inputs kept across the audio device reopen");
        carried
    }
}

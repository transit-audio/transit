//! The MIDI inputs outlive an audio device reopen (#760). The Player being replaced hands its MIDI
//! side over ([`Native::hand_off`]): the device thread with the open input handles, the ingress
//! worker with their consumers, and the per-port state (storm mutes, transport button settings, the
//! MIDI Learn list) stay; what belongs to the old runtime goes (its routes and sources, and the
//! output thread, whose drop releases what the echo held on external destinations, as a reopen did
//! before). Inputs that are not open (a failed open, an open still in flight) are let go, so the
//! replacement tries them again, as a reopen always did.
//!
//! From the handover on, the worker matches, learns and captures nothing. The replacement
//! ([`Native::adopt`]) gives the carried side its own action lane, output thread and monitor, sent
//! to the worker at its first service ([`Native::bind`]); a full worker queue keeps what was not
//! taken for the next service, and no route of the new runtime is made before all of it went
//! through. Then the boundary is drawn, once, for everything that arrived while no runtime was open:
//! each input's cutoff is taken, the routes of that service cancel what precedes it (notes), the
//! transport buttons take it, the worker skips what precedes it for MIDI Learn, mapped controls and
//! capture, and what the worker published before the boundary is dropped. The mappings are renewed
//! as a replacement's (a new generation), so the interface treats the Player as new: its held
//! button latches are cleared and an armed Learn is armed again. Host time has one process-wide
//! origin (`HostOrigin::default`), so arrival stamps need no bridge across the reopen.
use super::*;
use std::collections::VecDeque;

/// What ties a Native to its Player's runtime and is not yet with the worker.
pub(super) struct Bind {
    /// Updates for the worker not yet taken, in order: the action lane into the audio thread, the
    /// output thread's echo producer and the monitor's input tap, then a boundary's mappings and
    /// its resume.
    queue: VecDeque<ingress::Update>,
    /// A carried side's handover whose boundary is not drawn yet.
    handover: Option<u64>,
}

impl Bind {
    pub(super) fn new(lane: crate::action_lane::Control<ingress::Packet, ActionResult>, echo: midi_out::Producer, tap: monitor::Tap) -> Self {
        let queue = VecDeque::from([ingress::Update::Lane(Box::new(lane)), ingress::Update::Echo(echo), ingress::Update::Monitor(tap)]);
        Self { queue, handover: None }
    }
}

impl Native {
    /// Whether everything that ties this Native to its runtime is with the worker now. Called at
    /// every service until it is; nothing waits.
    pub(super) fn bind(&mut self) -> bool {
        let Some(mut bind) = self.unbound.take() else { return true };
        let done = self.send(&mut bind) && self.boundary(&mut bind) && self.send(&mut bind);
        if !done {
            if self.worker.finished() {
                self.status.error = Some("MIDI devices stopped; reconnect audio to reopen MIDI".into());
            }
            self.unbound = Some(bind);
        }
        done
    }

    fn send(&mut self, bind: &mut Bind) -> bool {
        while let Some(update) = bind.queue.pop_front() {
            if let Err(update) = self.worker.offer(update) {
                bind.queue.push_front(update);
                return false;
            }
        }
        true
    }

    /// Draws a carried side's boundary once the worker has taken the suspension up.
    fn boundary(&mut self, bind: &mut Bind) -> bool {
        let Some(token) = bind.handover else { return true };
        if self.worker.suspend_seen() < token {
            return false;
        }
        drop(self.worker.take_map_events());
        drop(self.wanted.take_moves());
        self.wanted.renew();
        let cutoffs: Vec<(u64, u64)> =
            self.inputs.iter().filter_map(|input| input.attached.as_ref().map(|(id, open)| (*id, open.cutoff()))).collect();
        for (id, cutoff) in &cutoffs {
            self.buttons.cutoffs.insert(*id, *cutoff);
        }
        self.floor.clone_from(&cutoffs);
        // The renewed mappings reach the worker before it resumes matching.
        if let Some(snapshot) = self.wanted.rebuild(self.map_ids()) {
            bind.queue.push_back(ingress::Update::Maps(snapshot));
        }
        bind.queue.push_back(ingress::Update::Resume { token, cutoffs });
        bind.handover = None;
        true
    }

    /// Between a handover and the replacement's boundary: what arrives has no effect.
    pub(super) fn before_boundary(&self) -> bool {
        self.unbound.as_ref().is_some_and(|bind| bind.handover.is_some())
    }

    /// Detaches this Native from the runtime of the Player being replaced. Every open input stays
    /// open and attached to the worker; its routes are revoked here (the worker routes nothing until
    /// the replacement's routes arrive), the worker stops matching, learning and capturing, the old
    /// runtime's sources are forgotten, inputs that are not open are let go, and the output thread
    /// closes now, with the old device.
    pub(in crate::player) fn hand_off(&mut self) {
        self.handovers += 1;
        self.worker.suspend(self.handovers);
        let io = self.io.as_ref();
        self.inputs.retain_mut(|input| {
            for binding in input.routes.drain(..) {
                binding.port.invalidate();
            }
            input.routes_applied = None;
            input.dirty = true;
            if input.attached.is_some() {
                return true;
            }
            if !input.closed
                && let Some(io) = io
            {
                let _ = io.request(Request::Close(input.id));
            }
            false
        });
        self.retired.clear();
        self.floor.clear();
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
        let fresh_bind = fresh.unbound.as_ref().is_some_and(|bind| bind.handover.is_none() && bind.queue.len() == 3);
        if fresh.io.is_none() || !fresh_bind || !alive {
            return fresh;
        }
        let (idle, _, _) = midi_out::Out::new();
        carried.out = std::mem::replace(&mut fresh.out, idle);
        carried.playback = fresh.playback.take();
        carried.monitor = std::mem::replace(&mut fresh.monitor, monitor::Hub::new().0);
        carried.unbound = fresh.unbound.take().map(|bind| Bind { handover: Some(carried.handovers), ..bind });
        // The fresh worker and device thread hold nothing yet; they end here.
        drop(fresh);
        carried.status.error = None;
        let kept = carried.inputs.len();
        log::info!("MIDI: {kept} open inputs kept across the audio device reopen");
        carried
    }
}

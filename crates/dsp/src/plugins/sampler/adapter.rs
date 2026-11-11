//! The core path: the audio-thread side of a Sampler instance that plays through
//! `transit-sampler-core` (ADR 0041). It owns the [`SamplerCore`], the current and retained patch
//! generations, the slot parameters and the event queue; each block it adopts a waiting generation
//! when it can, applies changed slot parameters, hands the core the events and the borrowed
//! sources, and retires generations no voice reads any more. Nothing here allocates, frees, locks
//! or waits after [`CorePath::new`] (the probe in `tests/sampler_core_alloc.rs`).
//!
//! What the transport sends: when playback stops, a `Stop` before the first stopped block (every
//! voice fades over 32 frames and the pedal lifts; the ADR requires only that one-shots end, the
//! fuller fade also ends held notes, as transport stops do in Ableton and Logic). A locate or a
//! loop wrap while playing sends nothing. While playing, the song position at the block's first
//! frame goes to the core, which interpolates it inside the block (see the block-size test).
//!
//! Slot level and pan changes glide: a changed `zoneNNN_level` or `zoneNNN_pan` ramps linearly
//! across the block it applies in, from the old value to the new, every frame by the same
//! increment, the same for every voice of the zone (`SamplerCore::set_zone_glide`). The dsp crate's
//! other smoothed parameters (a plug-in's output level, through `util::Smoother`) glide with a
//! one-pole filter on the plug-in's own output; a zone's level and pan are per-voice gains inside
//! the core, rendered voice by voice, so the glide is keyed to absolute frames instead, which
//! needs no per-zone state stepped in frame order. The other slot parameters (the shaper and the
//! matrix) apply at the block start, as the Macro Oscillator's do.

use transit_sampler_core::event::{Context, Event, EventKind};
use transit_sampler_core::patch::PlayMode;
use transit_sampler_core::reader::RegionReader;
use transit_sampler_core::{SamplerCore, ZONES};

use super::bank::Banks;
use super::events::CoreQueue;
use super::handoff::{self, AudioEnd, Generation, Publisher};
use super::pads::PadWords;
use super::params::SlotParams;
use crate::Transport;
use crate::util::frames_in;

pub(super) struct CorePath {
    core: SamplerCore,
    seed: u32,
    end: AudioEnd,
    current: Option<Box<Generation>>,
    retained: [Option<Box<Generation>>; 2],
    params: SlotParams,
    queue: CoreQueue,
    /// What each pad did, for the Pads view (`pads.rs`).
    pads: PadWords,
    /// Every zone's controls must be rebuilt (the rate or the bar changed).
    all_dirty: bool,
    stop_due: bool,
    playing: bool,
    /// Frames since the last reset (the core's absolute frame).
    frame: u64,
    tempo: f64,
    quarters: Option<f64>,
    bar_quarters: f64,
    bar_origin: (i64, f64),
    sr: f32,
}

impl CorePath {
    /// The core path at rate `sr` with the voices' random state from `seed`, and the control side's
    /// end of its handoff. Allocates everything it will use.
    pub(super) fn new(seed: u32, sr: f32) -> (CorePath, Publisher) {
        let (publisher, end) = handoff::channel();
        let mut core = SamplerCore::new();
        core.prepare(sr, seed);
        let path = CorePath {
            core,
            seed,
            end,
            current: None,
            retained: [None, None],
            params: SlotParams::new(),
            queue: CoreQueue::new(),
            pads: PadWords::new(),
            all_dirty: false,
            stop_due: false,
            playing: false,
            frame: 0,
            tempo: 120.0,
            quarters: None,
            bar_quarters: 4.0,
            bar_origin: (0, 0.0),
            sr,
        };
        (path, publisher)
    }

    /// A new session rate: the core works its durations out again and every zone's shaper settings
    /// are rebuilt at the rate before the next block.
    pub(super) fn prepare(&mut self, sr: f32) {
        self.sr = sr;
        self.core.prepare(sr, self.seed);
        self.all_dirty = true;
        self.reset();
    }

    pub(super) fn reset(&mut self) {
        self.core.reset();
        self.queue.clear();
        self.stop_due = false;
        self.playing = false;
        self.frame = 0;
        self.quarters = None;
    }

    pub(super) fn set_transport(&mut self, t: &Transport) {
        self.tempo = if t.tempo.is_finite() && t.tempo > 0.0 { t.tempo.clamp(1.0, 999.0) } else { 120.0 };
        let playing = t.playing && t.quarters.is_finite();
        if self.playing && !playing {
            self.stop_due = true;
        }
        self.playing = playing;
        self.quarters = playing.then_some(t.quarters);
        let bar = if t.numerator > 0 && t.denominator > 0 { f64::from(t.numerator) * 4.0 / f64::from(t.denominator) } else { 4.0 };
        let bar = bar.clamp(0.0625, 256.0);
        if bar != self.bar_quarters || self.bar_origin != (t.bar, t.bar_start) {
            self.bar_quarters = bar;
            self.bar_origin = (t.bar, t.bar_start);
            self.all_dirty = true;
        }
    }

    /// Queues a host event; `false` when the queue refused it.
    pub(super) fn push(&mut self, offset: usize, kind: EventKind) -> bool {
        self.queue.push(offset, kind)
    }

    pub(super) fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.params.set(id, value)
    }

    pub(super) fn param(&self, id: &str) -> Option<f32> {
        self.params.get(id)
    }

    /// Renders a block into the first `ch` channels of `io` (a third channel and beyond get the
    /// mean of left and right).
    pub(super) fn process(&mut self, io: &mut [Vec<f32>], frames: usize, ch: usize) {
        let ch = ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        self.take_controls();
        self.adopt();
        self.apply_params(frames);
        let CorePath { core, current, retained, queue, pads, .. } = self;
        let [r0, r1] = retained;
        let banks = Banks { banks: [current.as_deref().map(|g| &g.bank), r0.as_deref().map(|g| &g.bank), r1.as_deref().map(|g| &g.bank)] };
        let ctx = Context { frame: self.frame, tempo: self.tempo, quarters: self.quarters };
        if self.stop_due {
            // Before the block's own events, at its first frame, without rendering.
            core.process(&ctx, &banks, &[Event::new(0, EventKind::Stop)], &mut []);
            self.stop_due = false;
        }
        let events = queue.sorted();
        // Before the core renders: the pads as they stand and the voices playing at the block start.
        pads.observe(core, events);
        let rendered = match (ch, &mut *io) {
            (0, _) | (_, []) => {
                core.process(&ctx, &banks, events, &mut []);
                0
            }
            (1, [l, ..]) | (_, [l]) => {
                core.process(&ctx, &banks, events, &mut [l.get_mut(..frames).unwrap_or_default()]);
                frames
            }
            (_, [l, r, ..]) => {
                core.process(&ctx, &banks, events, &mut [l.get_mut(..frames).unwrap_or_default(), r.get_mut(..frames).unwrap_or_default()]);
                frames
            }
        };
        queue.clear();
        if let [l, r, rest @ ..] = io {
            for c in rest.iter_mut().take(ch.saturating_sub(2)) {
                for ((x, a), b) in c.iter_mut().zip(l.iter()).zip(r.iter()).take(frames) {
                    *x = (a + b) * 0.5;
                }
            }
        }
        self.frame = self.frame.wrapping_add(rendered as u64);
        self.retire();
    }

    /// Stores the slot values the control side sent ([`Publisher::set_control`]) as `set_param`
    /// stores them. Bounded by the ring's capacity; no allocation.
    fn take_controls(&mut self) {
        while let Ok(c) = self.end.controls.pop() {
            self.params.set_index(usize::from(c.zone), usize::from(c.field), c.value);
        }
    }

    /// Takes the waiting generation when a retained slot is free for the current one.
    fn adopt(&mut self) {
        let Some(free) = self.retained.iter().position(Option::is_none) else { return };
        let Ok(mut g) = self.end.publications.pop() else { return };
        for (z, zone) in g.zones.iter_mut().enumerate() {
            if let Some(zone) = zone {
                self.params.compose(z, zone, self.bar_quarters, self.bar_origin, self.sr);
            }
        }
        // Every zone of the new patch carries its slot values now.
        let _ = self.params.take_dirty();
        let outgoing = match self.core.set_patch(&g.zones, &g.pads) {
            Ok(()) => {
                self.core.set_group_modes(&g.groups);
                self.current.replace(g)
            }
            // Checked before publication, so not expected: the old patch keeps playing and the
            // refused generation goes back.
            Err(_) => Some(g),
        };
        if let Some(slot) = self.retained.get_mut(free) {
            *slot = outgoing;
        }
    }

    /// Hands the core every zone whose slot parameters changed (all of them after a rate or bar
    /// change); a level or pan change glides over the block's `frames`.
    fn apply_params(&mut self, frames: usize) {
        let dirty = self.params.take_dirty();
        let all = std::mem::take(&mut self.all_dirty);
        let Some(g) = self.current.as_deref_mut() else { return };
        for (z, zone) in g.zones.iter_mut().enumerate().take(ZONES) {
            let marked = dirty.get(z / 64).is_some_and(|w| w >> (z % 64) & 1 == 1);
            if let Some(zone) = zone
                && (all || marked)
            {
                self.params.compose(z, zone, self.bar_quarters, self.bar_origin, self.sr);
                // Same region: no fade; checked when it was published.
                let _ = self.core.set_zone_glide(z, *zone, self.frame, u32::try_from(frames).unwrap_or(u32::MAX));
            }
        }
    }

    /// Sends back each retained generation whose sources no sounding voice reads only through it.
    fn retire(&mut self) {
        for i in 0..self.retained.len() {
            let needed = {
                let Some(g) = self.retained.get(i).and_then(Option::as_deref) else { continue };
                let others = |id: u16| {
                    self.current.as_deref().is_some_and(|c| c.holds(id))
                        || self.retained.iter().enumerate().any(|(j, r)| j != i && r.as_deref().is_some_and(|r| r.holds(id)))
                };
                self.core.playing_sources().any(|id| g.holds(id) && !others(id))
            };
            if needed {
                continue;
            }
            if let Some(g) = self.retained.get_mut(i).and_then(Option::take)
                && let Err(g) = self.end.retire(g)
                && let Some(slot) = self.retained.get_mut(i)
            {
                *slot = Some(g);
            }
        }
    }

    /// How long a note can sound after its key-up: the longest release among the zones (the
    /// shaper's amp release run on to silence when it is on), plus the whole region for a one-shot.
    pub(super) fn tail_samples(&self) -> usize {
        let Some(g) = self.current.as_deref() else { return 0 };
        let sr = f64::from(self.sr);
        let longest = g
            .zones
            .iter()
            .enumerate()
            .filter_map(|(i, z)| z.as_ref().map(|z| (i, z)))
            .map(|(i, z)| {
                let p = &z.playback;
                // The shaper's release time is to -60 dB; it ends at -100 dB (as the Macro Oscillator's tail).
                let shaped = if z.shaper.on { self.params.amp_release_ms(i) * 100.0 / 60.0 } else { 0.0 };
                let rest = if p.mode == PlayMode::OneShot {
                    let rate = g.bank.get(usize::from(z.region.source)).map_or(sr, |s| f64::from(RegionReader::sample_rate(s)));
                    (z.region.end - z.region.start) as f64 * sr / rate.max(1.0)
                } else {
                    0.0
                };
                f64::from(p.release_ms.max(shaped)) * sr / 1000.0 + rest
            })
            .fold(0.0f64, f64::max);
        if longest.is_finite() { longest.min(1.0e9) as usize } else { 0 }
    }

    /// Each pad's [`crate::PadWord`] as of the last block.
    pub(super) fn pad_words(&self) -> &[u32; transit_sampler_core::PADS] {
        self.pads.words()
    }

    /// Each sounding voice's [`crate::VoicePos`] as of the last block into `out`; how many.
    pub(super) fn voice_positions(&self, out: &mut [u64; crate::VOICE_SLOTS]) -> usize {
        let mut n = 0;
        for (slot, p) in out.iter_mut().zip(self.core.voice_positions()) {
            *slot = crate::VoicePos::new(p.zone, p.phase).0;
            n += 1;
        }
        n
    }

    /// The state fingerprint (feature `testing`): the core's, the adapter's frame counter.
    #[cfg(feature = "testing")]
    pub(super) fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        self.core.fingerprint(h);
        h.write_u64(self.frame);
    }

    /// The core, for the crate's tests.
    #[cfg(test)]
    pub(super) fn core(&self) -> &SamplerCore {
        &self.core
    }

    /// Generations held on the audio side: (current, retained).
    #[cfg(test)]
    pub(super) fn held(&self) -> (bool, usize) {
        (self.current.is_some(), self.retained.iter().flatten().count())
    }

    #[cfg(test)]
    pub(super) fn queued(&self) -> usize {
        self.queue.len()
    }
}

//! Sampler: transit's built-in sample instrument (ADR 0041), one plug-in (`SAMPLER_INFO`, id
//! `sampler`) with two render paths.
//!
//! - **Legacy path** (`legacy.rs`): plays one recorded sample across the keyboard, 32 voices. Each
//!   note plays the sample pitched from its root note (an octave up plays twice as fast) with
//!   4-point cubic interpolation, which also converts the sample's own rate to the host's. Key and
//!   velocity ranges, an amp ADSR with velocity sensitivity, a sample start point, forward or
//!   ping-pong loops, and gate or one-shot playback, set by the 19 parameters. The host hands the
//!   audio over with [`Plugin::set_sample`]; without one the Sampler is silent. Every instance
//!   plays here unless switched (`Insert::sample` sessions), bit for bit as the v0.6.2 release
//!   (`tests/sampler_legacy_golden.rs`).
//! - **Core path** (`adapter.rs`): an instance switched with [`Sampler::enable_core`] plays through
//!   `transit-sampler-core` (zones, 64 pads in four banks, 64 voices, the per-voice shaper and
//!   matrix). The samples are a [`PreparedBank`] of `Arc`s by stable source id, lent to the core as
//!   [`Source`] views (`bank.rs`); host events become core events in a fixed array of 1024 with the
//!   release reserve (`events.rs`); each zone slot has fixed parameter ids (`zone001_level`,
//!   `zone001_pan`, the shaper and `mod` ids behind the prefix; `params.rs`), built into the zone's
//!   settings at the session rate. The unprefixed ids keep addressing the legacy renderer.
//! - **Handoff** (`handoff.rs`): the control side publishes a [`PatchSpec`] through the
//!   [`Publisher`]; it is checked whole and prepared off the audio thread as a generation. Two
//!   publication slots (the current generation and at most one pending; edits coalesce on the
//!   control side), two retirement slots and four bank-generation credits ([`CREDITS`]). At a block
//!   boundary the audio thread adopts the pending generation only when a retained slot is free for
//!   the old one; voices of replaced zones fade over 32 frames still reading the retained
//!   generation, which goes back through the retirement queue once no sounding voice reads it (a
//!   full queue keeps it retained and holds further adoption). The control side drops retired
//!   generations: no `Arc` drop, free, lock or allocation on the audio thread.
//!
//! **Sessions and ownership.** An instrument whose `Insert::sampler_patch` is present plays the
//! core path: the mix's plan compiler, which makes every instance on the control side, switches
//! a fresh instance with [`Plugin::enable_sampler_core`] before it first processes and keeps its
//! [`Publisher`] (one per instance, keyed by the insert's uid), so no other thread ever holds it.
//! It publishes the patch's structure (sources, zones' regions and playback, pads, choke and
//! layer groups) and sends the controls through the slot parameters: on a fresh instance with
//! `set_param` before the first block, on a running one through [`Publisher::set_control`],
//! whose values the audio thread stores at the next block start exactly as `set_param` does.
//! It calls [`Publisher::collect`] on the control side's service tick (the plan bridge's poll)
//! and at every compile. A structural change to a running instance is a new publication; a mode
//! change (a patch made or removed, or a new seed) is a fresh instance. A legacy instance never
//! switches on its own: opening or saving an old session makes no patch.
//!
//! **The control table.** The slot values live in two places outside the callback: the model's
//! `SamplerPatch::controls` (the sparse map of nondefault values the engine's commands and a
//! load write and a save reads) and the instance's own table (`params.rs`), which the compiler
//! keeps equal by sending only the entries that changed. The slot ids are not in
//! `SAMPLER_INFO.params` or the mix's inline parameter cache, and they are not automatable yet:
//! the automation lanes would have to learn 28,928 ids (ADR 0041 "Capacities and admission").

use std::sync::Arc;

use crate::event::PluginEvent;
use crate::params::Params;
use crate::plugins::instrument_queue::{AdsrRates, EventKind, EventQueue, run_block};
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, Plugin, PluginInfo, SampleSource, Transport};
use transit_sampler_core::event::EventKind as CoreKind;

mod adapter;
mod bank;
mod events;
mod handoff;
mod legacy;
mod params;

use adapter::CorePath;
pub use bank::{CREDITS, PreparedBank, SOURCE_IDS, Source};
pub use handoff::{CONTROL_RING, PatchSpec, PublishError, Publisher};
use legacy::{Derived, RELEASE, SAMPLER_PARAMS, VOICES, Voice};
pub use params::{FIELD_LEVEL, FIELD_PAN, ZONE_FIELDS, field_info, resolve, slot_id};
pub use transit_sampler_core::patch::{Membership, Pad, PatchError, PlayMode, Playback, Zone};
pub use transit_sampler_core::playhead::{LoopMode, Region};
pub use transit_sampler_core::{PADS, ZONES};

/// Checks a spec as [`Publisher::publish`] will before it prepares anything (counts, sources,
/// regions, the zones and pads as the core checks them), without publishing: a command validates
/// an edit with it before it changes the document.
pub fn check_spec(spec: &PatchSpec) -> Result<(), PublishError> {
    handoff::check(spec).map(|_| ())
}

pub static SAMPLER_INFO: PluginInfo = PluginInfo {
    id: "sampler",
    name: "Sampler",
    short_name: "Sampler",
    category: Category::Instrument,
    params: &SAMPLER_PARAMS,
    is_instrument: true,
    audiosuite: false,
};

/// Plays a sample across the keyboard.
pub struct Sampler {
    p: Params,
    d: Derived,
    sr: f32,
    ch: usize,
    sample: Option<Arc<dyn SampleSource>>,
    /// Frames in the sample (0 = nothing to play).
    len: usize,
    /// The sample's own rate.
    sample_sr: f32,
    voices: [Voice; VOICES],
    events: EventQueue,
    rates: AdsrRates,
    level: Smoother,
    /// The core path (`None`: the legacy renderer plays).
    core: Option<Box<CorePath>>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    /// Switches this instance to the core path (ADR 0041) and returns the control side of its
    /// patch handoff: from now on it plays the generations the [`Publisher`] publishes, through
    /// `transit-sampler-core`, with the voices' random state from `seed`; until the first one is
    /// adopted it is silent. The legacy sample and the 19 legacy parameters stay stored but no
    /// longer sound. Control side only: it allocates the core, the queues and the slot parameters.
    pub fn enable_core(&mut self, seed: u32) -> Publisher {
        let (path, publisher) = CorePath::new(seed, self.sr);
        self.core = Some(Box::new(path));
        publisher
    }

    /// Whether the instance plays through the core path.
    #[must_use]
    pub fn uses_core(&self) -> bool {
        self.core.is_some()
    }
}

impl Plugin for Sampler {
    fn info(&self) -> &'static PluginInfo {
        &SAMPLER_INFO
    }

    /// The 19 legacy ids first (their legacy meanings), then, on the core path, the zone slot ids.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if self.p.set(id, value).is_some() {
            self.update();
            true
        } else {
            self.core.as_deref_mut().is_some_and(|c| c.set_param(id, value))
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id).or_else(|| self.core.as_deref().and_then(|c| c.param(id)))
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
        if let Some(c) = self.core.as_deref_mut() {
            c.prepare(self.sr);
        }
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.stop();
        }
        self.events.clear();
        self.level.snap();
        if let Some(c) = self.core.as_deref_mut() {
            c.reset();
        }
    }

    fn set_transport(&mut self, transport: &Transport) {
        if let Some(c) = self.core.as_deref_mut() {
            c.set_transport(transport);
        }
    }

    #[cfg(feature = "testing")]
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        if let Some(c) = self.core.as_deref() {
            c.fingerprint(h);
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if let Some(c) = self.core.as_deref_mut() {
            c.process(io, frames, self.ch);
            return;
        }
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        for b in io.iter_mut().take(ch) {
            b.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let evs = self.events.take_sorted();
        run_block(self, &evs, frames, Self::apply, |p, n| {
            let level = p.level.next();
            p.render_frame(io, ch, n, level);
        });
        self.events.restore(evs);
    }

    fn tail_samples(&self) -> usize {
        if let Some(c) = self.core.as_deref() {
            return c.tail_samples();
        }
        let release = ms_to_samples(self.p.v(RELEASE), self.sr);
        // A one-shot keeps playing its sample after the note ends.
        let rest = if self.d.one_shot { self.len as f32 * self.sr / self.sample_sr } else { 0.0 };
        let t = release + rest;
        if t.is_finite() { t.min(1.0e9) as usize } else { 0 }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(offset, CoreKind::NoteOn { channel: 0, key: note.min(127), velocity: velocity.min(127), id: -1 });
            return;
        }
        self.events.push(offset, EventKind::On(0, note.min(127), velocity.min(127), -1));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(offset, CoreKind::NoteOff { channel: 0, key: note.min(127), velocity: 64, id: -1 });
            return;
        }
        self.events.push(offset, EventKind::Off(0, note.min(127), -1));
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(c) = self.core.as_deref_mut()
            && let Some(i) = events::from_midi(msg)
        {
            c.push(offset, i);
        }
    }

    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if let Some(c) = self.core.as_deref_mut() {
            return events::from_event(ev).is_none_or(|i| c.push(offset as usize, i));
        }
        EventKind::from_event(ev).is_none_or(|kind| self.events.push(offset as usize, kind))
    }

    fn all_notes_off(&mut self) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(0, CoreKind::AllNotesOff);
            return;
        }
        self.events.push(0, EventKind::AllOff);
    }

    fn enable_sampler_core(&mut self, seed: u32) -> Option<Publisher> {
        Some(self.enable_core(seed))
    }

    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        for v in &mut self.voices {
            v.stop();
        }
        self.len = sample.as_deref().map_or(0, |s| if s.num_channels() == 0 { 0 } else { s.frames() });
        let rate = sample.as_deref().map_or(self.sr, |s| s.sample_rate());
        // An unusable rate plays at the host rate rather than not at all.
        self.sample_sr = if rate.is_finite() && rate > 0.0 { rate.min(1_536_000.0) } else { self.sr };
        self.sample = sample;
        true
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod core_tests;

#[cfg(test)]
mod core_tests_c2;

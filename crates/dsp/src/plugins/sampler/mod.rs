//! Sampler: transit's built-in sample instrument (ADR 0041), one plug-in (`SAMPLER_INFO`, id
//! `sampler`) that plays a kit through `transit-sampler-core`. The one-sample legacy renderer and
//! its 19 parameters are retired (ADR 0041 amendment 2026-11-11, #455): an instance the host never
//! switched to the core path has nothing to play and is silent.
//!
//! - **Core path** (`adapter.rs`): an instance switched with [`Sampler::enable_core`] plays through
//!   `transit-sampler-core` (zones, 64 pads in four banks, 64 voices, the per-voice shaper and
//!   matrix). The samples are a [`PreparedBank`] of `Arc`s by stable source id, lent to the core as
//!   [`Source`] views (`bank.rs`); host events become core events in a fixed array of 1024 with the
//!   release reserve (`events.rs`); each zone slot has fixed parameter ids (`zone001_level`,
//!   `zone001_pan`, the shaper and `mod` ids behind the prefix; `params.rs`), built into the zone's
//!   settings at the session rate.
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
//! change (a patch made or removed, or a new seed) is a fresh instance.
//!
//! **The control table.** The slot values live in two places outside the callback: the model's
//! `SamplerPatch::controls` (the sparse map of nondefault values the engine's commands and a
//! load write and a save reads) and the instance's own table (`params.rs`), which the compiler
//! keeps equal by sending only the entries that changed. The slot ids are not in
//! `SAMPLER_INFO.params` or the mix's inline parameter cache, and they are not automatable yet:
//! the automation lanes would have to learn 28,928 ids (ADR 0041 "Capacities and admission").

use crate::event::PluginEvent;
use crate::util::{sane_channels, sane_sr};
use crate::{Category, Plugin, PluginInfo, Transport};
use transit_sampler_core::event::EventKind as CoreKind;

mod adapter;
mod bank;
mod events;
mod handoff;
mod pads;
mod params;

use adapter::CorePath;
pub use bank::{CREDITS, PreparedBank, SOURCE_IDS, Source};
pub use handoff::{CONTROL_RING, PatchSpec, PublishError, Publisher, SILENT_SOURCE};
pub use params::{FIELD_LEVEL, FIELD_PAN, ZONE_FIELDS, field_info, resolve, slot_id};
pub use transit_sampler_core::patch::{Fades, GroupMode, Membership, Pad, PatchError, PlayMode, Playback, Zone};
pub use transit_sampler_core::playhead::{LoopMode, Region};
pub use transit_sampler_core::{GROUPS, PADS, ZONES};

const _: () = assert!(transit_sampler_core::VOICES == crate::VOICE_SLOTS, "every voice has a published place");

/// Checks a spec as [`Publisher::publish`] will before it prepares anything (counts, sources,
/// regions, the zones and pads as the core checks them), without publishing: a command validates
/// an edit with it before it changes the document.
pub fn check_spec(spec: &PatchSpec) -> Result<(), PublishError> {
    handoff::check(spec).map(|_| ())
}

/// The Sampler has no table parameters: its controls are the zone slot ids (`params.rs`).
pub static SAMPLER_INFO: PluginInfo = PluginInfo {
    id: "sampler",
    name: "Sampler",
    short_name: "Sampler",
    category: Category::Instrument,
    params: &[],
    is_instrument: true,
    audiosuite: false,
    pages: &[],
};

/// Plays a kit of samples.
pub struct Sampler {
    sr: f32,
    ch: usize,
    /// The core path (`None`: not switched yet, silent).
    core: Option<Box<CorePath>>,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    #[must_use]
    pub fn new() -> Self {
        Self { sr: 48_000.0, ch: 0, core: None }
    }

    /// Switches this instance to the core path (ADR 0041) and returns the control side of its
    /// patch handoff: from now on it plays the generations the [`Publisher`] publishes, through
    /// `transit-sampler-core`, with the voices' random state from `seed`; until the first one is
    /// adopted it is silent. Control side only: it allocates the core, the queues and the slot
    /// parameters.
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

    /// The zone slot ids, on the core path only.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.core.as_deref_mut().is_some_and(|c| c.set_param(id, value))
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.core.as_deref().and_then(|c| c.param(id))
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        // As before the legacy renderer was retired: the core resets, then prepares at the rate.
        if let Some(c) = self.core.as_deref_mut() {
            c.reset();
            c.prepare(self.sr);
        }
    }

    fn reset(&mut self) {
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
        for b in io.iter_mut().take(self.ch) {
            b.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }

    fn tail_samples(&self) -> usize {
        self.core.as_deref().map_or(0, CorePath::tail_samples)
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(offset, CoreKind::NoteOn { channel: 0, key: note.min(127), velocity: velocity.min(127), id: -1 });
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(offset, CoreKind::NoteOff { channel: 0, key: note.min(127), velocity: 64, id: -1 });
        }
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(c) = self.core.as_deref_mut()
            && let Some(i) = events::from_midi(msg)
        {
            c.push(offset, i);
        }
    }

    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        match self.core.as_deref_mut() {
            Some(c) => events::from_event(ev).is_none_or(|i| c.push(offset as usize, i)),
            None => true,
        }
    }

    fn all_notes_off(&mut self) {
        if let Some(c) = self.core.as_deref_mut() {
            c.push(0, CoreKind::AllNotesOff);
        }
    }

    /// The core path's pads (`pads.rs`); an instance off the core path has none.
    fn pad_activity(&self, out: &mut [u32; crate::PAD_SLOTS]) -> bool {
        let Some(c) = self.core.as_deref() else { return false };
        out.copy_from_slice(c.pad_words());
        true
    }

    /// The core path's sounding voices (the Slices view's playheads, #470).
    fn voice_positions(&self, out: &mut [u64; crate::VOICE_SLOTS]) -> usize {
        self.core.as_deref().map_or(0, |c| c.voice_positions(out))
    }

    fn enable_sampler_core(&mut self, seed: u32) -> Option<Publisher> {
        Some(self.enable_core(seed))
    }
}

#[cfg(test)]
mod core_tests;

#[cfg(test)]
mod core_tests_c2;

#[cfg(test)]
mod pads_tests;

#[cfg(test)]
mod positions_tests;

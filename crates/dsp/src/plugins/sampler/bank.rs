//! The prepared source bank: the `Arc` samples one patch generation plays, by source id, lent to
//! the portable core as [`SliceSource`] views through [`SourceBank`] (ADR 0041: desktop sample
//! ownership is `Arc` in the adapter; the core only borrows readers and never drops a sample).
//!
//! Source ids are stable across generations: the control side gives a sample that the next
//! generation keeps the same id (so an unchanged zone keeps the same region and its voices play
//! on), and gives a new sample an id no live generation holds. Each live generation holds at most
//! [`ZONES`] ids and at most [`CREDITS`] generations live at once, so [`SOURCE_IDS`] ids always
//! suffice. Voices of a replaced zone fade while still reading their old id, which the retained
//! generation answers until the voices are done.

use std::sync::Arc;

use transit_sampler_core::ZONES;
use transit_sampler_core::reader::{RegionReader, SliceSource, SourceBank};

use crate::SampleSource;

/// Bank generations alive at once (ADR 0041 "Patch handoff"): the current one, up to two retained
/// while voices still read them, and one prepared candidate.
pub const CREDITS: usize = 4;

/// The source-id space: every live generation's distinct sources fit without reuse.
pub const SOURCE_IDS: usize = CREDITS * ZONES;

/// One sample as the core reads it.
pub struct Source {
    sample: Arc<dyn SampleSource>,
    frames: u64,
    rate: f32,
}

impl Source {
    fn new(sample: Arc<dyn SampleSource>) -> Source {
        let frames = sample.frames() as u64;
        let rate = sample.sample_rate();
        Source { sample, frames, rate }
    }

    /// The sample as a [`SliceSource`] over its borrowed channels: mono for one channel, stereo
    /// (the shorter channel's length) for two. The core refuses other channel counts by
    /// [`RegionReader::channels`], before it reads.
    #[must_use]
    pub fn view(&self) -> SliceSource<'_> {
        match (self.sample.channel(0), self.sample.channel(1)) {
            (Some(l), Some(r)) => SliceSource::stereo(l, r, self.rate),
            (Some(l), None) => SliceSource::mono(l, self.rate),
            _ => SliceSource::mono(&[], self.rate),
        }
    }

    /// The owning reference (the control side compares identities with it).
    #[must_use]
    pub fn shared(&self) -> &Arc<dyn SampleSource> {
        &self.sample
    }
}

impl RegionReader for Source {
    fn channels(&self) -> usize {
        self.sample.num_channels()
    }

    fn frames(&self) -> u64 {
        self.frames
    }

    fn sample_rate(&self) -> f32 {
        self.rate
    }

    fn sample(&self, channel: usize, frame: u64) -> Option<f32> {
        self.view().sample(channel, frame)
    }
}

/// One generation's sources by id, built off the audio thread. Lookups neither allocate nor free.
pub struct PreparedBank {
    slots: Vec<Option<Source>>,
}

impl Default for PreparedBank {
    fn default() -> Self {
        PreparedBank::new()
    }
}

impl PreparedBank {
    /// An empty bank with room for every id (allocates).
    #[must_use]
    pub fn new() -> PreparedBank {
        PreparedBank { slots: (0..SOURCE_IDS).map(|_| None).collect() }
    }

    /// Puts `sample` at `id`; false when `id` is past [`SOURCE_IDS`].
    pub fn insert(&mut self, id: u16, sample: Arc<dyn SampleSource>) -> bool {
        match self.slots.get_mut(usize::from(id)) {
            Some(slot) => {
                *slot = Some(Source::new(sample));
                true
            }
            None => false,
        }
    }

    /// The source at `id`.
    #[must_use]
    pub fn get(&self, id: usize) -> Option<&Source> {
        self.slots.get(id).and_then(Option::as_ref)
    }

    /// Whether the bank holds `id`.
    #[must_use]
    pub fn holds(&self, id: usize) -> bool {
        self.get(id).is_some()
    }

    /// The ids it holds, in order.
    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.slots.iter().enumerate().filter(|(_, s)| s.is_some()).map(|(i, _)| i as u16)
    }
}

impl SourceBank for PreparedBank {
    fn source(&self, index: usize) -> Option<&dyn RegionReader> {
        self.get(index).map(|s| s as &dyn RegionReader)
    }
}

/// What the core reads in a block on the audio thread: the current generation's bank first, then
/// the retained ones (a fading voice of a replaced zone reads its old id there). Ids mean the same
/// sample in every live generation, so the first bank holding an id answers it.
pub(super) struct Banks<'a> {
    pub(super) banks: [Option<&'a PreparedBank>; 3],
}

impl SourceBank for Banks<'_> {
    fn source(&self, index: usize) -> Option<&dyn RegionReader> {
        self.banks.iter().flatten().find_map(|b| b.source(index))
    }
}

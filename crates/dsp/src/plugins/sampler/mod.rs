//! Sampler: plays one recorded sample across the keyboard, 32 voices.
//!
//! Each note plays the sample pitched from its root note (an octave up plays twice as fast) with
//! 4-point cubic interpolation, which also converts the sample's own rate to the host's. Key and
//! velocity ranges, an amp ADSR with velocity sensitivity, a sample start point, forward or
//! ping-pong loops, and gate or one-shot playback. The host hands the audio over with
//! [`Plugin::set_sample`]; without one the Sampler is silent.

use std::sync::Arc;

use crate::event::PluginEvent;
use crate::params::{Params, param_plumbing};
use crate::plugins::instrument_queue::{AdsrRates, EventKind, EventQueue, run_block};
use crate::util::{Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, Plugin, PluginInfo, SampleSource};

mod legacy;

use legacy::{Derived, RELEASE, SAMPLER_PARAMS, VOICES, Voice};

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
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Sampler {
    param_plumbing!(SAMPLER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.voices {
            v.stop();
        }
        self.events.clear();
        self.level.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
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
        let release = ms_to_samples(self.p.v(RELEASE), self.sr);
        // A one-shot keeps playing its sample after the note ends.
        let rest = if self.d.one_shot { self.len as f32 * self.sr / self.sample_sr } else { 0.0 };
        let t = release + rest;
        if t.is_finite() { t.min(1.0e9) as usize } else { 0 }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.events.push(offset, EventKind::On(0, note.min(127), velocity.min(127), -1));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.events.push(offset, EventKind::Off(0, note.min(127), -1));
    }

    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        EventKind::from_event(ev).is_none_or(|kind| self.events.push(offset as usize, kind))
    }

    fn all_notes_off(&mut self) {
        self.events.push(0, EventKind::AllOff);
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

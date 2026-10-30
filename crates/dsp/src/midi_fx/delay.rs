//! Test-only (feature `testing`): moves every event `samples` later, so a test can watch events
//! cross block edges through the host's carry. Not in the registry and never in the app.

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::params::lin;
use crate::{Category, ParamInfo, PluginInfo, Unit};

static DELAY_PARAMS: [ParamInfo; 1] = [lin("samples", "Samples", 0.0, 48_000.0, 0.0, Unit::None)];

pub static DELAY_INFO: PluginInfo = PluginInfo {
    id: "midi_test_delay",
    name: "Test Delay",
    short_name: "Delay",
    category: Category::Other,
    params: &DELAY_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

#[derive(Clone, Debug, Default)]
pub struct Delay {
    samples: u32,
}

impl MidiEffect for Delay {
    fn info(&self) -> &'static PluginInfo {
        &DELAY_INFO
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = DELAY_INFO.param(id) else { return false };
        self.samples = p.clamp(value).round() as u32;
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        (id == "samples").then_some(self.samples as f32)
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, _frames: u32) {
        for e in input {
            out.push(TimedEvent { offset: e.offset.saturating_add(self.samples), event: e.event });
        }
    }
}

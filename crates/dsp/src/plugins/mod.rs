//! The built-in plugins and their registry.

mod codec_path;
pub mod dynamics;
pub mod eq;
pub mod filter;
pub mod harmonic;
pub mod instrument_queue;
pub mod lfo;
pub mod macro_osc;
pub mod modulator;
mod resample;
pub mod resonator;
pub mod reverb;
pub mod sampler;
pub mod texture;
pub mod time_stretcher;
pub mod tuner;
pub mod utility;

use crate::{Plugin, PluginInfo};

// One table owns menu order and construction; IDs come from the descriptions themselves.
macro_rules! registry {
    ($($info:path => $make:expr),+ $(,)?) => {
        pub(crate) static REGISTRY: &[&PluginInfo] = &[$(&$info),+];

        pub(crate) fn instantiate(id: &str) -> Option<Box<dyn Plugin>> {
            $(if id == $info.id {
                return Some(Box::new($make));
            })+
            None
        }
    };
}

registry! {
    eq::EQ1_INFO => eq::Eq1::new(),
    eq::EQ7_INFO => eq::Eq7::new(),
    filter::FILTER_INFO => filter::Filter::new(),
    dynamics::COMP_INFO => dynamics::Compressor::new(),
    dynamics::GATE_INFO => dynamics::ExpanderGate::new(),
    reverb::ROOM_INFO => reverb::Reverb::room(),
    reverb::PLATE_INFO => reverb::Reverb::plate(),
    harmonic::SAT_INFO => harmonic::Saturator::new(),
    texture::TEXTURE_INFO => texture::TextureFx::new(),
    time_stretcher::TIME_STRETCHER_INFO => time_stretcher::TimeStretcher::new(),
    modulator::MODULATOR_INFO => modulator::CrossModulator::new(),
    utility::GAIN_INFO => utility::Gain::new(),
    utility::TRIM_INFO => utility::Trim::new(),
    utility::INVERT_INFO => utility::Invert::default(),
    utility::DC_INFO => utility::DcRemoval::new(),
    utility::GEN_INFO => utility::SignalGenerator::new(),
    utility::DITHER_INFO => utility::Dither::new(),
    tuner::TUNER_INFO => tuner::Tuner::new(),
    sampler::SAMPLER_INFO => sampler::Sampler::new(),
    macro_osc::MACRO_OSC_INFO => macro_osc::MacroOscillator::new(),
    resonator::RESONATOR_INFO => resonator::ResonatorInstrument::new(),
}

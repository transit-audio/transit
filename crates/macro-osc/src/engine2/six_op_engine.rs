//! 6-operator FM synth with 32 presets.
//!
//! Engine parameters:
//! - *HARMONICS:* preset selection.
//! - *TIMBRE:* modulator(s) level.
//! - *MORPH:* envelope and modulation stretching/time-travel.

// Based on MIT-licensed code (c) 2021 by Emilie Gillet (emilie.o.gillet@gmail.com)

use core::cell::RefCell;
use std::sync::OnceLock;

use crate::engine::{Engine, EngineParameters, TriggerState};
use crate::fm::{
    algorithms::Algorithms,
    lfo::Lfo,
    patch::{Patch, SYX_SIZE},
    voice::{Voice, VoiceParameters},
};
use crate::utils::hysteresis_quantizer::HysteresisQuantizer2;
use crate::utils::random::Random;
use crate::utils::soft_clip;

const NUM_SIX_OP_VOICES: usize = 2;
const NUM_PATCHES_PER_BANK: usize = 32;

static ALGORITHMS: OnceLock<Algorithms<6, 32>> = OnceLock::new();

/// The six-operator engine. Like the firmware, it renders its voices staggered: each block, one
/// voice renders two blocks ahead into an accumulator, so each voice's CPU cost is paid every
/// other block.
#[derive(Debug, Clone)]
pub struct SixOpEngine<'a> {
    rng: Random,
    patches: [Patch; NUM_PATCHES_PER_BANK],
    patch_index_quantizer: HysteresisQuantizer2,
    voice: [FmVoice<'a>; NUM_SIX_OP_VOICES],

    // `temp_buffer_` and `acc_buffer_` in the firmware. The voice renders three buffers of
    // NUM_SIX_OP_VOICES blocks into the scratch: out, aux and one temporary.
    temp_buffer: Box<[f32]>,
    acc_buffer: Box<[f32]>,

    active_voice: usize,
    rendered_voice: usize,
    sample_rate: f32,
}

impl SixOpEngine<'_> {
    pub fn new(block_size: usize) -> Self {
        Self {
            rng: Random::default(),
            patches: core::array::from_fn(|_| Patch::new()),
            patch_index_quantizer: HysteresisQuantizer2::new(),
            voice: core::array::from_fn(|_| FmVoice::new()),
            temp_buffer: vec![0.0; 3 * NUM_SIX_OP_VOICES * block_size].into_boxed_slice(),
            acc_buffer: vec![0.0; NUM_SIX_OP_VOICES * block_size].into_boxed_slice(),
            active_voice: 0,
            rendered_voice: 0,
            sample_rate: 48000.0,
        }
    }

    pub fn load_syx_bank(&mut self, bank: &[u8; 4096]) {
        for (i, patch) in self.patches.iter_mut().enumerate() {
            (*patch).unpack(&bank[i * SYX_SIZE..]);
        }

        for voice in self.voice.iter_mut() {
            voice.unload_patch();
        }
    }
}

impl Engine for SixOpEngine<'_> {
    fn seed(&mut self, seed: u32) {
        self.rng.seed(seed);
    }

    fn init(&mut self, sample_rate_hz: f32) {
        // The voices and the envelope scrubbing run against the firmware's corrected sample rate.
        self.sample_rate = crate::CORRECTED_SAMPLE_RATE * (sample_rate_hz / crate::REF_SAMPLE_RATE);
        self.patch_index_quantizer.init(32, 0.005, false);

        let algorithms = ALGORITHMS.get_or_init(|| {
            let mut algo = Algorithms::<6, 32>::new();
            algo.init();
            algo
        });
        for voice in self.voice.iter_mut() {
            voice.init(algorithms, self.sample_rate);
        }

        self.active_voice = NUM_SIX_OP_VOICES - 1;
        self.rendered_voice = 0;
    }

    fn render(&mut self, parameters: &EngineParameters, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool) {
        let size = out.len();
        let patch_index = self.patch_index_quantizer.process(parameters.harmonics * 1.02) as usize;

        if parameters.trigger.has(TriggerState::UNPATCHED) {
            let t = parameters.morph;
            self.voice[0].mutable_lfo().scrub(&mut self.rng, 2.0 * self.sample_rate * t);

            for i in 0..NUM_SIX_OP_VOICES {
                self.voice[i].load_patch(patch_index, &self.patches[patch_index]);
                let p = self.voice[i].mutable_parameters();
                p.sustain = i == 0;
                p.gate = false;
                p.note = parameters.note;
                p.velocity = parameters.accent;
                p.brightness = parameters.timbre;
                p.envelope_control = t;
                // Read after this voice's patch load, as the firmware does.
                let (pitch_mod, amp_mod) = (self.voice[0].lfo().pitch_mod(), self.voice[0].lfo().amp_mod());
                self.voice[i].set_modulations(pitch_mod, amp_mod);
            }
        } else {
            if parameters.trigger.has(TriggerState::RISING_EDGE) {
                self.active_voice = (self.active_voice + 1) % NUM_SIX_OP_VOICES;
                let active = self.active_voice;
                self.voice[active].load_patch(patch_index, &self.patches[patch_index]);
                self.voice[active].mutable_lfo().reset();
            }
            let active = self.active_voice;
            let p = self.voice[active].mutable_parameters();
            p.note = parameters.note;
            p.velocity = parameters.accent;
            p.envelope_control = parameters.morph;
            self.voice[active].mutable_lfo().step(&mut self.rng, size as f32);

            let active_patch = self.voice[active].patch_index();
            for i in 0..NUM_SIX_OP_VOICES {
                let p = self.voice[i].mutable_parameters();
                p.brightness = parameters.timbre;
                p.sustain = false;
                p.gate = parameters.trigger.has(TriggerState::HIGH) && i == active;
                if self.voice[i].patch_index() != active_patch {
                    self.voice[i].mutable_lfo().step(&mut self.rng, size as f32);
                    let (pitch_mod, amp_mod) = (self.voice[i].lfo().pitch_mod(), self.voice[i].lfo().amp_mod());
                    self.voice[i].set_modulations(pitch_mod, amp_mod);
                } else {
                    let (pitch_mod, amp_mod) = (self.voice[active].lfo().pitch_mod(), self.voice[active].lfo().amp_mod());
                    self.voice[i].set_modulations(pitch_mod, amp_mod);
                }
            }
        }

        // Staggered rendering.
        let span = NUM_SIX_OP_VOICES * size;
        let temp = &mut self.temp_buffer[..3 * span];
        temp[..span - size].copy_from_slice(&self.acc_buffer[..span - size]);
        temp[span - size..span].fill(0.0);
        self.rendered_voice = (self.rendered_voice + 1) % NUM_SIX_OP_VOICES;
        self.voice[self.rendered_voice].render(temp, span);

        for ((out_sample, aux_sample), t) in out.iter_mut().zip(aux.iter_mut()).zip(temp.iter()) {
            *out_sample = soft_clip(*t * 0.25);
            *aux_sample = *out_sample;
        }
        self.acc_buffer[..span - size].copy_from_slice(&temp[size..span]);
    }
}

/// One FM voice with its LFO (`FMVoice` in `engine2/six_op_engine.h`).
#[derive(Debug, Clone)]
pub struct FmVoice<'a> {
    // Which of the engine's patches is loaded: the firmware compares patch pointers.
    patch_index: Option<usize>,
    lfo: Lfo,
    voice: Voice<'a, 6, 32>,
    parameters: VoiceParameters,
}

impl<'a> FmVoice<'a> {
    pub fn new() -> Self {
        Self { patch_index: None, lfo: Lfo::new(), voice: Voice::<'a, 6, 32>::new(), parameters: VoiceParameters::new() }
    }

    pub fn init(&mut self, algorithms: &'a Algorithms<6, 32>, sample_rate: f32) {
        self.voice.init(algorithms, sample_rate);
        self.lfo.init(sample_rate);
        self.parameters.sustain = false;
        self.parameters.gate = false;
        self.parameters.note = 48.0;
        self.parameters.velocity = 0.5;
        self.parameters.brightness = 0.5;
        self.parameters.envelope_control = 0.5;
        self.parameters.pitch_mod = 0.0;
        self.parameters.amp_mod = 0.0;
        self.patch_index = None;
    }

    /// `LoadPatch`: nothing happens when the same patch is already loaded.
    pub fn load_patch(&mut self, index: usize, patch: &Patch) {
        if self.patch_index == Some(index) {
            return;
        }
        self.patch_index = Some(index);
        self.voice.set_patch(Some(patch.clone()));
        self.lfo.set(&patch.modulations);
    }

    /// Renders `size` frames into `buffer`, which holds the voice's three buffers back to back
    /// (out, aux, temp), as the firmware's `Voice::Render(parameters, temp, size)`.
    #[inline]
    pub fn render(&mut self, buffer: &mut [f32], size: usize) {
        if self.patch_index.is_none() {
            return;
        }

        let (out, rest) = buffer.split_at_mut(size);
        let (aux, rest) = rest.split_at_mut(size);
        let buffers = [RefCell::new(out), RefCell::new(aux), RefCell::new(&mut rest[..size])];

        self.voice.render(&self.parameters, &buffers);
    }

    /// `UnloadPatch`: forgets the patch, so the next load always takes.
    #[inline]
    pub fn unload_patch(&mut self) {
        self.patch_index = None;
    }

    #[inline]
    pub fn patch_index(&self) -> Option<usize> {
        self.patch_index
    }

    #[inline]
    pub fn mutable_parameters(&mut self) -> &mut VoiceParameters {
        &mut self.parameters
    }

    #[inline]
    pub fn mutable_lfo(&mut self) -> &mut Lfo {
        &mut self.lfo
    }

    #[inline]
    pub fn lfo(&self) -> &Lfo {
        &self.lfo
    }

    #[inline]
    pub fn set_modulations(&mut self, pitch_mod: f32, amp_mod: f32) {
        self.parameters.pitch_mod = pitch_mod;
        self.parameters.amp_mod = amp_mod;
    }
}

impl Default for FmVoice<'_> {
    fn default() -> Self {
        Self::new()
    }
}

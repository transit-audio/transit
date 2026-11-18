//! The synthesis voice: the firmware's `Voice` (`plaits/dsp/voice.cc`), engine selection, the trigger
//! delay, the decay envelope, the low-pass gate and its envelope, the limiter and the int16 output.
//!
//! Nulled against the oracle's voice frame by frame (`tests/null_vs_oracle_voice.rs`,
//! `tests/voice_scenarios_vs_oracle.rs`); the one documented difference is the firmware's shared
//! engine RAM (see the crate README).

// Based on MIT-licensed code (c) 2016 by Emilie Gillet (emilie.o.gillet@gmail.com)

use super::engine::additive_engine::AdditiveEngine;
use super::engine::bass_drum_engine::BassDrumEngine;
use super::engine::chord_engine::ChordEngine;
use super::engine::fm_engine::FmEngine;
use super::engine::grain_engine::GrainEngine;
use super::engine::hihat_engine::HihatEngine;
use super::engine::modal_engine::ModalEngine;
use super::engine::noise_engine::NoiseEngine;
use super::engine::particle_engine::ParticleEngine;
use super::engine::snare_drum_engine::SnareDrumEngine;
use super::engine::speech_engine::SpeechEngine;
use super::engine::string_engine::StringEngine;
use super::engine::swarm_engine::SwarmEngine;
use super::engine::virtual_analog_engine::VirtualAnalogEngine;
use super::engine::waveshaping_engine::WaveshapingEngine;
use super::engine::wavetable_engine::WavetableEngine;
use super::engine::{Engine, EngineParameters, MAX_BLOCK_SIZE, TriggerState, note_to_frequency};
use super::engine2::chiptune_engine::{self, ChiptuneEngine};
use super::engine2::phase_distortion_engine::PhaseDistortionEngine;
use super::engine2::six_op_engine::SixOpEngine;
use super::engine2::string_machine_engine::StringMachineEngine;
use super::engine2::virtual_analog_vcf_engine::VirtualAnalogVcfEngine;
use super::engine2::wave_terrain_engine::WaveTerrainEngine;
use super::envelope::{DecayEnvelope, LpgEnvelope};
use super::fx::low_pass_gate::LowPassGate;
use crate::SampleRate;
use crate::resources::sysex::{SYX_BANK_0, SYX_BANK_1, SYX_BANK_2};
use crate::resources::waves::WAV_INTEGRATED_WAVES;
use crate::utils::delay_line::DelayLine;
use crate::utils::hysteresis_quantizer::HysteresisQuantizer2;
use crate::utils::limiter::Limiter;
use crate::utils::random::Random;
use crate::utils::units::semitones_to_ratio;
use crate::utils::{clip_16, ftz};

const MAX_TRIGGER_DELAY: usize = 8;
/// The trigger is read 5 blocks late (`kTriggerDelay`) at every rate: 1.25 ms at 48 kHz in
/// 12-frame blocks (1.36 ms at 44.1 kHz, 0.63 ms at 96 kHz).
const TRIGGER_DELAY: f32 = 5.0;
/// The block the voice's envelopes are timed for (`kBlockSize`): every render call advances them
/// by one 12-frame block, whatever its frame count, as in the firmware.
const CONTROL_BLOCK_SIZE: f32 = 12.0;
pub const NUM_ENGINES: usize = 24;

/// One output frame, as the firmware's DAC receives it (`Voice::Frame`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Main output.
    pub out: i16,
    /// Auxiliary output.
    pub aux: i16,
}

/// Patch parameters.
#[derive(Debug, Clone)]
pub struct Patch {
    /// Note number in the range from `-119.0` to `120.0`. Default is `48.0`.
    pub note: f32,

    /// HARMONICS parameter in the range from `0.0` to `1.0`. Default is `0.5`.
    pub harmonics: f32,

    /// TIMBRE parameter in the range from `0.0` to `1.0`. Default is `0.5`.
    pub timbre: f32,

    /// MORPH parameter in the range from `0.0` to `1.0`. Default is `0.5`.
    pub morph: f32,

    /// Frequency modulation amount in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub frequency_modulation_amount: f32,

    /// TIMBRE modulation amount in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub timbre_modulation_amount: f32,

    /// MORPH modulation amount in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub morph_modulation_amount: f32,

    /// Engine selection in the range from `0` to `23`. Default is `0`.
    pub engine: usize,

    /// Envelope decay in the range from `0.0` to `1.0`. Default is `0.5`.
    pub decay: f32,

    /// Low-pass gate color in the range from `0.0` to `1.0`. Default is `0.5`.
    pub lpg_colour: f32,
}

impl Default for Patch {
    fn default() -> Self {
        Self {
            note: 48.0,
            harmonics: 0.5,
            timbre: 0.5,
            morph: 0.5,
            frequency_modulation_amount: 0.0,
            timbre_modulation_amount: 0.0,
            morph_modulation_amount: 0.0,
            engine: 0,
            decay: 0.5,
            lpg_colour: 0.5,
        }
    }
}

/// Modulation parameters.
#[derive(Debug, Default, Clone)]
pub struct Modulations {
    /// Engine select modulation in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub engine: f32,

    /// Note number modulation in the range from `-119.0` to `120.0`. Default is `0.0`.
    pub note: f32,

    /// Frequency modulation in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub frequency: f32,

    /// HARMONICS modulation in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub harmonics: f32,

    /// TIMBRE modulation in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub timbre: f32,

    /// MORPH modulation in the range from `-1.0` to `1.0`. Default is `0.0`.
    pub morph: f32,

    /// Trigger signal in the range from `0.0` to `1.0`. Default is `0.0`.
    pub trigger: f32,

    /// Level modulation in the range from `0.0` to `1.0`. Default is `0.0`.
    pub level: f32,

    /// Flag if frequency modulation is applied. Default is `false`.
    pub frequency_patched: bool,

    /// Flag if timbre modulation is applied. Default is `false`.
    pub timbre_patched: bool,

    /// Flag if morph modulation is applied. Default is `false`.
    pub morph_patched: bool,

    /// Flag if trigger signal is used. Default is `false`.
    pub trigger_patched: bool,

    /// Flag if level modulation is used. Default is `false`.
    pub level_patched: bool,
}

/// Resources used by some of the engines. The provided data is loaded when an engine
/// is selected. Call `Voice::reload_resources` to force an update without changing the engine.
#[derive(Debug, Clone)]
pub struct Resources<'a> {
    /// Sysex bank for six op engine 2. Default is integrated `SYX_BANK_0`.
    pub syx_bank_a: &'a [u8; 4096],

    /// Sysex bank for six op engine 3. Default is integrated `SYX_BANK_1`.
    pub syx_bank_b: &'a [u8; 4096],

    /// Sysex bank for six op engine 4. Default is integrated `SYX_BANK_2`.
    pub syx_bank_c: &'a [u8; 4096],

    /// User terrain for the wave terrain engine. Default is `None`.
    pub user_wave_terrain: Option<&'a [u8; 4096]>,

    /// Wavetables for the wavetable engine. Default is integrated waves.
    pub wavetables: &'a [i16; 25344],
}

impl Default for Resources<'_> {
    fn default() -> Self {
        Self { syx_bank_a: &SYX_BANK_0, syx_bank_b: &SYX_BANK_1, syx_bank_c: &SYX_BANK_2, user_wave_terrain: None, wavetables: &WAV_INTEGRATED_WAVES }
    }
}

/// The 24 engines of a voice (22 instances: the three six-operator banks share one), in the
/// firmware's registration order (`Voice::Init` in `plaits/dsp/voice.cc`).
#[derive(Debug, Clone)]
pub struct EngineBank<'a> {
    pub additive_engine: AdditiveEngine,
    pub bass_drum_engine: BassDrumEngine,
    pub chiptune_engine: ChiptuneEngine,
    pub chord_engine: ChordEngine<'a>,
    pub fm_engine: FmEngine,
    pub grain_engine: GrainEngine,
    pub hihat_engine: HihatEngine,
    pub modal_engine: ModalEngine,
    pub noise_engine: NoiseEngine,
    pub particle_engine: ParticleEngine,
    pub phase_distortion_engine: PhaseDistortionEngine,
    pub six_op_engine: SixOpEngine<'a>,
    pub snare_drum_engine: SnareDrumEngine,
    pub speech_engine: SpeechEngine<'a>,
    pub string_engine: StringEngine,
    pub string_machine_engine: StringMachineEngine,
    pub swarm_engine: SwarmEngine,
    pub virtual_analog_engine: VirtualAnalogEngine,
    pub virtual_analog_vcf_engine: VirtualAnalogVcfEngine,
    pub waveshaping_engine: WaveshapingEngine,
    pub wavetable_engine: WavetableEngine<'a>,
    pub waveterrain_engine: WaveTerrainEngine<'a>,
}

impl EngineBank<'_> {
    /// The engine at `index` with its post-processing settings: already enveloped, out gain, aux
    /// gain (a negative gain runs the limiter at that pre-gain).
    pub fn get(&mut self, index: usize) -> Option<(&mut dyn Engine, bool, f32, f32)> {
        match index {
            0 => Some((&mut self.virtual_analog_vcf_engine, false, 1.0, 1.0)),
            1 => Some((&mut self.phase_distortion_engine, false, 0.7, 0.7)),
            2 => Some((&mut self.six_op_engine, true, 1.0, 1.0)),
            3 => Some((&mut self.six_op_engine, true, 1.0, 1.0)),
            4 => Some((&mut self.six_op_engine, true, 1.0, 1.0)),
            5 => Some((&mut self.waveterrain_engine, false, 0.7, 0.7)),
            6 => Some((&mut self.string_machine_engine, false, 0.8, 0.8)),
            7 => Some((&mut self.chiptune_engine, false, 0.5, 0.5)),
            8 => Some((&mut self.virtual_analog_engine, false, 0.8, 0.8)),
            9 => Some((&mut self.waveshaping_engine, false, 0.7, 0.6)),
            10 => Some((&mut self.fm_engine, false, 0.6, 0.6)),
            11 => Some((&mut self.grain_engine, false, 0.7, 0.6)),
            12 => Some((&mut self.additive_engine, false, 0.8, 0.8)),
            13 => Some((&mut self.wavetable_engine, false, 0.6, 0.6)),
            14 => Some((&mut self.chord_engine, false, 0.8, 0.8)),
            15 => Some((&mut self.speech_engine, false, -0.7, 0.8)),
            16 => Some((&mut self.swarm_engine, false, -3.0, 1.0)),
            17 => Some((&mut self.noise_engine, false, -1.0, -1.0)),
            18 => Some((&mut self.particle_engine, false, -2.0, 1.0)),
            19 => Some((&mut self.string_engine, true, -1.0, 0.8)),
            20 => Some((&mut self.modal_engine, true, -1.0, 0.8)),
            21 => Some((&mut self.bass_drum_engine, true, 0.8, 0.8)),
            22 => Some((&mut self.snare_drum_engine, true, 0.8, 0.8)),
            23 => Some((&mut self.hihat_engine, true, 0.8, 0.8)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Voice<'a> {
    pub engines: EngineBank<'a>,

    /// The voice's one random generator, handed to whichever engine renders (the firmware's
    /// `stmlib::Random` is one static shared by all of a voice's engines).
    rng: Random,

    pub resources: Resources<'a>,

    engine_quantizer: HysteresisQuantizer2,

    reload_resources: bool,
    /// The engine that rendered last (`previous_engine_index_`, -1 before the first render).
    previous_engine_index: Option<usize>,
    engine_cv: f32,

    previous_note: f32,
    trigger_state: bool,

    decay_envelope: DecayEnvelope,
    lpg_envelope: LpgEnvelope,

    trigger_delay: DelayLine<f32, MAX_TRIGGER_DELAY>,

    out_post_processor: ChannelPostProcessor,
    aux_post_processor: ChannelPostProcessor,

    out_buffer: [f32; MAX_BLOCK_SIZE],
    aux_buffer: [f32; MAX_BLOCK_SIZE],

    sample_rate_hz: f32,
    /// `(200 * kBlockSize) / kSampleRate`, the short decay before the knob's scaling.
    short_decay_scale: f32,
    /// `(20 * kBlockSize) / kSampleRate`, the low-pass gate's decay tail before the knob's scaling.
    decay_tail_scale: f32,
    a0_normalized: f32,
    /// The firmware's `a0` at this rate; `a0_normalized` is it times the tuning's pitch scale.
    a0_firmware: f32,
    tuning: crate::Tuning,
}

impl Voice<'_> {
    /// A voice at `sample_rate`, initialized ([`Voice::init`] runs here, so a new voice renders
    /// from the firmware's power-on state; calling `init` again returns a used voice to it). Build
    /// it off the audio thread.
    ///
    /// The firmware computes pitch against its corrected rate, [`crate::CORRECTED_SAMPLE_RATE`]
    /// (`a0` in `dsp/dsp.h`): the module's codec runs at 47872.34 Hz, so the module plays in tune.
    /// At 48 kHz the voice keeps the firmware's arithmetic bit for bit, and so plays the firmware's
    /// pitches scaled by 48000 / 47872.34 (4.6 cents sharp). At the other rates the normalized
    /// pitch is scaled by 48000 / rate, so every note sounds at the same frequency in hertz as at
    /// 48 kHz, and the voice's envelope coefficients are per 12-frame block at that rate, so
    /// decays keep their duration in seconds.
    #[must_use]
    pub fn new(sample_rate: SampleRate) -> Self {
        let sample_rate_hz = sample_rate.hz();
        let block_size = MAX_BLOCK_SIZE;
        // 48000 / 48000 is exactly 1, so at 48 kHz this is the firmware's `a0` bit for bit.
        let a0_normalized = crate::A0 * (crate::REF_SAMPLE_RATE / sample_rate_hz);

        let mut voice = Self {
            engines: EngineBank {
                additive_engine: AdditiveEngine::new(),
                bass_drum_engine: BassDrumEngine::new(),
                chiptune_engine: ChiptuneEngine::new(),
                chord_engine: ChordEngine::new(),
                fm_engine: FmEngine::new(),
                grain_engine: GrainEngine::new(),
                hihat_engine: HihatEngine::new(block_size),
                modal_engine: ModalEngine::new(block_size),
                noise_engine: NoiseEngine::new(block_size),
                particle_engine: ParticleEngine::new(block_size),
                phase_distortion_engine: PhaseDistortionEngine::new(block_size),
                six_op_engine: SixOpEngine::new(block_size),
                snare_drum_engine: SnareDrumEngine::new(),
                speech_engine: SpeechEngine::new(block_size),
                string_engine: StringEngine::new(block_size),
                string_machine_engine: StringMachineEngine::new(),
                swarm_engine: SwarmEngine::new(),
                virtual_analog_engine: VirtualAnalogEngine::new(block_size),
                virtual_analog_vcf_engine: VirtualAnalogVcfEngine::new(),
                waveshaping_engine: WaveshapingEngine::new(),
                wavetable_engine: WavetableEngine::new(),
                waveterrain_engine: WaveTerrainEngine::new(block_size),
            },
            rng: Random::default(),

            resources: Resources::default(),

            engine_quantizer: HysteresisQuantizer2::new(),
            reload_resources: false,
            previous_engine_index: None,
            engine_cv: 0.0,

            previous_note: 0.0,
            trigger_state: false,

            decay_envelope: DecayEnvelope::new(),
            lpg_envelope: LpgEnvelope::new(),

            trigger_delay: DelayLine::new(),

            out_post_processor: ChannelPostProcessor::new(),
            aux_post_processor: ChannelPostProcessor::new(),

            out_buffer: [0.0; MAX_BLOCK_SIZE],
            aux_buffer: [0.0; MAX_BLOCK_SIZE],

            sample_rate_hz,
            short_decay_scale: (200.0 * CONTROL_BLOCK_SIZE) / sample_rate_hz,
            decay_tail_scale: (20.0 * CONTROL_BLOCK_SIZE) / sample_rate_hz,
            a0_normalized,
            a0_firmware: a0_normalized,
            tuning: crate::Tuning::Firmware,
        };
        voice.init();
        voice
    }

    pub fn init(&mut self) {
        let sample_rate_hz = self.sample_rate_hz;
        for i in 0..NUM_ENGINES {
            self.engines.get(i).unwrap().0.init(sample_rate_hz);
        }

        self.engine_quantizer.init(NUM_ENGINES as i32, 0.05, true);
        self.previous_engine_index = None;
        self.reload_resources = false;
        self.engine_cv = 0.0;

        self.out_post_processor.init();
        self.aux_post_processor.init();

        self.decay_envelope.init();
        self.lpg_envelope.init();

        self.trigger_state = false;
        self.previous_note = 0.0;

        self.trigger_delay.init();
    }

    /// Sets how the voice computes pitch (see [`crate::Tuning`]). A new voice uses
    /// [`crate::Tuning::Firmware`], the firmware's arithmetic bit for bit. Allocation-free and cheap:
    /// safe on the audio thread; a sounding note changes pitch at the next block.
    pub fn set_tuning(&mut self, tuning: crate::Tuning) {
        self.tuning = tuning;
        // Times exactly 1 for the firmware's tuning, so its `a0` is kept bit for bit.
        self.a0_normalized = self.a0_firmware * tuning.pitch_scale();
        self.engines.six_op_engine.set_tuning(tuning);
        self.engines.speech_engine.set_tuning(tuning);
    }

    /// transit's same-pitch restart on the six-op models (#263, ADR 0033): a re-strike of the
    /// note the six-op engine is still sounding restarts that internal voice instead of starting
    /// the note on the other one under the old note's release (see
    /// [`SixOpEngine::set_same_pitch_restart`] for the rule). Off in a new voice, the firmware's
    /// behavior bit for bit, which the null tests compare; transit's instrument turns it on. Kept
    /// across `init`; allocation-free, safe on the audio thread.
    pub fn set_same_pitch_restart(&mut self, on: bool) {
        self.engines.six_op_engine.set_same_pitch_restart(on);
    }

    /// How the voice computes pitch.
    #[must_use]
    pub fn tuning(&self) -> crate::Tuning {
        self.tuning
    }

    /// Sets the voice's random state (`stmlib::Random::Seed`). A new voice starts from the
    /// firmware's power-on state, [`Random::DEFAULT_SEED`].
    pub fn seed(&mut self, seed: u32) {
        self.rng.seed(seed);
    }

    #[inline]
    pub fn reload_resources(&mut self) {
        self.reload_resources = true;
    }

    /// Renders `frames.len()` frames as the firmware's `Voice::Render`: int16 out and aux, inverted
    /// as the module's DAC expects. One call is one control block: the envelopes, the trigger delay
    /// and the parameter smoothing advance by one 12-frame block whatever the frame count, as in the
    /// firmware, so call it with 12 frames (any count from 1 to [`MAX_BLOCK_SIZE`] is rendered as
    /// one block). A longer slice is rendered as consecutive 12-frame blocks with the same inputs;
    /// an empty one renders nothing.
    ///
    /// Feed 12-frame blocks for the firmware's envelope timing at every rate: 24 frames are one
    /// control block (its envelopes then run half as fast per frame), 25 are three (12 + 12 + 1).
    #[inline]
    pub fn render(&mut self, patch: &Patch, modulations: &Modulations, frames: &mut [Frame]) {
        let size = frames.len();
        if size > MAX_BLOCK_SIZE {
            for chunk in frames.chunks_mut(CONTROL_BLOCK_SIZE as usize) {
                self.render(patch, modulations, chunk);
            }
            return;
        }
        if size == 0 {
            return;
        }
        let (out, aux) = (&mut self.out_buffer[..size], &mut self.aux_buffer[..size]);
        // Trigger, LPG, internal envelope.

        // Delay trigger by 1ms to deal with sequencers or MIDI interfaces whose
        // CV out lags behind the GATE out.
        self.trigger_delay.write(modulations.trigger);
        let trigger_value = self.trigger_delay.read_with_delay_frac(TRIGGER_DELAY);

        let previous_trigger_state = self.trigger_state;

        if !previous_trigger_state {
            if trigger_value > 0.3 {
                self.trigger_state = true;
                if !modulations.level_patched {
                    self.lpg_envelope.trigger();
                }
                self.decay_envelope.trigger();
                self.engine_cv = modulations.engine;
            }
        } else if trigger_value < 0.1 {
            self.trigger_state = false;
        }

        if !modulations.trigger_patched {
            self.engine_cv = modulations.engine;
        }

        // Engine selection.
        // The quantizer bounds the index to 0..NUM_ENGINES.
        let engine_index = self.engine_quantizer.process_with_base(patch.engine as i32, self.engine_cv) as usize;

        if Some(engine_index) != self.previous_engine_index || self.reload_resources {
            match engine_index {
                2 => {
                    self.engines.six_op_engine.load_syx_bank(self.resources.syx_bank_a);
                }
                3 => {
                    self.engines.six_op_engine.load_syx_bank(self.resources.syx_bank_b);
                }
                4 => {
                    self.engines.six_op_engine.load_syx_bank(self.resources.syx_bank_c);
                }
                5 => {
                    self.engines.waveterrain_engine.set_user_terrain(self.resources.user_wave_terrain);
                }
                13 => {
                    self.engines.wavetable_engine.set_wavetables(self.resources.wavetables);
                }
                _ => {}
            }

            let engine = self.engines.get(engine_index).unwrap().0;
            engine.reset();

            self.out_post_processor.reset();
            self.previous_engine_index = Some(engine_index);
            self.reload_resources = false;
        }

        let mut p = EngineParameters { a0_normalized: self.a0_normalized, ..Default::default() };

        let rising_edge = self.trigger_state && !previous_trigger_state;
        let note = (modulations.note + self.previous_note) * 0.5;
        self.previous_note = modulations.note;

        if modulations.trigger_patched {
            p.trigger = (if rising_edge { TriggerState::RISING_EDGE } else { TriggerState::LOW })
                | (if self.trigger_state { TriggerState::HIGH } else { TriggerState::LOW });
        } else {
            p.trigger = TriggerState::UNPATCHED;
        }

        // Decay and color are knobs in [0, 1]; out of range the pitch table lookup would read past its
        // end (the firmware reads past its table). Clamping changes no bit in range.
        let decay = patch.decay.clamp(0.0, 1.0);
        let short_decay = self.short_decay_scale * semitones_to_ratio(-96.0 * decay);

        self.decay_envelope.process(short_decay * 2.0);

        let compressed_level = (1.3 * modulations.level / (0.3 + modulations.level.abs())).clamp(0.0, 1.0);
        p.accent = if modulations.level_patched { compressed_level } else { 0.8 };

        let use_internal_envelope = modulations.trigger_patched;

        // Actual synthesis parameters.

        p.harmonics = patch.harmonics + modulations.harmonics;
        p.harmonics = p.harmonics.clamp(0.0, 1.0);

        let mut internal_envelope_amplitude = 1.0;
        let mut internal_envelope_amplitude_timbre = 1.0;

        if engine_index == 15 {
            internal_envelope_amplitude = 2.0 - p.harmonics * 6.0;
            internal_envelope_amplitude = internal_envelope_amplitude.clamp(0.0, 1.0);
            self.engines.speech_engine.set_prosody_amount(if !modulations.trigger_patched || modulations.frequency_patched {
                0.0
            } else {
                patch.frequency_modulation_amount
            });
            self.engines.speech_engine.set_speed(if !modulations.trigger_patched || modulations.morph_patched {
                0.0
            } else {
                patch.morph_modulation_amount
            });
        } else if engine_index == 7 {
            if modulations.trigger_patched && !modulations.timbre_patched {
                // Disable internal envelope on TIMBRE, and enable the envelope generator
                // built into the chiptune engine.
                internal_envelope_amplitude_timbre = 0.0;
                // The envelope shape is the TIMBRE modulation amount, as is (0 holds the note).
                self.engines.chiptune_engine.set_envelope_shape(patch.timbre_modulation_amount);
            } else {
                self.engines.chiptune_engine.set_envelope_shape(chiptune_engine::NO_ENVELOPE);
            }
        }

        p.note = apply_modulations(
            patch.note + note,
            patch.frequency_modulation_amount,
            modulations.frequency_patched,
            modulations.frequency,
            use_internal_envelope,
            ftz(ftz(ftz(internal_envelope_amplitude * self.decay_envelope.value()) * self.decay_envelope.value()) * 48.0),
            1.0,
            -119.0,
            120.0,
        );

        p.timbre = apply_modulations(
            patch.timbre,
            patch.timbre_modulation_amount,
            modulations.timbre_patched,
            modulations.timbre,
            use_internal_envelope,
            ftz(internal_envelope_amplitude_timbre * self.decay_envelope.value()),
            0.0,
            0.0,
            1.0,
        );

        p.morph = apply_modulations(
            patch.morph,
            patch.morph_modulation_amount,
            modulations.morph_patched,
            modulations.morph,
            use_internal_envelope,
            ftz(internal_envelope_amplitude * self.decay_envelope.value()),
            0.0,
            0.0,
            1.0,
        );

        let engine = self.engines.get(engine_index).unwrap();
        let mut already_enveloped = engine.1;
        let out_gain = engine.2;
        let aux_gain = engine.3;

        engine.0.render(&p, &mut self.rng, out, aux, &mut already_enveloped);

        let lpg_bypass = already_enveloped || (!modulations.level_patched && !modulations.trigger_patched);

        // Compute LPG parameters.
        if !lpg_bypass {
            let hf = patch.lpg_colour.clamp(0.0, 1.0);
            let decay_tail = self.decay_tail_scale * semitones_to_ratio(-72.0 * decay + 12.0 * hf) - short_decay;

            if modulations.level_patched {
                self.lpg_envelope.process_lp(compressed_level, short_decay, decay_tail, hf);
            } else {
                let attack = note_to_frequency(p.note, self.a0_normalized) * CONTROL_BLOCK_SIZE * 2.0;
                self.lpg_envelope.process_ping(attack, short_decay, decay_tail, hf);
            }
        } else {
            self.lpg_envelope.init();
        }

        self.out_post_processor.process(
            out_gain,
            lpg_bypass,
            self.lpg_envelope.gain(),
            self.lpg_envelope.frequency(),
            self.lpg_envelope.hf_bleed(),
            out,
        );

        self.aux_post_processor.process(
            aux_gain,
            lpg_bypass,
            self.lpg_envelope.gain(),
            self.lpg_envelope.frequency(),
            self.lpg_envelope.hf_bleed(),
            aux,
        );

        for (frame, (o, a)) in frames.iter_mut().zip(out.iter().zip(aux.iter())) {
            *frame = Frame { out: to_i16(*o), aux: to_i16(*a) };
        }
    }

    /// The engine that rendered last, `None` before the first render.
    pub fn active_engine(&self) -> Option<usize> {
        self.previous_engine_index
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChannelPostProcessor {
    limiter: Limiter,
    lpg: LowPassGate,
}

impl ChannelPostProcessor {
    pub fn new() -> Self {
        Self { limiter: Limiter::new(), lpg: LowPassGate::new() }
    }

    pub fn init(&mut self) {
        self.lpg.init();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.limiter.init();
    }

    /// The firmware's `ChannelPostProcessor::Process` up to the int16 conversion: the limiter
    /// (negative gain), then the low-pass gate or the plain gain, scaled to int16 full scale and
    /// inverted (`* -32767`). [`to_i16`] finishes the conversion.
    #[inline]
    pub fn process(
        &mut self,
        gain: f32,
        bypass_lpg: bool,
        low_pass_gate_gain: f32,
        low_pass_gate_frequency: f32,
        low_pass_gate_hf_bleed: f32,
        in_out: &mut [f32],
    ) {
        if gain < 0.0 {
            self.limiter.process(-gain, in_out);
        }

        let post_gain = (if gain < 0.0 { 1.0 } else { gain }) * -32767.0;

        if !bypass_lpg {
            self.lpg.process_replacing(post_gain * low_pass_gate_gain, low_pass_gate_frequency, low_pass_gate_hf_bleed, in_out);
        } else {
            for sample in in_out.iter_mut() {
                *sample *= post_gain;
            }
        }
    }
}

/// The firmware's int16 conversion of a post-processed sample: `Clip16(1 + int32_t(x))`, the cast
/// truncating toward zero. Identical for every finite value below 2^31 in magnitude. Outside that
/// the Rust cast saturates: NaN maps to 1 and a huge value to +/-32767, where the C++ cast gives
/// INT_MIN on x86 (so -32768); the nulled voice never produces either.
#[inline]
#[must_use]
pub fn to_i16(x: f32) -> i16 {
    // `clip_16` bounds the value to the i16 range, so the narrowing cannot truncate.
    #[allow(clippy::cast_possible_truncation)]
    let sample = clip_16(1_i32.saturating_add(x as i32)) as i16;
    sample
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn apply_modulations(
    base_value: f32,
    mut modulation_amount: f32,
    use_external_modulation: bool,
    external_modulation: f32,
    use_internal_envelope: bool,
    envelope: f32,
    default_internal_modulation: f32,
    minimum_value: f32,
    maximum_value: f32,
) -> f32 {
    let mut value = base_value;
    modulation_amount *= f32::max(modulation_amount.abs() - 0.05, 0.05);
    modulation_amount *= 1.05;
    // The envelope decays toward zero, so the product and the sum are flushed as on the firmware.

    let modulation = if use_external_modulation {
        external_modulation
    } else if use_internal_envelope {
        envelope
    } else {
        default_internal_modulation
    };

    value = ftz(value + ftz(modulation_amount * modulation));
    value = value.clamp(minimum_value, maximum_value);

    value
}

// ---- The state record (feature `testing`; ADR 0029, amendment for #742) ----

#[cfg(feature = "testing")]
impl crate::record::Record for Resources<'_> {
    /// The resources are loaded content (configuration): each reference as whether it is the
    /// integrated default, the only resources transit gives a voice (`Resources::default`).
    /// Restore refuses anything else and leaves the receiving voice's (default) references.
    fn record(&self, w: &mut crate::record::Writer<'_>) {
        let Self { syx_bank_a, syx_bank_b, syx_bank_c, user_wave_terrain, wavetables } = self;
        w.bool(core::ptr::eq(*syx_bank_a, &SYX_BANK_0));
        w.bool(core::ptr::eq(*syx_bank_b, &SYX_BANK_1));
        w.bool(core::ptr::eq(*syx_bank_c, &SYX_BANK_2));
        w.bool(user_wave_terrain.is_none());
        w.bool(core::ptr::eq(*wavetables, &WAV_INTEGRATED_WAVES));
    }
    fn restore(&mut self, r: &mut crate::record::Reader<'_>) -> Option<()> {
        let Self { syx_bank_a, syx_bank_b, syx_bank_c, user_wave_terrain, wavetables } = self;
        let defaults = [
            core::ptr::eq(*syx_bank_a, &SYX_BANK_0),
            core::ptr::eq(*syx_bank_b, &SYX_BANK_1),
            core::ptr::eq(*syx_bank_c, &SYX_BANK_2),
            user_wave_terrain.is_none(),
            core::ptr::eq(*wavetables, &WAV_INTEGRATED_WAVES),
        ];
        for d in defaults {
            (r.bool()? && d).then_some(())?;
        }
        Some(())
    }
}

#[cfg(feature = "testing")]
crate::record_struct!([] Frame { out, aux });

/// Calls `$m!` with every engine field of [`EngineBank`], in declaration order, and the engine
/// indices that select it ([`EngineBank::get`]; a unit test checks the two agree).
#[cfg(feature = "testing")]
macro_rules! engine_fields {
    ($m:ident) => {
        $m! {
            additive_engine: [12],
            bass_drum_engine: [21],
            chiptune_engine: [7],
            chord_engine: [14],
            fm_engine: [10],
            grain_engine: [11],
            hihat_engine: [23],
            modal_engine: [20],
            noise_engine: [17],
            particle_engine: [18],
            phase_distortion_engine: [1],
            six_op_engine: [2, 3, 4],
            snare_drum_engine: [22],
            speech_engine: [15],
            string_engine: [19],
            string_machine_engine: [6],
            swarm_engine: [16],
            virtual_analog_engine: [8],
            virtual_analog_vcf_engine: [0],
            waveshaping_engine: [9],
            wavetable_engine: [13],
            waveterrain_engine: [5],
        }
    };
}

/// The bank's record (feature `testing`): every engine as a concrete field, in declaration order;
/// the selected engine (`active`, the voice's `previous_engine_index`) whole, every other dormant
/// (see `crate::record`: a dormant engine's reset-rewritten state is left out, because every
/// switch to it calls its `reset()` in `Voice::render` before it renders).
#[cfg(feature = "testing")]
impl EngineBank<'_> {
    fn selects(active: Option<usize>, indices: &[usize]) -> bool {
        active.is_some_and(|a| indices.contains(&a))
    }

    fn record_with(&self, active: Option<usize>, w: &mut crate::record::Writer<'_>) {
        use crate::record::Record;
        macro_rules! go {
            ($($f:ident: [$($i:literal),+]),* $(,)?) => {
                let Self { $($f),* } = self;
                $( if Self::selects(active, &[$($i),+]) { $f.record(w) } else { $f.record_dormant(w) } )*
            };
        }
        engine_fields!(go);
    }

    fn restore_with(&mut self, active: Option<usize>, r: &mut crate::record::Reader<'_>) -> Option<()> {
        use crate::record::Record;
        macro_rules! go {
            ($($f:ident: [$($i:literal),+]),* $(,)?) => {
                let Self { $($f),* } = self;
                $( if Self::selects(active, &[$($i),+]) { $f.restore(r)? } else { $f.restore_dormant(r)? } )*
            };
        }
        engine_fields!(go);
        Some(())
    }

    /// Poisons the state a dormant engine's record leaves out (the engine at `index`, when it is
    /// not the selected one); the poison witnesses' hook.
    fn poison_dormant_engine(&mut self, active: Option<usize>, index: usize, p: &mut crate::record::Poison) {
        use crate::record::Record;
        macro_rules! go {
            ($($f:ident: [$($i:literal),+]),* $(,)?) => {
                let Self { $($f),* } = self;
                $( if [$($i),+].contains(&index) && !Self::selects(active, &[$($i),+]) { $f.poison_dormant(p) } )*
            };
        }
        engine_fields!(go);
    }

    /// Each engine field's address and the indices that select it (the table's unit test).
    #[cfg(test)]
    fn field_addresses(&self) -> [(*const (), &'static [usize]); 22] {
        macro_rules! go {
            ($($f:ident: [$($i:literal),+]),* $(,)?) => {{
                let Self { $($f),* } = self;
                [$( (core::ptr::from_ref($f).cast::<()>(), &[$($i),+][..]) ),*]
            }};
        }
        engine_fields!(go)
    }
}

// Configuration: `sample_rate_hz`, `short_decay_scale`, `decay_tail_scale`, `a0_firmware` (from the
// rate, in `new`), `tuning` and `a0_normalized` (`set_tuning`, which the owner calls once at
// construction); nothing else writes them.
// Unobservable:
// - `out_buffer`, `aux_buffer`: `render` hands their first `size` frames to the engine, which writes
//   every one of them before anything reads them (each engine assigns its outputs, or fills them
//   with 0 before it sums into them), and the post-processors and the int16 conversion read them
//   only after that; nothing else reads them. Witnesses: every case, every engine (the engine
//   switch's revisits and one witness per engine, `print_completeness/macro_osc.rs`), with the
//   restored instance's buffers poisoned after the restore and after every block
//   (`Voice::poison_unobservable`) against an unpoisoned twin.
// - `decay_envelope`: read only through `apply_modulations`, as the internal envelope times a
//   patch modulation amount, and the Macro Oscillator's patch carries none (`Slot::render` leaves
//   `frequency_modulation_amount` and `morph_modulation_amount` at 0, and sets the timbre amount
//   only for the chiptune model, whose internal envelope amplitude for Timbre is 0 in `render`);
//   an owner that sets a modulation amount must encode it. Guard: `Slot::render` asserts this of
//   every patch it renders in a testing build (`decay_envelope_unread`, transit-dsp's
//   `macro_osc_voice.rs`). Witnesses: every case, with the restored instance's envelope poisoned
//   after the restore and after every block against an unpoisoned twin.
// The selected engine's index comes first, so restore knows which engine is dormant before it
// reads the bank (`EngineBank::record_with`).
#[cfg(feature = "testing")]
impl crate::record::Record for Voice<'_> {
    fn record(&self, w: &mut crate::record::Writer<'_>) {
        let Self {
            engines,
            rng,
            resources,
            engine_quantizer,
            reload_resources,
            previous_engine_index,
            engine_cv,
            previous_note,
            trigger_state,
            decay_envelope: _,
            lpg_envelope,
            trigger_delay,
            out_post_processor,
            aux_post_processor,
            out_buffer: _,
            aux_buffer: _,
            sample_rate_hz: _,
            short_decay_scale: _,
            decay_tail_scale: _,
            a0_normalized: _,
            a0_firmware: _,
            tuning: _,
        } = self;
        previous_engine_index.record(w);
        engines.record_with(*previous_engine_index, w);
        rng.record(w);
        resources.record(w);
        engine_quantizer.record(w);
        reload_resources.record(w);
        engine_cv.record(w);
        previous_note.record(w);
        trigger_state.record(w);
        lpg_envelope.record(w);
        trigger_delay.record(w);
        out_post_processor.record(w);
        aux_post_processor.record(w);
    }

    fn restore(&mut self, r: &mut crate::record::Reader<'_>) -> Option<()> {
        let Self {
            engines,
            rng,
            resources,
            engine_quantizer,
            reload_resources,
            previous_engine_index,
            engine_cv,
            previous_note,
            trigger_state,
            decay_envelope: _,
            lpg_envelope,
            trigger_delay,
            out_post_processor,
            aux_post_processor,
            out_buffer: _,
            aux_buffer: _,
            sample_rate_hz: _,
            short_decay_scale: _,
            decay_tail_scale: _,
            a0_normalized: _,
            a0_firmware: _,
            tuning: _,
        } = self;
        previous_engine_index.restore(r)?;
        engines.restore_with(*previous_engine_index, r)?;
        rng.restore(r)?;
        resources.restore(r)?;
        engine_quantizer.restore(r)?;
        reload_resources.restore(r)?;
        engine_cv.restore(r)?;
        previous_note.restore(r)?;
        trigger_state.restore(r)?;
        lpg_envelope.restore(r)?;
        trigger_delay.restore(r)?;
        out_post_processor.restore(r)?;
        aux_post_processor.restore(r)?;
        // Derived: a selected six-operator model's patches, unpacked from its bank (see
        // `SixOpEngine`'s record).
        if let Some(model @ 2..=4) = *previous_engine_index {
            let bank = match model {
                2 => resources.syx_bank_a,
                3 => resources.syx_bank_b,
                _ => resources.syx_bank_c,
            };
            engines.six_op_engine.unpack_bank(bank);
        }
        Some(())
    }
}

#[cfg(feature = "testing")]
impl Voice<'_> {
    /// Poison witnesses only: overwrites, with values that differ from the ones they replace,
    /// the state the record leaves out of the engine at `index` (nothing when it is the selected
    /// engine or keeps nothing out while dormant).
    pub fn poison_engine(&mut self, index: usize, p: &mut crate::record::Poison) {
        let active = self.previous_engine_index;
        // The three six-operator models share one engine.
        let dormant = !active.is_some_and(|a| a == index || ((2..=4).contains(&a) && (2..=4).contains(&index)));
        if index == 15 {
            // The speech frames no read reaches (all of them while dormant).
            self.engines.speech_engine.poison_unread_frames(dormant, p);
        }
        if (2..=4).contains(&index) && dormant {
            self.engines.six_op_engine.poison_patches(p);
        }
        self.engines.poison_dormant_engine(active, index, p);
    }

    /// Poison witnesses only: [`Voice::poison_engine`] for every engine.
    /// Also the voice's block buffers and its decay envelope (unobservable, see the record).
    pub fn poison_unobservable(&mut self, p: &mut crate::record::Poison) {
        use crate::record::Record;
        self.out_buffer.poison(p);
        self.aux_buffer.poison(p);
        self.decay_envelope.poison(p);
        for index in 0..NUM_ENGINES {
            // The three six-operator models share one engine: poison it once.
            if !(3..=4).contains(&index) {
                self.poison_engine(index, p);
            }
        }
    }
}

#[cfg(feature = "testing")]
crate::record_struct!([] ChannelPostProcessor { limiter, lpg });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_slice_renders_as_consecutive_twelve_frame_blocks() {
        let patch = Patch { note: 60.0, engine: 10, ..Patch::default() };
        let modulations = Modulations::default();
        let mut whole = Voice::new(SampleRate::Hz48000);
        whole.init();
        let mut blocks = Voice::new(SampleRate::Hz48000);
        blocks.init();
        let mut long = [Frame::default(); 60];
        whole.render(&patch, &modulations, &mut long);
        let mut short = [Frame::default(); 60];
        for chunk in short.chunks_mut(12) {
            blocks.render(&patch, &modulations, chunk);
        }
        assert_eq!(long, short);
        whole.render(&patch, &modulations, &mut []);
    }

    /// The record's engine table (`engine_fields!`) names, for every index, the field `get`
    /// returns, and each index once.
    #[cfg(feature = "testing")]
    #[test]
    fn the_record_s_engine_table_agrees_with_get() {
        let mut voice = Voice::new(SampleRate::Hz48000);
        let fields = voice.engines.field_addresses();
        for index in 0..NUM_ENGINES {
            let selected: Vec<_> = fields.iter().filter(|(_, ix)| ix.contains(&index)).collect();
            assert_eq!(selected.len(), 1, "engine {index} is named {} times", selected.len());
            let engine = core::ptr::from_mut(voice.engines.get(index).unwrap().0).cast::<()>().cast_const();
            assert_eq!(selected[0].0, engine, "engine {index}: the table names another field");
        }
    }

    #[test]
    fn a_new_voice_renders_without_an_explicit_init_and_knobs_out_of_range_do_not_panic() {
        let mut voice = Voice::new(SampleRate::Hz48000);
        let modulations = Modulations { trigger: 1.0, trigger_patched: true, ..Modulations::default() };
        let mut frames = [Frame::default(); 12];
        for (decay, lpg_colour) in [(-5.0, 20.0), (5.0, -20.0), (f32::NAN, f32::NAN), (0.5, 0.5)] {
            voice.render(&Patch { decay, lpg_colour, ..Patch::default() }, &modulations, &mut frames);
        }
    }
}

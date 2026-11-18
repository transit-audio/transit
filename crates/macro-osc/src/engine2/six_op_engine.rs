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
    patches: [Patch; NUM_PATCHES_PER_BANK],
    patch_index_quantizer: HysteresisQuantizer2,
    voice: [FmVoice<'a>; NUM_SIX_OP_VOICES],

    // `temp_buffer_` and `acc_buffer_` in the firmware. The voice renders three buffers of
    // NUM_SIX_OP_VOICES blocks into the scratch: out, aux and one temporary.
    temp_buffer: Box<[f32]>,
    acc_buffer: Box<[f32]>,
    /// transit's restart crossfade (#263): the outgoing and the restarted render, three buffers
    /// of NUM_SIX_OP_VOICES blocks each.
    restart_buffer: Box<[f32]>,

    active_voice: usize,
    rendered_voice: usize,
    sample_rate: f32,
    tuning: crate::Tuning,
    /// transit's same-pitch restart (#263), off by default (the firmware's behavior); see
    /// [`SixOpEngine::set_same_pitch_restart`].
    same_pitch_restart: bool,
}

/// Two pitches closer than this many semitones are the same pitch for the same-pitch restart.
pub const SAME_PITCH_SEMITONES: f32 = 0.5;

impl SixOpEngine<'_> {
    pub fn new(block_size: usize) -> Self {
        Self {
            patches: core::array::from_fn(|_| Patch::new()),
            patch_index_quantizer: HysteresisQuantizer2::new(),
            voice: core::array::from_fn(|_| FmVoice::new()),
            temp_buffer: vec![0.0; 3 * NUM_SIX_OP_VOICES * block_size].into_boxed_slice(),
            acc_buffer: vec![0.0; NUM_SIX_OP_VOICES * block_size].into_boxed_slice(),
            restart_buffer: vec![0.0; 6 * NUM_SIX_OP_VOICES * block_size].into_boxed_slice(),
            active_voice: 0,
            rendered_voice: 0,
            sample_rate: 48000.0,
            tuning: crate::Tuning::Firmware,
            same_pitch_restart: false,
        }
    }

    /// transit's deviation from the firmware (#263, ADR 0033), off by default; kept across `init`.
    ///
    /// The firmware starts every trigger's note on its other internal voice and lets the voice
    /// that was sounding play its release, so a re-strike of the note already sounding layers
    /// that note's release under the new note at the same pitch, where the two can cancel each
    /// other's fundamental. With the restart on, a rising edge restarts the voice already
    /// sounding instead when all of these hold:
    /// - its patch is the one the edge selects (the same HARMONICS bank slot);
    /// - the edge's note is within [`SAME_PITCH_SEMITONES`] of the note that voice was struck at
    ///   (the engine has no pitch quantizer: this is the note input as the engine receives it,
    ///   with the bend and any pitch offset in it, so a held bend or offset counts as the same
    ///   pitch and one moved by half a semitone or more between the strikes does not);
    /// - it still sounds: its gate is high or one of its operator envelopes has not finished its
    ///   release.
    ///
    /// The restart retriggers the voice's operator and pitch envelopes from their current levels
    /// (no jump in level), samples the note and velocity, resets the LFO as a new note does, and
    /// resets the operators' phase when the patch asks for it, as the firmware does at a note-on,
    /// so the re-struck note has the first strike's timbre. A phase reset in a waveform that is
    /// sounding is a step, so the voice's next [`RESTART_FADE_RENDERS`] renders (48 frames, 1 ms at
    /// 48 kHz) crossfade, on a raised cosine, from the voice restarted keeping its phase to the
    /// voice restarted with the reset. Every other edge switches voices as the firmware does, so a
    /// render without a same-pitch re-strike is the firmware's bit for bit.
    pub fn set_same_pitch_restart(&mut self, on: bool) {
        self.same_pitch_restart = on;
    }

    /// The internal voice the last trigger selected still sounds: its gate is high or an operator
    /// envelope has not finished its release. This is what the same-pitch restart reads at an edge.
    #[must_use]
    pub fn sounding(&self) -> bool {
        self.voice[self.active_voice].sounding()
    }

    /// Sets the voices' pitch reference (see [`crate::Tuning`]); kept across `init`.
    pub fn set_tuning(&mut self, tuning: crate::Tuning) {
        self.tuning = tuning;
        for voice in self.voice.iter_mut() {
            voice.set_tuning(tuning);
        }
    }

    pub fn load_syx_bank(&mut self, bank: &[u8; 4096]) {
        self.unpack_bank(bank);

        for voice in self.voice.iter_mut() {
            voice.unload_patch();
        }
    }

    /// Unpacks the bank's 32 patches (the first half of [`SixOpEngine::load_syx_bank`]; the
    /// record's restore rebuilds the patches with it).
    pub(crate) fn unpack_bank(&mut self, bank: &[u8; 4096]) {
        for (i, patch) in self.patches.iter_mut().enumerate() {
            (*patch).unpack(&bank[i * SYX_SIZE..]);
        }
    }
}

impl Engine for SixOpEngine<'_> {
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
            voice.set_tuning(self.tuning);
        }

        self.active_voice = NUM_SIX_OP_VOICES - 1;
        self.rendered_voice = 0;
    }

    fn render(&mut self, parameters: &EngineParameters, rng: &mut Random, out: &mut [f32], aux: &mut [f32], _already_enveloped: &mut bool) {
        crate::engine::debug_check_render(parameters, out, aux);
        let size = out.len();
        let patch_index = self.patch_index_quantizer.process(parameters.harmonics * 1.02) as usize;

        if parameters.trigger.has(TriggerState::UNPATCHED) {
            let t = parameters.morph;
            self.voice[0].mutable_lfo().scrub(rng, 2.0 * self.sample_rate * t);

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
                let sounding = &self.voice[self.active_voice];
                let restart = self.same_pitch_restart
                    && sounding.patch_index() == Some(patch_index)
                    && (parameters.note - sounding.struck_note).abs() < SAME_PITCH_SEMITONES
                    && sounding.sounding();
                if restart {
                    self.voice[self.active_voice].restart();
                } else {
                    self.active_voice = (self.active_voice + 1) % NUM_SIX_OP_VOICES;
                }
                let active = self.active_voice;
                self.voice[active].load_patch(patch_index, &self.patches[patch_index]);
                self.voice[active].mutable_lfo().reset();
                self.voice[active].struck_note = parameters.note;
            }
            let active = self.active_voice;
            let p = self.voice[active].mutable_parameters();
            p.note = parameters.note;
            p.velocity = parameters.accent;
            p.envelope_control = parameters.morph;
            self.voice[active].mutable_lfo().step(rng, size as f32);

            let active_patch = self.voice[active].patch_index();
            for i in 0..NUM_SIX_OP_VOICES {
                let p = self.voice[i].mutable_parameters();
                p.brightness = parameters.timbre;
                p.sustain = false;
                p.gate = parameters.trigger.has(TriggerState::HIGH) && i == active;
                if self.voice[i].patch_index() != active_patch {
                    self.voice[i].mutable_lfo().step(rng, size as f32);
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
        self.voice[self.rendered_voice].render(temp, span, &mut self.restart_buffer[..6 * span]);

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
    /// The note input at this voice's last trigger (transit's same-pitch restart reads it).
    struck_note: f32,
    lfo: Lfo,
    voice: Voice<'a, 6, 32>,
    parameters: VoiceParameters,
    /// transit's restart crossfade (#263): the voice restarted keeping its phase, faded out over
    /// [`RESTART_FADE_RENDERS`] renders while the voice restarted with the reset fades in, and
    /// the frames of the fade done. Never set on the firmware's path.
    outgoing: Option<Voice<'a, 6, 32>>,
    faded: usize,
}

/// The restart crossfade lasts this many of a voice's renders (two blocks each): 48 frames, 1 ms
/// at 48 kHz.
pub const RESTART_FADE_RENDERS: usize = 2;

impl<'a> FmVoice<'a> {
    pub fn new() -> Self {
        Self {
            patch_index: None,
            struck_note: 0.0,
            lfo: Lfo::new(),
            voice: Voice::<'a, 6, 32>::new(),
            parameters: VoiceParameters::new(),
            outgoing: None,
            faded: 0,
        }
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
        self.struck_note = 0.0;
        self.outgoing = None;
        self.faded = 0;
    }

    /// The voice still sounds: its gate is high or an operator envelope is still moving.
    #[must_use]
    pub fn sounding(&self) -> bool {
        self.voice.sounding()
    }

    /// Restarts the voice at its next render (transit's same-pitch restart, #263).
    pub fn restart(&mut self) {
        self.voice.restart();
    }

    /// `LoadPatch`: nothing happens when the same patch is already loaded.
    pub fn load_patch(&mut self, index: usize, patch: &Patch) {
        if self.patch_index == Some(index) {
            return;
        }
        self.patch_index = Some(index);
        self.outgoing = None;
        self.voice.set_patch(Some(patch.clone()));
        self.lfo.set(&patch.modulations);
    }

    /// Renders `size` frames into `buffer`, which holds the voice's three buffers back to back
    /// (out, aux, temp), as the firmware's `Voice::Render(parameters, temp, size)`.
    ///
    /// transit's same-pitch restart (#263) with a phase reset: for [`RESTART_FADE_RENDERS`]
    /// renders the voice renders twice into `scratch` (six buffers of `size`), restarted keeping
    /// its phase and restarted with the reset, and adds a raised-cosine crossfade from the first
    /// to the second to the out buffer, so the reset makes no step. Never on the firmware's path.
    #[inline]
    pub fn render(&mut self, buffer: &mut [f32], size: usize, scratch: &mut [f32]) {
        if self.patch_index.is_none() {
            return;
        }

        if self.voice.restart_resets_phase(&self.parameters) {
            self.outgoing = Some(self.voice.keeping_phase());
            self.faded = 0;
        }

        if let Some(outgoing) = self.outgoing.as_mut() {
            let (fading_out, fading_in) = scratch.split_at_mut(3 * size);
            fading_out.fill(0.0);
            fading_in.fill(0.0);
            outgoing.render(&self.parameters, &Self::buffers(fading_out, size));
            self.voice.render(&self.parameters, &Self::buffers(fading_in, size));
            let total = (RESTART_FADE_RENDERS * size + 1) as f32;
            for (i, ((y, a), b)) in buffer[..size].iter_mut().zip(fading_out.iter()).zip(fading_in.iter()).enumerate() {
                let x = (self.faded + i + 1) as f32 / total;
                let w = 0.5 - 0.5 * (core::f32::consts::PI * x).cos();
                *y += *a + (*b - *a) * w;
            }
            self.faded += size;
            if self.faded >= RESTART_FADE_RENDERS * size {
                self.outgoing = None;
            }
            return;
        }

        self.voice.render(&self.parameters, &Self::buffers(buffer, size));
    }

    /// The voice's three buffers (out, aux, temp) at the start of `buffer`.
    fn buffers(buffer: &mut [f32], size: usize) -> [RefCell<&mut [f32]>; 3] {
        let (out, rest) = buffer.split_at_mut(size);
        let (aux, rest) = rest.split_at_mut(size);
        [RefCell::new(out), RefCell::new(aux), RefCell::new(&mut rest[..size])]
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
    /// Sets the voice's pitch reference (see [`crate::Tuning`]).
    pub fn set_tuning(&mut self, tuning: crate::Tuning) {
        self.voice.set_tuning(tuning);
    }

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

// ---- The state record (feature `testing`; ADR 0029, amendment for #742) ----

#[cfg(feature = "testing")]
impl crate::record::Record for FmVoice<'_> {
    /// Every field encoded. A present `outgoing` restored into a receiving voice without one
    /// starts from a copy of the receiving `voice` (whose algorithm table it shares), then takes
    /// every recorded field.
    fn record(&self, w: &mut crate::record::Writer<'_>) {
        let Self { patch_index, struck_note, lfo, voice, parameters, outgoing, faded } = self;
        patch_index.record(w);
        struck_note.record(w);
        lfo.record(w);
        voice.record(w);
        parameters.record(w);
        w.present(outgoing.as_ref(), |w, v| v.record(w));
        faded.record(w);
    }
    fn restore(&mut self, r: &mut crate::record::Reader<'_>) -> Option<()> {
        let Self { patch_index, struck_note, lfo, voice, parameters, outgoing, faded } = self;
        patch_index.restore(r)?;
        struck_note.restore(r)?;
        lfo.restore(r)?;
        voice.restore(r)?;
        parameters.restore(r)?;
        if r.bool()? {
            outgoing.get_or_insert_with(|| voice.clone()).restore(r)?;
        } else {
            *outgoing = None;
        }
        faded.restore(r)
    }
}

// `patches`: not encoded.
// - Derived while a six-operator model (2, 3 or 4) is the selected engine: the switch to it ran
//   `load_syx_bank` with that model's bank (`Voice::render`, the engine selection; also on a
//   resource reload), which unpacks all 32 patches, and nothing else writes them (`render` reads
//   them only through `load_patch`, which copies one). The banks are the voice's resources, the
//   integrated banks in transit (configuration: the voice's record refuses others). The voice's
//   restore rebuilds them with `unpack_bank`, the function `load_syx_bank` runs, from the
//   selected model's bank (`Voice`'s `Record::restore`).
// - Unobservable while no six-operator model is selected: every switch to one runs
//   `load_syx_bank` before the engine renders, which rewrites all 32. Witnesses: poisoned while
//   dormant, then switched to (`tests/record_witnesses.rs`); a transplant into a voice holding
//   another bank (same file).
#[cfg(feature = "testing")]
crate::record_struct!(['a] SixOpEngine<'a> { patch_index_quantizer, voice, temp_buffer, acc_buffer, restart_buffer, active_voice, rendered_voice, sample_rate, tuning, same_pitch_restart } derived { patches });

#[cfg(feature = "testing")]
impl SixOpEngine<'_> {
    /// Poison witnesses only: poisons the patches (while no six-operator model is selected).
    pub(crate) fn poison_patches(&mut self, p: &mut crate::record::Poison) {
        use crate::record::Record;
        self.patches.poison(p);
    }
}

//! The resonator (`rings::Part` in `rings/dsp/part.{h,cc}`): up to four voices of one of six
//! models, their excitation, the strummed note allocation, the reverb of the string-and-reverb
//! model and the output limiter.

#![forbid(unsafe_code)]

use crate::chords::CHORDS;
use crate::float::F;
use crate::fm_voice::FmVoice;
use crate::limiter::Limiter;
use crate::modal::ModalResonator;
use crate::note_filter::NoteFilter;
use crate::plucker::Plucker;
use crate::reverb::Reverb;
use crate::stmlib::cosine_oscillator::CosineOscillator;
use crate::stmlib::filter::{DcBlocker, FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::random::Random;
use crate::stmlib::{integral_fractional, semitones_to_ratio};
use crate::string::KarplusString;
use crate::{CHORD_COUNT, MAX_BLOCK_SIZE, MAX_POLYPHONY, SAMPLE_RATE};

/// The number of strings, `kNumStrings` (two per voice).
const NUM_STRINGS: usize = MAX_POLYPHONY * 2;

/// `a3`: A3 (MIDI 69 in the firmware's numbering) normalized to the sample rate. The firmware uses
/// the true 48 kHz here (no corrected rate), so note 69 plays 440 Hz.
const A3: f32 = 440.0 / SAMPLE_RATE;

/// The voice order odd polyphonies strum through (`kPingPattern`).
const PING_PATTERN: [usize; 8] = [1, 0, 2, 1, 0, 2, 1, 0];

/// The resonator models, in the firmware's order (`ResonatorModel`). Names describe what each does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResonatorModel {
    /// A bank of up to 64 band-pass modes.
    #[default]
    Modal,
    /// A plucked string with sympathetic strings tuned by structure.
    SympatheticStrings,
    /// A string with dispersion (stiffness, a curved bridge, delay noise).
    InharmonicString,
    /// A two-operator FM voice with an envelope follower.
    FmVoice,
    /// Sympathetic strings tuned to a chord of the chord table.
    QuantizedSympatheticStrings,
    /// The inharmonic string through a reverb.
    StringAndReverb,
}

impl ResonatorModel {
    /// Every model, in the firmware's order.
    pub const ALL: [Self; 6] =
        [Self::Modal, Self::SympatheticStrings, Self::InharmonicString, Self::FmVoice, Self::QuantizedSympatheticStrings, Self::StringAndReverb];

    /// The output gain the limiter applies (`model_gains_`).
    fn gain(self) -> F {
        F(match self {
            Self::Modal | Self::InharmonicString | Self::StringAndReverb => 1.4,
            Self::SympatheticStrings | Self::QuantizedSympatheticStrings => 1.0,
            Self::FmVoice => 0.7,
        })
    }

    fn is_sympathetic(self) -> bool {
        matches!(self, Self::SympatheticStrings | Self::QuantizedSympatheticStrings)
    }
}

/// The four macros (`Patch` in `rings/dsp/patch.h`), each 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    /// Structure: inharmonicity, stiffness, the sympathetic strings' intervals, the FM ratio.
    pub structure: f32,
    /// Brightness.
    pub brightness: f32,
    /// Damping (decay time).
    pub damping: f32,
    /// Position of the excitation or pickup (the FM voice's feedback).
    pub position: f32,
}

impl Default for Patch {
    fn default() -> Self {
        Self { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 }
    }
}

/// The performance inputs of one block (`PerformanceState` in `rings/dsp/performance_state.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerformanceState {
    /// A strum in this block: the next voice takes the note and the internal exciter fires.
    pub strum: bool,
    /// The part excites itself (nothing in the audio input).
    pub internal_exciter: bool,
    /// Nothing in the strum input (not read by the resonator; kept for the instrument).
    pub internal_strum: bool,
    /// Nothing in the V/OCT input (not read by the resonator; kept for the instrument).
    pub internal_note: bool,
    /// The tonic, semitones (the module's frequency knob, `12 + transpose`).
    pub tonic: f32,
    /// The note over the tonic, semitones (the V/OCT input).
    pub note: f32,
    /// Frequency modulation, semitones.
    pub fm: f32,
    /// The chord of the quantized sympathetic strings, 0 to [`CHORD_COUNT`] - 1.
    pub chord: i32,
}

/// The resonator: the firmware's part, at 48 kHz.
#[derive(Clone, Debug)]
pub struct Resonator {
    bypass: bool,
    dirty: bool,
    model: ResonatorModel,
    active_voice: usize,
    step_counter: usize,
    polyphony: usize,
    resonator: [ModalResonator; MAX_POLYPHONY],
    string: [KarplusString; NUM_STRINGS],
    lfo: [CosineOscillator; NUM_STRINGS],
    fm_voice: [FmVoice; MAX_POLYPHONY],
    excitation_filter: [Svf; MAX_POLYPHONY],
    dc_blocker: [DcBlocker; MAX_POLYPHONY],
    plucker: [Plucker; MAX_POLYPHONY],
    note: [F; MAX_POLYPHONY],
    note_filter: NoteFilter,
    resonator_input: [F; MAX_BLOCK_SIZE],
    sympathetic_resonator_input: [F; MAX_BLOCK_SIZE],
    noise_burst_buffer: [F; MAX_BLOCK_SIZE],
    out_buffer: [F; MAX_BLOCK_SIZE],
    aux_buffer: [F; MAX_BLOCK_SIZE],
    reverb: Reverb,
    limiter: Limiter,
    random: Random,
}

impl Default for Resonator {
    fn default() -> Self {
        Self::new()
    }
}

/// The voice-level inputs every model's renderer receives.
#[derive(Clone, Copy)]
struct VoiceInputs<'a> {
    voice: usize,
    performance: &'a PerformanceState,
    patch: &'a Patch,
    frequency: F,
    filter_cutoff: F,
    size: usize,
}

impl Resonator {
    /// A resonator as the firmware's `Part::Init` leaves it on the module's zeroed memory:
    /// polyphony 1, the modal model, not bypassed, the random state at its power-on value. Every
    /// buffer is allocated here; [`Resonator::process`] never allocates.
    #[must_use]
    pub fn new() -> Self {
        let mut part = Self {
            bypass: false,
            dirty: true,
            model: ResonatorModel::Modal,
            active_voice: 0,
            step_counter: 0,
            polyphony: 1,
            resonator: std::array::from_fn(|_| ModalResonator::default()),
            string: std::array::from_fn(|_| KarplusString::default()),
            lfo: [CosineOscillator::default(); NUM_STRINGS],
            fm_voice: [FmVoice::default(); MAX_POLYPHONY],
            excitation_filter: [Svf::default(); MAX_POLYPHONY],
            dc_blocker: [DcBlocker::default(); MAX_POLYPHONY],
            plucker: std::array::from_fn(|_| Plucker::default()),
            note: [F::ZERO; MAX_POLYPHONY],
            note_filter: NoteFilter::default(),
            resonator_input: [F::ZERO; MAX_BLOCK_SIZE],
            sympathetic_resonator_input: [F::ZERO; MAX_BLOCK_SIZE],
            noise_burst_buffer: [F::ZERO; MAX_BLOCK_SIZE],
            out_buffer: [F::ZERO; MAX_BLOCK_SIZE],
            aux_buffer: [F::ZERO; MAX_BLOCK_SIZE],
            reverb: Reverb::default(),
            limiter: Limiter::default(),
            random: Random::default(),
        };
        part.init();
        part
    }

    /// `Part::Init`. Members the firmware's `Init` does not write keep their values.
    fn init(&mut self) {
        self.active_voice = 0;
        self.note = [F::ZERO; MAX_POLYPHONY];
        self.bypass = false;
        self.polyphony = 1;
        self.model = ResonatorModel::Modal;
        self.dirty = true;
        for i in 0..MAX_POLYPHONY {
            self.excitation_filter[i].init();
            self.plucker[i].init();
            self.dc_blocker[i].init(1.0 - F(10.0) / SAMPLE_RATE);
        }
        self.reverb.init();
        self.limiter.init();
        self.note_filter.init(
            F(SAMPLE_RATE) / F::from_usize(MAX_BLOCK_SIZE),
            F(0.001), // Lag time with a sharp edge on the V/Oct input or trigger.
            F(0.010), // Lag time after the trigger has been received.
            F(0.050), // Time to transition from reactive to filtered.
            F(0.004), // Prevent a sharp edge to partly leak on the previous voice.
        );
    }

    /// Writes the random state (`stmlib::Random::Seed`); a new resonator starts from the
    /// firmware's power-on state, [`Random::DEFAULT_SEED`].
    pub fn seed(&mut self, seed: u32) {
        self.random = Random::new(seed);
    }

    /// The random state.
    #[must_use]
    pub fn rng_state(&self) -> u32 {
        self.random.state()
    }

    /// Whether the input is copied to the outputs unprocessed.
    #[must_use]
    pub fn bypass(&self) -> bool {
        self.bypass
    }

    /// `set_bypass`.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    /// The polyphony, 1 to [`MAX_POLYPHONY`].
    #[must_use]
    pub fn polyphony(&self) -> usize {
        self.polyphony
    }

    /// `set_polyphony`: capped at [`MAX_POLYPHONY`] (0 is taken as 1). New voices start a twentieth
    /// of a semitone apart above the first; the voices are re-initialized on the next block.
    pub fn set_polyphony(&mut self, polyphony: usize) {
        let old_polyphony = self.polyphony;
        self.polyphony = polyphony.clamp(1, MAX_POLYPHONY);
        for i in old_polyphony..self.polyphony {
            self.note[i] = self.note[0] + F::from_usize(i) * 0.05;
        }
        self.dirty = true;
    }

    /// The model.
    #[must_use]
    pub fn model(&self) -> ResonatorModel {
        self.model
    }

    /// `set_model`: the voices are re-initialized on the next block when the model changes.
    pub fn set_model(&mut self, model: ResonatorModel) {
        if model != self.model {
            self.model = model;
            self.dirty = true;
        }
    }

    /// `ConfigureResonators`.
    fn configure_resonators(&mut self) {
        if !self.dirty {
            return;
        }
        match self.model {
            ResonatorModel::Modal => {
                let resolution = 64 / self.polyphony - 4;
                for r in &mut self.resonator[..self.polyphony] {
                    r.init();
                    r.set_resolution(resolution);
                }
            }
            ResonatorModel::SympatheticStrings
            | ResonatorModel::InharmonicString
            | ResonatorModel::QuantizedSympatheticStrings
            | ResonatorModel::StringAndReverb => {
                // The eighth string's LFO is not listed in the firmware's initializer: it is zero.
                const LFO_FREQUENCIES: [f32; NUM_STRINGS] = [0.5, 0.4, 0.35, 0.23, 0.211, 0.2, 0.171, 0.0];
                let has_dispersion = matches!(self.model, ResonatorModel::InharmonicString | ResonatorModel::StringAndReverb);
                for ((string, lfo), lfo_frequency) in self.string.iter_mut().zip(&mut self.lfo).zip(LFO_FREQUENCIES) {
                    string.init(has_dispersion);
                    let f_lfo = F::from_usize(MAX_BLOCK_SIZE) / SAMPLE_RATE * lfo_frequency;
                    lfo.init_approximate(f_lfo);
                }
                for p in &mut self.plucker[..self.polyphony] {
                    p.init();
                }
            }
            ResonatorModel::FmVoice => {
                for v in &mut self.fm_voice[..self.polyphony] {
                    v.init();
                }
            }
        }
        if self.active_voice >= self.polyphony {
            self.active_voice = 0;
        }
        self.dirty = false;
    }

    /// The `Squash` curve of the sympathetic strings' interval interpolation.
    fn squash(x: F) -> F {
        if x < 0.5 {
            let mut x = x * 2.0;
            x *= x;
            x *= x;
            x *= x;
            x *= x;
            x * 0.5
        } else {
            let mut x = 2.0 - 2.0 * x;
            x *= x;
            x *= x;
            x *= x;
            x *= x;
            1.0 - 0.5 * x
        }
    }

    /// `ComputeSympatheticStringsNotes`: the notes of `destination.len()` strings.
    fn compute_sympathetic_strings_notes(&self, tonic: F, note: F, parameter: F, destination: &mut [F]) {
        let notes = [tonic, note - 12.0, note - 7.01955, note, note + 7.01955, note + 12.0, note + 19.01955, note + 24.0, note + 24.0];
        const DETUNINGS: [f32; 4] = [0.013, 0.011, 0.007, 0.017];
        let num_strings = destination.len();

        if parameter >= 2.0 {
            // Quantized chords.
            let chord_index = usize::try_from((parameter - 2.0).trunc_i32()).unwrap_or(0).min(CHORDS[0].len() - 1);
            let chord = &CHORDS[self.polyphony - 1][chord_index];
            for (d, c) in destination.iter_mut().zip(chord) {
                *d = *c + note;
            }
            return;
        }

        let num_detuned_strings = (num_strings - 1) >> 1;
        let first_detuned_string = num_strings - num_detuned_strings;
        let mut parameter = parameter;
        for i in 0..first_detuned_string {
            let mut string_note = F(3.0);
            if i != 0 {
                string_note = parameter * 7.0;
                parameter += (1.0 - parameter) * 0.2;
            }
            let (integral, fractional) = integral_fractional(string_note);
            let fractional = Self::squash(fractional);
            let integral = usize::try_from(integral).unwrap_or(0).min(notes.len() - 2);
            let a = notes[integral];
            let b = notes[integral + 1];
            destination[i] = a + (b - a) * fractional;
            if i + first_detuned_string < num_strings {
                destination[i + first_detuned_string] = destination[i] + DETUNINGS[i & 3];
            }
        }
    }

    fn render_modal_voice(&mut self, v: VoiceInputs<'_>) {
        let size = v.size;
        // The internal exciter is a pulse, before the filter.
        if v.performance.internal_exciter && v.voice == self.active_voice && v.performance.strum {
            self.resonator_input[0] += 0.25 * semitones_to_ratio(v.filter_cutoff * v.filter_cutoff * 24.0) / v.filter_cutoff;
        }
        self.excitation_filter[v.voice].process_block(&mut self.resonator_input[..size], FilterMode::LowPass);
        let r = &mut self.resonator[v.voice];
        r.set_frequency(v.frequency);
        r.set_structure(F(v.patch.structure));
        r.set_brightness(F(v.patch.brightness) * F(v.patch.brightness));
        r.set_position(F(v.patch.position));
        r.set_damping(F(v.patch.damping));
        r.process(&self.resonator_input[..size], &mut self.out_buffer[..size], &mut self.aux_buffer[..size]);
    }

    fn render_fm_voice(&mut self, v: VoiceInputs<'_>) {
        let size = v.size;
        let fm = &mut self.fm_voice[v.voice];
        if v.performance.internal_exciter && v.voice == self.active_voice && v.performance.strum {
            fm.trigger_internal_envelope();
        }
        fm.set_frequency(v.frequency);
        fm.set_ratio(F(v.patch.structure));
        fm.set_brightness(F(v.patch.brightness));
        fm.set_feedback_amount(F(v.patch.position));
        fm.set_position(F::ZERO);
        fm.set_damping(F(v.patch.damping));
        fm.process(&self.resonator_input[..size], &mut self.out_buffer[..size], &mut self.aux_buffer[..size]);
    }

    fn render_string_voice(&mut self, v: VoiceInputs<'_>) {
        let size = v.size;
        let patch = v.patch;
        let performance = v.performance;

        // The number of strings and their frequencies.
        let mut num_strings = 1;
        let mut frequencies = [F::ZERO; NUM_STRINGS];
        if self.model.is_sympathetic() {
            num_strings = 2 * MAX_POLYPHONY / self.polyphony;
            let parameter = if self.model == ResonatorModel::SympatheticStrings {
                F(patch.structure)
            } else {
                2.0 + F::from_i32(performance.chord.clamp(0, CHORD_COUNT - 1))
            };
            let tonic = F(performance.tonic) + performance.fm;
            let note = F(performance.tonic) + self.note[v.voice] + performance.fm;
            self.compute_sympathetic_strings_notes(tonic, note, parameter, &mut frequencies[..num_strings]);
            for f in &mut frequencies[..num_strings] {
                *f = semitones_to_ratio(*f - 69.0) * A3;
            }
        } else {
            frequencies[0] = v.frequency;
        }

        if v.voice == self.active_voice {
            let gain = 1.0 / (F::from_usize(num_strings) * 2.0).sqrt();
            for x in &mut self.resonator_input[..size] {
                *x *= gain;
            }
        }

        // The external input.
        self.excitation_filter[v.voice].process_block(&mut self.resonator_input[..size], FilterMode::LowPass);

        // The noise burst.
        if performance.internal_exciter {
            if v.voice == self.active_voice && performance.strum {
                self.plucker[v.voice].trigger(v.frequency, v.filter_cutoff * 8.0, F(patch.position));
            }
            self.plucker[v.voice].process(&mut self.noise_burst_buffer[..size], &mut self.random);
            for (x, n) in self.resonator_input[..size].iter_mut().zip(&self.noise_burst_buffer[..size]) {
                *x += *n;
            }
        }
        self.dc_blocker[v.voice].process(&mut self.resonator_input[..size]);

        self.out_buffer[..size].fill(F::ZERO);
        self.aux_buffer[..size].fill(F::ZERO);

        let structure = F(patch.structure);
        let dispersion = if structure < 0.24 {
            (structure - 0.24) * 4.166
        } else if structure > 0.26 {
            (structure - 0.26) * 1.35135
        } else {
            F::ZERO
        };

        for (string, &string_frequency) in frequencies[..num_strings].iter().enumerate() {
            let i = v.voice + string * self.polyphony;
            let lfo_value = self.lfo[i].next();

            let mut brightness = F(patch.brightness);
            let mut damping = F(patch.damping);
            let mut position = F(patch.position);
            let mut glide = F(1.0);
            let string_index = F::from_usize(string) / F::from_usize(num_strings);
            let mut sympathetic = false;

            if self.model == ResonatorModel::StringAndReverb {
                damping *= 2.0 - damping;
            }

            // With the internal exciter, string 0 is the main source and the others vibrate by
            // sympathetic resonance; without it, every string vibrates by sympathetic resonance.
            if string > 0 && performance.internal_exciter {
                brightness *= 2.0 - brightness;
                brightness *= 2.0 - brightness;
                damping = 0.7 + F(patch.damping) * 0.27;
                let amount = (0.5 - (0.5 - F(patch.position)).abs()) * 0.9;
                position = F(patch.position) + lfo_value * amount;
                glide = semitones_to_ratio((brightness - 1.0) * 36.0);
                sympathetic = true;
            }

            let s = &mut self.string[i];
            s.set_dispersion(dispersion);
            s.set_frequency_glide(string_frequency, glide);
            s.set_brightness(brightness);
            s.set_position(position);
            s.set_damping(damping + string_index * (0.95 - damping));
            let input = if sympathetic { &self.sympathetic_resonator_input[..size] } else { &self.resonator_input[..size] };
            s.process(input, &mut self.out_buffer[..size], &mut self.aux_buffer[..size], &mut self.random);

            if string == 0 {
                let gain = 0.2 / F::from_usize(num_strings);
                for ((y, o), a) in self.sympathetic_resonator_input[..size].iter_mut().zip(&self.out_buffer[..size]).zip(&self.aux_buffer[..size]) {
                    *y = gain * (*o - *a);
                }
            }
        }
    }

    /// `Part::Process` over one block of at most [`MAX_BLOCK_SIZE`] frames.
    fn process_block(&mut self, performance: &PerformanceState, patch: &Patch, input: &[f32], out: &mut [F], aux: &mut [F]) {
        let size = input.len();
        if self.bypass {
            for ((x, o), a) in input.iter().zip(out.iter_mut()).zip(aux.iter_mut()) {
                *o = F(*x);
                *a = F(*x);
            }
            return;
        }

        self.configure_resonators();

        self.note_filter.process(F(performance.note), performance.strum);
        if performance.strum {
            self.note[self.active_voice] = self.note_filter.stable_note();
            if self.polyphony > 1 && self.polyphony & 1 == 1 {
                self.active_voice = PING_PATTERN[self.step_counter % 8];
                self.step_counter = (self.step_counter + 1) % 8;
            } else {
                self.active_voice = (self.active_voice + 1) % self.polyphony;
            }
        }
        self.note[self.active_voice] = self.note_filter.note();

        out.fill(F::ZERO);
        aux.fill(F::ZERO);
        for voice in 0..self.polyphony {
            // The note, the frequency and the cutoff of the excitation filter.
            let brightness = F(patch.brightness);
            let cutoff = brightness * (2.0 - brightness);
            let note = self.note[voice] + performance.tonic + performance.fm;
            let frequency = semitones_to_ratio(note - 69.0) * A3;
            let filter_cutoff_range = if performance.internal_exciter {
                frequency * semitones_to_ratio((cutoff - 0.5) * 96.0)
            } else {
                0.4 * semitones_to_ratio((cutoff - 1.0) * 108.0)
            };
            let filter_cutoff = (if voice == self.active_voice { filter_cutoff_range } else { F(10.0) / SAMPLE_RATE }).min(F(0.499));
            let filter_q = if performance.internal_exciter { F(1.5) } else { F(0.8) };

            // The active voice takes the input; the others receive silence.
            self.excitation_filter[voice].set_f_q(FrequencyApproximation::Dirty, filter_cutoff, filter_q);
            if voice == self.active_voice {
                for (r, x) in self.resonator_input[..size].iter_mut().zip(input) {
                    *r = F(*x);
                }
            } else {
                self.resonator_input[..size].fill(F::ZERO);
            }

            let inputs = VoiceInputs { voice, performance, patch, frequency, filter_cutoff, size };
            match self.model {
                ResonatorModel::Modal => self.render_modal_voice(inputs),
                ResonatorModel::FmVoice => self.render_fm_voice(inputs),
                _ => self.render_string_voice(inputs),
            }

            if self.polyphony == 1 {
                // The two sets of harmonics or pickups go to separate outputs.
                for ((o, a), (bo, ba)) in out.iter_mut().zip(aux.iter_mut()).zip(self.out_buffer.iter().zip(&self.aux_buffer)) {
                    *o += *bo;
                    *a += *ba;
                }
            } else {
                // Odd and even voices go to separate outputs.
                let destination = if voice & 1 == 1 { &mut *aux } else { &mut *out };
                for (d, (bo, ba)) in destination.iter_mut().zip(self.out_buffer.iter().zip(&self.aux_buffer)) {
                    *d += *bo - *ba;
                }
            }
        }

        if self.model == ResonatorModel::StringAndReverb {
            let position = F(patch.position);
            for (o, a) in out.iter_mut().zip(aux.iter_mut()) {
                let l = *o;
                let r = *a;
                *o = l * position + (1.0 - position) * r;
                *a = r * position + (1.0 - position) * l;
            }
            let damping = F(patch.damping);
            self.reverb.set_amount(0.1 + damping * 0.5);
            self.reverb.set_diffusion(F(0.625));
            self.reverb.set_time(0.35 + 0.63 * damping);
            self.reverb.set_input_gain(F(0.2));
            self.reverb.set_lp(0.3 + F(patch.brightness) * 0.6);
            self.reverb.process(out, aux);
            for a in aux.iter_mut() {
                *a = -*a;
            }
        }

        // The limiter on the output.
        self.limiter.process(out, aux, self.model.gain());
    }

    /// Renders one block, as the firmware's `Part::Process`: `input` is the audio input (the
    /// exciter when [`PerformanceState::internal_exciter`] is false), `out` and `aux` receive the
    /// two outputs (overwritten), floats at full scale 1.0 before the module's int16 conversion.
    ///
    /// Render contract: `input`, `out` and `aux` have the same length, 1 to [`MAX_BLOCK_SIZE`]
    /// (24) frames, and one call is one control block (the note filter and the strum advance once
    /// per call, as on the module, which always renders 24 frames); the patch values are 0 to 1,
    /// and the notes are finite and keep every string within -128 to 127 semitones. Checked with
    /// `debug_assert!`. In a release build a longer slice is rendered as consecutive blocks of 24
    /// frames with the strum on the first only, a mismatched length renders the shortest of the
    /// three, and an empty slice renders nothing. The patch is used as given (the module's
    /// structure knob stops at 0.9995).
    pub fn process(&mut self, performance: &PerformanceState, patch: &Patch, input: &[f32], out: &mut [f32], aux: &mut [f32]) {
        debug_assert!(input.len() == out.len() && input.len() == aux.len(), "input, out and aux must have the same length");
        debug_assert!((1..=MAX_BLOCK_SIZE).contains(&input.len()), "block of {} frames, expected 1 to {MAX_BLOCK_SIZE}", input.len());
        debug_assert!(performance.note.is_finite() && performance.tonic.is_finite() && performance.fm.is_finite(), "notes must be finite");
        debug_assert!((0..CHORD_COUNT).contains(&performance.chord), "chord {} outside 0 to {}", performance.chord, CHORD_COUNT - 1);
        let n = input.len().min(out.len()).min(aux.len());
        let mut performance = *performance;
        let mut start = 0;
        while start < n {
            let end = (start + MAX_BLOCK_SIZE).min(n);
            let mut o = [F::ZERO; MAX_BLOCK_SIZE];
            let mut a = [F::ZERO; MAX_BLOCK_SIZE];
            let len = end - start;
            self.process_block(&performance, patch, &input[start..end], &mut o[..len], &mut a[..len]);
            for (dst, src) in out[start..end].iter_mut().zip(&o[..len]) {
                *dst = src.0;
            }
            for (dst, src) in aux[start..end].iter_mut().zip(&a[..len]) {
                *dst = src.0;
            }
            performance.strum = false;
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn performance() -> PerformanceState {
        PerformanceState { strum: true, internal_exciter: true, tonic: 36.0, note: 24.0, ..PerformanceState::default() }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "block of 25 frames")]
    fn a_block_longer_than_the_firmware_block_is_refused_in_debug() {
        let mut part = Resonator::new();
        let (input, mut out, mut aux) = ([0.0; 25], [0.0; 25], [0.0; 25]);
        part.process(&performance(), &Patch::default(), &input, &mut out, &mut aux);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "same length")]
    fn mismatched_lengths_are_refused_in_debug() {
        let mut part = Resonator::new();
        let (input, mut out, mut aux) = ([0.0; 24], [0.0; 24], [0.0; 12]);
        part.process(&performance(), &Patch::default(), &input, &mut out, &mut aux);
    }

    #[test]
    fn bypass_copies_the_input_to_both_outputs() {
        let mut part = Resonator::new();
        part.set_bypass(true);
        let input: [f32; 24] = std::array::from_fn(|i| i as f32 * 0.01);
        let (mut out, mut aux) = ([1.0; 24], [1.0; 24]);
        part.process(&performance(), &Patch::default(), &input, &mut out, &mut aux);
        assert_eq!(out, input);
        assert_eq!(aux, input);
    }

    #[test]
    fn polyphony_is_one_to_four_and_new_voices_start_detuned() {
        let mut part = Resonator::new();
        part.set_polyphony(0);
        assert_eq!(part.polyphony(), 1);
        part.set_polyphony(9);
        assert_eq!(part.polyphony(), MAX_POLYPHONY);
        for i in 1..MAX_POLYPHONY {
            assert_eq!(part.note[i], part.note[0] + F::from_usize(i) * 0.05);
        }
    }

    #[test]
    fn a_strum_rings_every_model() {
        for model in ResonatorModel::ALL {
            let mut part = Resonator::new();
            part.set_model(model);
            let input = [0.0; 24];
            let (mut out, mut aux) = ([0.0; 24], [0.0; 24]);
            let mut peak = 0.0_f32;
            for block in 0..100 {
                let p = PerformanceState { strum: block == 0, ..performance() };
                part.process(&p, &Patch::default(), &input, &mut out, &mut aux);
                peak = out.iter().chain(&aux).fold(peak, |m, s| m.max(s.abs()));
            }
            assert!(peak > 1e-3 && peak <= 1.0, "{model:?}: peak {peak}");
        }
    }
}

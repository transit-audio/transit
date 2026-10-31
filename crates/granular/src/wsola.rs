//! The pitch shifter and time stretcher's player (`clouds/dsp/wsola_sample_player.h`, with
//! `window.h`): two overlapping triangular windows read the recording at the pitch ratio; each new
//! window starts where the correlator ([`crate::correlator`]) found it lines up best with the one
//! fading out (WSOLA, waveform-similarity overlap-add), so the pitch changes without the speed and
//! the position moves without the pitch.

#![forbid(unsafe_code)]

use crate::audio_buffer::{AudioBuffer, Interpolation};
use crate::correlator::Correlator;
use crate::float::F;
use crate::stmlib::{constrain, semitones_to_ratio};

/// `kMaxWSOLASize`: the longest window, samples.
pub(crate) const MAX_WSOLA_SIZE: i32 = 4096;

/// What the player reads from the processor's parameters each block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WsolaParameters {
    pub(crate) position: F,
    pub(crate) size: F,
    pub(crate) pitch: F,
    pub(crate) trigger: bool,
}

/// `Window`: a triangular window over the recording, read at a fixed phase increment.
///
/// At `08460a69` `Start` no longer clears `done_` (two commits each removed one of two duplicate
/// lines, and a merge kept both removals), so no window ever starts and the mode is silent. The
/// oracle restores the line on a build-time copy (`patches/all/0001` in the oracle crate) and so
/// does the port: [`Window::start`] clears `done`.
#[derive(Clone, Copy, Debug, Default)]
struct Window {
    first_sample: i32,
    phase: i32,
    phase_increment: i32,
    envelope_phase_increment: F,
    done: bool,
    half: bool,
    regenerated: bool,
}

impl Window {
    /// `Init()`.
    fn init(&mut self) {
        self.done = true;
        self.regenerated = false;
        self.half = false;
    }

    /// `Start(buffer_size, start, width, phase_increment)`, with `done_ = false` restored.
    fn start(&mut self, buffer_size: i32, start: i32, width: i32, phase_increment: i32) {
        // C++ `%` truncates toward zero, as Rust's does.
        self.first_sample = start.wrapping_add(buffer_size) % buffer_size.max(1);
        self.phase_increment = phase_increment;
        self.phase = 0;
        self.done = false;
        self.regenerated = false;
        self.envelope_phase_increment = F(2.0) / F::from_i32(width);
    }

    /// `OverlapAdd(buffer, samples, channels)`: one sample of the window added to `frame`.
    #[inline]
    fn overlap_add(&mut self, buffers: &[AudioBuffer; 2], frame: &mut [F; 2], stereo: bool) {
        if self.done {
            return;
        }
        let phase_integral = self.phase >> 16;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let phase_fractional = (self.phase & 0xffff) as u16;
        let sample_index = self.first_sample.wrapping_add(phase_integral);

        let envelope_phase = F::from_i32(phase_integral) * self.envelope_phase_increment;
        self.done = envelope_phase >= 2.0;
        self.half = envelope_phase >= 1.0;
        let gain = if envelope_phase >= 1.0 { 2.0 - envelope_phase } else { envelope_phase };

        let l = buffers[0].read(Interpolation::Hermite, sample_index, phase_fractional) * gain;
        if stereo {
            let r = buffers[1].read(Interpolation::Hermite, sample_index, phase_fractional) * gain;
            frame[0] += l;
            frame[1] += r;
        } else {
            frame[0] += l;
            frame[1] += l;
        }
        self.phase = self.phase.wrapping_add(self.phase_increment);
    }

    fn needs_regeneration(&self) -> bool {
        self.half && !self.regenerated
    }

    fn mark_as_regenerated(&mut self) {
        self.regenerated = true;
    }
}

/// `WSOLASamplePlayer`. Every field starts at zero, as on the module, until [`WsolaPlayer::init`].
#[derive(Clone, Debug, Default)]
pub(crate) struct WsolaPlayer {
    windows: [Window; 2],
    window_size: i32,
    stereo: bool,
    pitch: F,
    smoothed_pitch: F,
    position: F,
    size_factor: F,
    next_pitch_ratio: F,
    correlator_loaded: bool,
    search_source: i32,
    search_target: i32,
    env_phase: F,
    env_phase_increment: F,
    elapsed: i32,
}

impl WsolaPlayer {
    /// `Init(correlator, num_channels)`.
    pub(crate) fn init(&mut self, stereo: bool) {
        self.stereo = stereo;
        self.pitch = F::ZERO;
        self.position = F::ZERO;
        self.smoothed_pitch = F::ZERO;
        self.windows[0].init();
        self.windows[1].init();
        self.next_pitch_ratio = F(1.0);
        self.correlator_loaded = true;
        self.search_source = 0;
        self.search_target = 0;
        self.window_size = MAX_WSOLA_SIZE / 2;
        self.env_phase = F::ZERO;
        self.env_phase_increment = F(0.5);
        self.elapsed = 0;
    }

    /// `Play(buffer, parameters, out, size)`: `out` is overwritten.
    pub(crate) fn play(&mut self, buffers: &[AudioBuffer; 2], correlator: &mut Correlator, parameters: &WsolaParameters, out: &mut [[F; 2]]) {
        self.elapsed = self.elapsed.wrapping_add(1);
        if parameters.trigger {
            self.env_phase = F::ZERO;
            self.env_phase_increment = constrain(F(1.0) / F::from_i32(self.elapsed), F(0.0001), F(0.1));
            self.elapsed = 0;
        }
        self.env_phase += self.env_phase_increment;
        if self.env_phase >= 1.0 {
            self.env_phase = F(1.0);
        }
        self.position = parameters.position;
        self.position += (1.0 - self.env_phase) * (1.0 - self.position);

        self.pitch = parameters.pitch;
        self.size_factor = parameters.size;

        if self.windows[0].done && self.windows[1].done {
            self.windows[1].mark_as_regenerated();
            self.schedule_aligned_window(buffers, correlator, 0);
        }

        for frame in out.iter_mut() {
            // Sum the two windows.
            *frame = [F::ZERO; 2];
            for w in &mut self.windows {
                w.overlap_add(buffers, frame, self.stereo);
            }
            // Regenerate expired windows.
            for i in 0..2 {
                if self.windows[i].needs_regeneration() {
                    self.windows[i].mark_as_regenerated();
                    self.schedule_aligned_window(buffers, correlator, 1 - i);
                    self.windows[1 - i].overlap_add(buffers, frame, self.stereo);
                }
            }
        }
    }

    /// `ReadSignBits<num_channels>(buffer, phase_increment, source, size, destination)`: the sign
    /// of the (summed) recording at `source`, read every `phase_increment` (16.16) for `size`
    /// samples, packed 32 to a word, the last word padded; returns the bits written, a multiple of
    /// 32.
    fn read_sign_bits(&self, buffers: &[AudioBuffer; 2], phase_increment: i32, source: i32, size: i32, destination: &mut [u32]) -> i32 {
        let mut phase = 0_i32;
        let mut bits = 0_u32;
        let mut bit_counter = 0_u32;
        let mut num_samples = 0_i32;
        let source = if source < 0 { source.wrapping_add(buffers[0].size()) } else { source };
        let mut store = |bit_counter: u32, bits: u32, num_samples: &mut i32| {
            if bit_counter & 0x1f == 0x1f {
                if let Some(w) = usize::try_from(bit_counter >> 5).ok().and_then(|i| destination.get_mut(i)) {
                    *w = bits;
                }
                *num_samples += 32;
            }
        };
        while (phase >> 16) < size {
            let integral = source.wrapping_add(phase >> 16);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let fractional = (phase & 0xffff) as u16;
            let mut s = buffers[0].read(Interpolation::Linear, integral, fractional);
            if self.stereo {
                s += buffers[1].read(Interpolation::Linear, integral, fractional);
            }
            bits |= u32::from(s > 0.0);
            store(bit_counter, bits, &mut num_samples);
            bit_counter = bit_counter.wrapping_add(1);
            bits <<= 1;
            phase = phase.wrapping_add(phase_increment);
        }
        while bit_counter & 0x1f != 0 {
            store(bit_counter, bits, &mut num_samples);
            bit_counter = bit_counter.wrapping_add(1);
            bits <<= 1;
        }
        num_samples
    }

    /// `LoadCorrelator(buffer)`: after a window was scheduled, the sign bits of the window just
    /// started (the source) and of the stretch around the next target (the destination) go to the
    /// correlator, and a new search starts.
    pub(crate) fn load_correlator(&mut self, buffers: &[AudioBuffer; 2], correlator: &mut Correlator) {
        if self.correlator_loaded {
            return;
        }
        let stride = constrain(F::from_i32(self.window_size) / 2048.0, F(1.0), F(2.0)) * 65536.0;
        let ratio = if self.next_pitch_ratio < 1.25 { F(1.25) } else { self.next_pitch_ratio };
        let increment = (stride * ratio).trunc_i32();
        let num_samples = self.read_sign_bits(buffers, increment, self.search_source, self.window_size, correlator.source_mut());
        self.read_sign_bits(
            buffers,
            increment,
            self.search_target.wrapping_sub(self.window_size),
            self.window_size.wrapping_mul(2),
            correlator.destination_mut(),
        );
        correlator.start_search(num_samples, self.search_target - self.window_size + (self.window_size >> 1), increment);
        self.correlator_loaded = true;
    }

    /// `ScheduleAlignedWindow(buffer, window)`: window `index` starts at the correlator's best
    /// match; then the pitch (slewed by at most an octave per window), the window size (slewed by a
    /// 32nd of the change) and the next target are set for the next search.
    fn schedule_aligned_window(&mut self, buffers: &[AudioBuffer; 2], correlator: &Correlator, index: usize) {
        let buffer = &buffers[0];
        let next_window_position = correlator.best_match();
        self.correlator_loaded = false;
        #[allow(clippy::cast_possible_wrap)]
        let phase_increment = (self.next_pitch_ratio * 65536.0).trunc_u32() as i32;
        self.windows[index].start(buffer.size(), next_window_position.wrapping_sub(self.window_size >> 1), self.window_size, phase_increment);

        let mut pitch_error = self.pitch - self.smoothed_pitch;
        let pitch_error_sign = if pitch_error < 0.0 { F(-1.0) } else { F(1.0) };
        pitch_error *= pitch_error_sign;
        if pitch_error >= 12.0 {
            pitch_error = F(12.0);
        }
        self.smoothed_pitch += pitch_error * pitch_error_sign;
        let pitch_ratio = semitones_to_ratio(self.smoothed_pitch);
        let inv_pitch_ratio = semitones_to_ratio(-self.smoothed_pitch);
        self.next_pitch_ratio = pitch_ratio;

        let size_factor = semitones_to_ratio((self.size_factor - 1.0) * 60.0);
        let mut new_window_size = (size_factor * F::from_i32(MAX_WSOLA_SIZE)).trunc_i32();
        if new_window_size.wrapping_sub(self.window_size).wrapping_abs() > 64 {
            let error = new_window_size.wrapping_sub(self.window_size) >> 5;
            new_window_size = self.window_size.wrapping_add(error);
            self.window_size = new_window_size - new_window_size % 4;
        }

        // The center offset of the window we want to mix in.
        let mut limit = buffer.size();
        limit = limit.wrapping_sub((2.0 * F::from_i32(self.window_size) * inv_pitch_ratio).trunc_i32());
        limit = limit.wrapping_sub(2_i32.wrapping_mul(self.window_size));
        if limit < 0 {
            limit = 0;
        }

        let mut target_position = buffer.head();
        target_position = target_position.wrapping_sub((F::from_i32(limit) * self.position).trunc_i32());
        target_position = target_position.wrapping_sub(self.window_size);

        self.search_source = next_window_position;
        self.search_target = target_position;
    }
}

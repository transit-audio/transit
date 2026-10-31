//! The vocoder (`warps/dsp/{filter_bank,vocoder,limiter}.{h,cc}`): a 20-band analysis filter bank
//! on the modulator and on the carrier (the lowest 13 bands at 8 kHz behind 3x and 4x decimators,
//! the next six at 32 kHz, the top band at 96 kHz), an envelope follower per band, the carrier's
//! bands shaped by the modulator's envelopes (with a formant shift across bands), resynthesis and
//! a limiter.

#![forbid(unsafe_code)]

use crate::converter::{Down, Up};
use crate::float::F;
use crate::resources::{FILTER_BANK, SRC_DOWN_3_36, SRC_DOWN_4_48, SRC_UP_3_36, SRC_UP_4_48};
use crate::stmlib::filter::{CrossoverSvf, FilterMode};
use crate::stmlib::{constrain, integral_fractional, semitones_to_ratio, slope, soft_limit};

/// `kNumBands`.
pub(crate) const NUM_BANDS: usize = 20;
/// `kMaxFilterBankBlockSize`.
const MAX_BLOCK: usize = 96;
/// `kDelayLineSize`.
const DELAY_LINE_SIZE: usize = 6144;
/// `kSampleMemorySize`.
const SAMPLE_MEMORY_SIZE: usize = MAX_BLOCK * NUM_BANDS / 2;
/// `kLowFactor`, `kMidFactor`.
const LOW_FACTOR: usize = 4;
const MID_FACTOR: usize = 3;

/// A band (`Band`): its decimation group and rate, post gain, the two crossover stages, the span
/// of the shared sample memory it uses and its delay line in the shared delay memory.
#[derive(Clone, Copy, Debug, Default)]
struct Band {
    group: usize,
    sample_rate: F,
    post_gain: F,
    svf: [CrossoverSvf; 2],
    decimation_factor: usize,
    samples: usize,
    /// `PooledDelayLine`: start in the delay memory, size (delay + 1), head.
    delay_start: usize,
    delay_size: usize,
    delay_head: usize,
}

/// `FilterBank`.
#[derive(Clone, Debug)]
pub(crate) struct FilterBank {
    mid_down: Down<MID_FACTOR, 36>,
    mid_up: Up<MID_FACTOR, 12>,
    low_down: Down<LOW_FACTOR, 48>,
    low_up: Up<LOW_FACTOR, 12>,
    /// `tmp_[0]` (the 32 kHz signal) and `tmp_[1]` (the 8 kHz signal).
    tmp: [[F; MAX_BLOCK]; 2],
    samples: [F; SAMPLE_MEMORY_SIZE],
    delay: Box<[F; DELAY_LINE_SIZE]>,
    bands: [Band; NUM_BANDS],
}

impl FilterBank {
    pub(crate) fn new() -> Self {
        Self {
            mid_down: Down::new(&SRC_DOWN_3_36),
            mid_up: Up::new(&SRC_UP_3_36),
            low_down: Down::new(&SRC_DOWN_4_48),
            low_up: Up::new(&SRC_UP_4_48),
            tmp: [[F::ZERO; MAX_BLOCK]; 2],
            samples: [F::ZERO; SAMPLE_MEMORY_SIZE],
            delay: Box::new([F::ZERO; DELAY_LINE_SIZE]),
            bands: [Band::default(); NUM_BANDS],
        }
    }

    /// `Init(sample_rate)`: the bands from the table, their delay lines compensating the
    /// decimators so every band comes back aligned.
    pub(crate) fn init(&mut self, sample_rate: f32) {
        self.low_down.init();
        self.low_up.init();
        self.mid_down.init();
        self.mid_up.init();
        let mut max_delay = 0_i32;
        let mut samples = 0;
        let mut group: i32 = -1;
        let mut decimation_factor = 0;
        for (i, b) in self.bands.iter_mut().enumerate() {
            let c = &FILTER_BANK[i * 7..i * 7 + 7];
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 12, 3 or 1.
            let factor = c[0] as usize;
            if factor != decimation_factor {
                decimation_factor = factor;
                group += 1;
            }
            b.decimation_factor = factor;
            b.group = usize::try_from(group).unwrap_or(0);
            b.sample_rate = F(sample_rate) / F::from_usize(factor);
            b.samples = samples;
            samples += MAX_BLOCK / factor;
            #[allow(clippy::cast_possible_truncation)] // Small whole numbers.
            let delay = c[1] as i32 * i32::try_from(factor).unwrap_or(1);
            b.post_gain = F(c[2]);
            max_delay = max_delay.max(delay);
            for (pass, svf) in b.svf.iter_mut().enumerate() {
                svf.init();
                svf.set_f_fq(F(c[pass * 2 + 3]), F(c[pass * 2 + 4]));
            }
            // The band's own delay, kept for the compensation below.
            b.delay_size = usize::try_from(delay).unwrap_or(0);
        }
        let max_delay = max_delay.min(256);
        let low = i32::try_from(LOW_FACTOR * (Down::<LOW_FACTOR, 48>::delay() + Up::<LOW_FACTOR, 12>::delay())).unwrap_or(0);
        let mid = i32::try_from(Down::<MID_FACTOR, 36>::delay() + Up::<MID_FACTOR, 12>::delay()).unwrap_or(0);
        let mut start = 0;
        for b in &mut self.bands {
            let own = i32::try_from(b.delay_size).unwrap_or(0);
            let mut compensation = max_delay - own;
            if b.group == 0 {
                compensation -= low;
                compensation -= mid;
            } else if b.group == 1 {
                compensation -= mid;
            }
            let factor = i32::try_from(b.decimation_factor).unwrap_or(1);
            compensation = (compensation - factor / 2).max(0);
            let delay = usize::try_from(compensation / factor).unwrap_or(0);
            b.delay_start = start;
            b.delay_size = delay + 1;
            b.delay_head = 0;
            self.delay[start..start + delay + 1].fill(F::ZERO);
            start += delay + 1;
        }
    }

    /// The band's rate, Hz.
    fn band_rate(&self, i: usize) -> F {
        self.bands[i].sample_rate
    }

    /// `Analyze(in, size)`.
    pub(crate) fn analyze(&mut self, input: &[F]) {
        let size = input.len();
        let [tmp_0, tmp_1] = &mut self.tmp;
        self.mid_down.process(input, &mut tmp_0[..size / MID_FACTOR]);
        self.low_down.process(&tmp_0[..size / MID_FACTOR], &mut tmp_1[..size / MID_FACTOR / LOW_FACTOR]);
        for (i, b) in self.bands.iter_mut().enumerate() {
            let band_size = size / b.decimation_factor;
            let source: &[F] = match b.group {
                0 => &tmp_1[..band_size],
                1 => &tmp_0[..band_size],
                _ => &input[..band_size],
            };
            let mode = if i == 0 {
                FilterMode::LowPass
            } else if i == NUM_BANDS - 1 {
                FilterMode::HighPass
            } else {
                FilterMode::BandPassNormalized
            };
            let out = &mut self.samples[b.samples..b.samples + band_size];
            b.svf[0].process(mode, source, out);
            b.svf[1].process_in_place(mode, out);
            for o in out.iter_mut() {
                *o *= b.post_gain;
            }
        }
    }

    /// The samples band `i` holds after `analyze` (the carrier's are shaped in place).
    fn band_samples(&mut self, i: usize, size: usize) -> &mut [F] {
        let b = &self.bands[i];
        let n = size / b.decimation_factor;
        &mut self.samples[b.samples..b.samples + n]
    }

    /// `Synthesize(out, size)`.
    pub(crate) fn synthesize(&mut self, out: &mut [F]) {
        let size = out.len();
        let first = size / self.bands[0].decimation_factor;
        self.tmp[1][..first].fill(F::ZERO);
        for i in 0..NUM_BANDS {
            let b = self.bands[i];
            let band_size = size / b.decimation_factor;
            let next_group = self.bands.get(i + 1).map_or(b.group + 1, |n| n.group);
            {
                let samples = &self.samples[b.samples..b.samples + band_size];
                let line = &mut self.delay[b.delay_start..b.delay_start + b.delay_size];
                let mut head = b.delay_head;
                let s: &mut [F] = match b.group {
                    0 => &mut self.tmp[1][..band_size],
                    1 => &mut self.tmp[0][..band_size],
                    _ => &mut out[..band_size],
                };
                for (y, x) in s.iter_mut().zip(samples) {
                    // `PooledDelayLine::ReadWrite`.
                    line[head] = *x;
                    head = (head + 1) % line.len();
                    *y += line[head];
                }
                self.bands[i].delay_head = head;
            }
            if next_group != b.group {
                let [tmp_0, tmp_1] = &mut self.tmp;
                if b.group == 0 {
                    self.low_up.process(&tmp_1[..band_size], &mut tmp_0[..band_size * LOW_FACTOR]);
                } else if b.group == 1 {
                    self.mid_up.process(&tmp_0[..band_size], &mut out[..band_size * MID_FACTOR]);
                }
            }
        }
    }
}

/// `EnvelopeFollower`.
#[derive(Clone, Copy, Debug, Default)]
struct EnvelopeFollower {
    attack: F,
    decay: F,
    envelope: F,
    peak: F,
    freeze: bool,
}

impl EnvelopeFollower {
    fn init(&mut self) {
        self.envelope = F::ZERO;
        self.freeze = false;
        self.attack = F(0.1);
        self.decay = F(0.1);
        self.peak = F::ZERO;
    }

    fn process(&mut self, input: &[F], out: &mut [F], gain: F) {
        let mut envelope = self.envelope;
        let attack = if self.freeze { F::ZERO } else { self.attack };
        let decay = if self.freeze { F::ZERO } else { self.decay };
        let mut peak = F::ZERO;
        for (x, o) in input.iter().zip(out.iter_mut()) {
            let error = (*x * gain).abs() - envelope;
            envelope += (if error > 0.0 { attack } else { decay }) * error;
            if envelope > peak {
                peak = envelope;
            }
            *o = envelope;
        }
        self.envelope = envelope;
        let error = peak - self.peak;
        self.peak += F(if error > 0.0 { 0.5 } else { 0.1 }) * error;
    }
}

/// `BandGain`.
#[derive(Clone, Copy, Debug, Default)]
struct BandGain {
    carrier: F,
    vocoder: F,
}

/// `Vocoder`.
#[derive(Clone, Debug)]
pub(crate) struct Vocoder {
    release_time: F,
    formant_shift: F,
    previous_gain: [BandGain; NUM_BANDS],
    gain: [BandGain; NUM_BANDS],
    tmp: [F; MAX_BLOCK],
    modulator_bank: FilterBank,
    carrier_bank: FilterBank,
    /// `Limiter::peak_`.
    limiter_peak: F,
    follower: [EnvelopeFollower; NUM_BANDS],
    /// `kFollowerGain`, `sqrtf(kNumBands)`.
    follower_gain: F,
}

impl Vocoder {
    pub(crate) fn new() -> Self {
        Self {
            release_time: F::ZERO,
            formant_shift: F::ZERO,
            previous_gain: [BandGain::default(); NUM_BANDS],
            gain: [BandGain::default(); NUM_BANDS],
            tmp: [F::ZERO; MAX_BLOCK],
            modulator_bank: FilterBank::new(),
            carrier_bank: FilterBank::new(),
            limiter_peak: F::ZERO,
            follower: [EnvelopeFollower::default(); NUM_BANDS],
            #[allow(clippy::cast_precision_loss)]
            follower_gain: F((NUM_BANDS as f32).sqrt()),
        }
    }

    /// `Init(sample_rate)`.
    pub(crate) fn init(&mut self, sample_rate: f32) {
        self.modulator_bank.init(sample_rate);
        self.carrier_bank.init(sample_rate);
        self.limiter_peak = F(0.5);
        self.release_time = F(0.5);
        self.formant_shift = F(0.5);
        self.previous_gain = [BandGain::default(); NUM_BANDS];
        self.gain = [BandGain::default(); NUM_BANDS];
        self.follower.iter_mut().for_each(EnvelopeFollower::init);
    }

    pub(crate) fn set_release_time(&mut self, release_time: F) {
        self.release_time = release_time;
    }

    pub(crate) fn set_formant_shift(&mut self, formant_shift: F) {
        self.formant_shift = formant_shift;
    }

    /// `Process(modulator, carrier, out, size)`; `out` may be the carrier's own buffer in the
    /// firmware, so the carrier is read before `out` is written.
    pub(crate) fn process(&mut self, modulator: &[F], carrier: &[F], out: &mut [F]) {
        let size = out.len();
        self.modulator_bank.analyze(modulator);
        self.carrier_bank.analyze(carrier);

        let mut f = 80.0 * semitones_to_ratio(-72.0 * self.release_time);
        for i in 0..NUM_BANDS {
            let decay = f / self.modulator_bank.band_rate(i);
            let follower = &mut self.follower[i];
            follower.attack = decay * 2.0;
            follower.decay = decay * 0.5;
            follower.freeze = self.release_time > 0.995;
            f *= 1.2599;
        }

        let mut formant_shift_amount = 2.0 * (self.formant_shift - 0.5).abs();
        formant_shift_amount *= 2.0 - formant_shift_amount;
        formant_shift_amount *= 2.0 - formant_shift_amount;
        let envelope_increment = 4.0 * semitones_to_ratio(-48.0 * self.formant_shift);
        let mut envelope = F::ZERO;
        #[allow(clippy::cast_precision_loss)]
        let last_band = F(NUM_BANDS as f32 - 1.0001);
        for i in 0..NUM_BANDS {
            let source_band = constrain(envelope, F::ZERO, last_band);
            let (integral, fractional) = integral_fractional(source_band);
            let k = usize::try_from(integral).unwrap_or(0).min(NUM_BANDS - 2);
            let a = self.follower[k].peak;
            let b = self.follower[k + 1].peak;
            let mut band_gain = a + (b - a) * fractional;
            let attenuation = envelope - last_band;
            if attenuation >= 0.0 {
                band_gain *= 1.0 / (1.0 + 1.0 * attenuation);
            }
            envelope += envelope_increment;
            self.gain[i] = BandGain { carrier: band_gain * formant_shift_amount, vocoder: 1.0 - formant_shift_amount };
        }

        for i in 0..NUM_BANDS {
            let band_size = size / self.modulator_bank.bands[i].decimation_factor;
            let step = 1.0 / F::from_usize(band_size);
            let gain = self.follower_gain;
            let envelope = &mut self.tmp[..band_size];
            self.follower[i].process(self.modulator_bank.band_samples(i, size), envelope, gain);
            let mut vocoder_gain = self.previous_gain[i].vocoder;
            let vocoder_gain_increment = (self.gain[i].vocoder - vocoder_gain) * step;
            let mut carrier_gain = self.previous_gain[i].carrier;
            let carrier_gain_increment = (self.gain[i].carrier - carrier_gain) * step;
            for (c, e) in self.carrier_bank.band_samples(i, size).iter_mut().zip(envelope.iter()) {
                *c *= carrier_gain + vocoder_gain * *e;
                vocoder_gain += vocoder_gain_increment;
                carrier_gain += carrier_gain_increment;
            }
            self.previous_gain[i] = self.gain[i];
        }

        self.carrier_bank.synthesize(out);
        // `Limiter::Process(out, 1.4f, size)`.
        for x in out.iter_mut() {
            let s = *x * 1.4;
            slope(&mut self.limiter_peak, s.abs(), F(0.05), F(0.00002));
            let gain = if self.limiter_peak <= 1.0 { F::ONE } else { 1.0 / self.limiter_peak };
            *x = soft_limit(s * gain * 0.8);
        }
    }
}

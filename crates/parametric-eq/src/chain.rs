//! The processor (ADR 0054 decisions 3, 4 and 12): 24 band slots on the processor's own sample
//! clock, a left/right stage then a mid/side stage, the Output gain and the applied gain match, all in
//! f64 from input to output.
//!
//! Every 16 samples of the clock a control tick advances the smoothers, redesigns moving bands (whose
//! coefficients then glide per sample to the new design, see `band`), decides ramps and configuration
//! switches, decides whether the mid/side stage runs, and advances the gain-match refresh. Between ticks the bands run band by band over the samples, in band-number
//! order within each stage; the arithmetic of every sample is the same whatever the host block size.

use alloc::vec::Vec;

use crate::band::{Band, Interp};
use crate::gain_match::{SHARE, Scheduler};
use crate::math;
use crate::smooth::{Smoother, time_coef};
use crate::{BANDS, BandParams, BandType, OUTPUT_MAX_DB, Placement, Slope, rate_class, sane_rate};

/// Samples per control tick (decision 3.1).
pub(crate) const TICK: usize = 16;
/// The parameter smoothing time (decision 3.1).
const SMOOTH_MS: f64 = 20.0;
/// The wet ramp time of discrete changes (decision 3.2).
const RAMP_MS: f64 = 10.0;
/// The smoothing of the applied gain match (decision 7).
const MATCH_MS: f64 = 50.0;

/// The Parametric EQ processor. Band `i` is the panel's band `i + 1`.
#[derive(Clone, Debug)]
pub struct ParametricEq {
    fs: f64,
    channels: usize,
    prepared: bool,
    pub(crate) bands: [Band; BANDS],
    pub(crate) clock: u64,
    pub(crate) stage2: bool,
    ramp_len: u32,
    output_db: f64,
    pub(crate) output: Smoother,
    match_on: bool,
    pub(crate) match_gain: Smoother,
    pub(crate) gm: Scheduler,
    work: Vec<[f64; TICK]>,
    interp: alloc::boxed::Box<Interp>,
    /// Test seam: a build with the mid/side stage removed (Evidence "Mid/side exactness").
    #[cfg(test)]
    pub(crate) no_stage2: bool,
}

/// The mid/side encode (decision 4): `M = L + R`, `S = L − R`, exact in f64 for f32 inputs within a
/// factor of 2^29 of each other or with either zero.
#[inline]
pub(crate) fn encode_ms(l: f64, r: f64) -> (f64, f64) {
    (l + r, l - r)
}

/// The mid/side decode: `L = (M + S)·0.5`, `R = (M − S)·0.5`.
#[inline]
pub(crate) fn decode_ms(m: f64, s: f64) -> (f64, f64) {
    ((m + s) * 0.5, (m - s) * 0.5)
}

impl Default for ParametricEq {
    fn default() -> Self {
        Self::new()
    }
}

impl ParametricEq {
    /// A processor with every band unused. Allocates the gain-match tables; call
    /// [`prepare`](Self::prepare) before processing.
    pub fn new() -> Self {
        Self {
            fs: 48_000.0,
            channels: 0,
            prepared: false,
            bands: core::array::from_fn(|_| Band::default()),
            clock: 0,
            stage2: false,
            ramp_len: 1,
            output_db: 0.0,
            output: Smoother::new(1.0),
            match_on: false,
            match_gain: Smoother::new(1.0),
            gm: Scheduler::new(),
            work: Vec::new(),
            interp: alloc::boxed::Box::default(),
            #[cfg(test)]
            no_stage2: false,
        }
    }

    /// Allocates for `channels` channels at `sample_rate` (1 kHz to 1.536 MHz), computes the gain match
    /// in full and resets. Not real-time safe.
    pub fn prepare(&mut self, sample_rate: f64, channels: usize) {
        self.fs = sane_rate(sample_rate);
        self.channels = channels;
        self.work = alloc::vec![[0.0; TICK]; channels];
        let smooth = time_coef(SMOOTH_MS, self.fs);
        for b in self.bands.iter_mut() {
            b.allocate(channels);
            b.set_coefs(smooth);
        }
        self.output.coef = smooth;
        self.match_gain.coef = time_coef(MATCH_MS, self.fs);
        // round(10 ms) in samples, at least one.
        self.ramp_len = ((RAMP_MS * self.fs / 1000.0 + 0.5) as u32).max(1);
        let bands = &self.bands;
        self.gm.compute_all(|b| bands[b].target, self.fs, channels);
        self.prepared = true;
        self.reset();
    }

    /// Back to the sample clock 0: section states cleared, smoothers snapped to their targets, ramps at
    /// their ends, designs recomputed. The gain-match tables are kept. Real-time safe.
    pub fn reset(&mut self) {
        self.clock = 0;
        for b in self.bands.iter_mut() {
            b.restart(self.fs, self.channels, self.ramp_len);
        }
        self.output.snap();
        self.update_match_target();
        self.match_gain.snap();
        self.update_stage2();
    }

    /// The sample rate in use.
    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    /// The prepared channel count.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Latency in samples: zero in Zero latency mode, the only mode of this core so far.
    pub fn latency(&self) -> usize {
        0
    }

    /// Band `i`'s stored values.
    pub fn band(&self, i: usize) -> BandParams {
        self.bands.get(i).map_or_else(BandParams::default, |b| b.target)
    }

    /// Every band's stored values.
    pub fn bands(&self) -> [BandParams; BANDS] {
        core::array::from_fn(|i| self.bands[i].target)
    }

    /// Sets band `i` whole (each field as its own setter would).
    pub fn set_band(&mut self, i: usize, p: BandParams) {
        self.set_used(i, p.used);
        self.set_on(i, p.on);
        self.set_kind(i, p.kind);
        self.set_freq(i, p.freq_hz);
        self.set_gain(i, p.gain_db);
        self.set_q(i, p.q);
        self.set_slope(i, p.slope);
        self.set_place(i, p.place);
    }

    fn changed(&mut self, i: usize, before: BandParams) {
        if self.bands[i].target != before {
            self.gm.mark_stale(i);
        }
    }

    /// Band `i` Used (a 10 ms ramp, decision 3.2).
    pub fn set_used(&mut self, i: usize, used: bool) {
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.target.used = used;
            self.changed(i, before);
        }
    }

    /// Band `i` On (a 10 ms ramp).
    pub fn set_on(&mut self, i: usize, on: bool) {
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.target.on = on;
            self.changed(i, before);
        }
    }

    /// Band `i` Type (crossfaded through `w = 0`).
    pub fn set_kind(&mut self, i: usize, kind: BandType) {
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.target.kind = kind;
            self.changed(i, before);
        }
    }

    /// Band `i` Slope (crossfaded; used by the cuts only).
    pub fn set_slope(&mut self, i: usize, slope: Slope) {
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.target.slope = slope;
            self.changed(i, before);
        }
    }

    /// Band `i` Placement (crossfaded).
    pub fn set_place(&mut self, i: usize, place: Placement) {
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.target.place = place;
            self.changed(i, before);
        }
    }

    /// Band `i` Frequency in hertz (20 ms in the log2 domain).
    pub fn set_freq(&mut self, i: usize, hz: f64) {
        let clock = self.clock;
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.set_freq(hz, clock);
            self.changed(i, before);
        }
    }

    /// Band `i` Gain in dB (20 ms in dB).
    pub fn set_gain(&mut self, i: usize, db: f64) {
        let clock = self.clock;
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.set_gain(db, clock);
            self.changed(i, before);
        }
    }

    /// Band `i` Q (20 ms in the log2 domain).
    pub fn set_q(&mut self, i: usize, q: f64) {
        let clock = self.clock;
        if let Some(b) = self.bands.get_mut(i) {
            let before = b.target;
            b.set_q(q, clock);
            self.changed(i, before);
        }
    }

    /// Output in dB, −24 to +24 (20 ms as a linear gain).
    pub fn set_output_db(&mut self, db: f64) {
        self.output_db = if db.is_finite() { db.clamp(-OUTPUT_MAX_DB, OUTPUT_MAX_DB) } else { 0.0 };
        self.output.target = math::db_to_gain(self.output_db);
    }

    /// Gain match on or off (the applied gain smoothed over 50 ms).
    pub fn set_gain_match(&mut self, on: bool) {
        self.match_on = on;
        self.update_match_target();
    }

    /// The computed gain match in dB (clamped to ±12 dB), whether or not it is applied.
    pub fn gain_match_db(&self) -> f64 {
        self.gm.match_db
    }

    fn update_match_target(&mut self) {
        self.match_gain.target = if self.match_on { math::db_to_gain(self.gm.match_db) } else { 1.0 };
    }

    fn update_stage2(&mut self) {
        self.stage2 = self.channels >= 2 && self.bands.iter().any(|b| b.processed && b.place.is_mid_side());
        #[cfg(test)]
        if self.no_stage2 {
            self.stage2 = false;
        }
    }

    fn control_tick(&mut self) {
        let (clock, fs, channels) = (self.clock, self.fs, self.channels);
        for b in self.bands.iter_mut() {
            b.tick(clock, fs, channels);
        }
        self.update_stage2();
        let share = SHARE / rate_class(fs);
        let bands = &self.bands;
        if self.gm.tick(|b| bands[b].target, fs, channels, share) {
            self.update_match_target();
        }
    }

    /// Processes `frames` samples of each channel in place (at most the prepared channel count).
    /// Non-finite input samples are read as 0. Real-time safe.
    pub fn process<C: AsMut<[f32]>>(&mut self, io: &mut [C], frames: usize) {
        if !self.prepared {
            return;
        }
        let nch = io.len().min(self.channels);
        let frames = io[..nch].iter_mut().fold(frames, |n, c| n.min(c.as_mut().len()));
        let mut done = 0;
        while done < frames {
            let phase = (self.clock % TICK as u64) as usize;
            if phase == 0 {
                self.control_tick();
            }
            let len = (TICK - phase).min(frames - done);
            self.run_chunk(io, nch, done, len, phase);
            self.clock += len as u64;
            done += len;
        }
    }

    fn run_chunk<C: AsMut<[f32]>>(&mut self, io: &mut [C], nch: usize, off: usize, len: usize, phase: usize) {
        for (buf, ch) in self.work.iter_mut().zip(io.iter_mut()) {
            for (w, &x) in buf[..len].iter_mut().zip(&ch.as_mut()[off..off + len]) {
                let x = f64::from(x);
                *w = if x.is_finite() { x } else { 0.0 };
            }
        }
        let mut wet = [0.0; TICK];
        let ramp_len = self.ramp_len;
        let stereo = self.channels >= 2 && nch >= 2;
        // Stage 1: Stereo, Left and Right bands in the left/right domain.
        for b in self.bands.iter_mut().filter(|b| !b.place.is_mid_side()) {
            if !b.processed {
                b.skip(len, ramp_len);
                continue;
            }
            b.wet(&mut wet[..len], ramp_len);
            b.fill(phase, len, &mut self.interp);
            match b.place {
                Placement::Left if stereo => b.run(0, &mut self.work[0][..len], &wet[..len], &self.interp),
                Placement::Right if stereo => b.run(1, &mut self.work[1][..len], &wet[..len], &self.interp),
                Placement::Stereo => {
                    for (ch, buf) in self.work.iter_mut().enumerate().take(nch) {
                        b.run(ch, &mut buf[..len], &wet[..len], &self.interp);
                    }
                }
                _ => {}
            }
        }
        // Stage 2: Mid and Side bands, between an exact f64 encode and decode while any runs.
        let encode = self.stage2 && stereo;
        if encode {
            let (l, r) = self.work.split_at_mut(1);
            for (a, b) in l[0][..len].iter_mut().zip(r[0][..len].iter_mut()) {
                (*a, *b) = encode_ms(*a, *b);
            }
        }
        for b in self.bands.iter_mut().filter(|b| b.place.is_mid_side()) {
            if !b.processed {
                b.skip(len, ramp_len);
                continue;
            }
            b.wet(&mut wet[..len], ramp_len);
            b.fill(phase, len, &mut self.interp);
            match b.place {
                // On mono a Mid band acts on the one channel (decision 4).
                Placement::Mid if encode || self.channels == 1 => b.run(0, &mut self.work[0][..len], &wet[..len], &self.interp),
                Placement::Side if encode => b.run(1, &mut self.work[1][..len], &wet[..len], &self.interp),
                _ => {}
            }
        }
        if encode {
            let (l, r) = self.work.split_at_mut(1);
            for (a, b) in l[0][..len].iter_mut().zip(r[0][..len].iter_mut()) {
                (*a, *b) = decode_ms(*a, *b);
            }
        }
        // Output and the applied gain match, then one conversion back.
        if !(self.output.settled() && self.match_gain.settled() && self.output.cur == 1.0 && self.match_gain.cur == 1.0) {
            let mut gain = [0.0; TICK];
            for g in gain[..len].iter_mut() {
                *g = self.output.step() * self.match_gain.step();
            }
            for buf in self.work.iter_mut().take(nch) {
                for (x, g) in buf[..len].iter_mut().zip(&gain[..len]) {
                    *x *= *g;
                }
            }
        }
        for (buf, ch) in self.work.iter().zip(io.iter_mut()) {
            for (o, &x) in ch.as_mut()[off..off + len].iter_mut().zip(&buf[..len]) {
                *o = x as f32;
            }
        }
    }

    /// Hashes the processor's whole run state (sections, smoothers, ramps, clock, gain match).
    #[cfg(any(test, feature = "testing"))]
    pub fn fingerprint<H: core::hash::Hasher>(&self, h: &mut H) {
        h.write_u64(self.clock);
        h.write_u8(self.stage2 as u8);
        for s in [&self.output, &self.match_gain] {
            h.write_u64(s.cur.to_bits());
            h.write_u64(s.target.to_bits());
        }
        for b in &self.bands {
            for s in [&b.log2_f, &b.log2_q, &b.gain] {
                h.write_u64(s.cur.to_bits());
                h.write_u64(s.target.to_bits());
            }
            h.write_u32(b.ramp.pos);
            h.write_u8(b.interp as u8);
            for s in &b.prev.sos[..b.prev.n_sos] {
                h.write_u64(s.a1.to_bits());
                h.write_u64(s.m1.to_bits());
            }
            h.write_i8(b.ramp.dir);
            h.write_u8(b.processed as u8);
            h.write_u8(b.kind as u8);
            for st in &b.states {
                for v in st.sos.iter().flatten() {
                    h.write_u64(v.to_bits());
                }
                h.write_u64(st.fos.to_bits());
            }
        }
        self.gm.fingerprint(h);
    }
}

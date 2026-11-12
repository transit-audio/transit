//! One band slot at run time (ADR 0054 decisions 3 and 4): its stored values, its smoothers (log2
//! frequency, log2 Q, gain in dB, each a 20 ms one-pole on the sample clock), its designed sections
//! with their per-channel 64-bit states, and its wet ramp.
//!
//! Smoothers advance lazily: a setter or a control tick first steps them over the samples since the
//! last call, with the target those samples had, so the values are those of a per-sample smoother
//! whatever the host block size. Every decision that changes what runs (designs, configuration
//! switches, ramp direction, identity) is taken at a control tick, on the sample clock.
//!
//! **Coefficient glide.** A band whose smoothers moved is redesigned every 16 samples (decision
//! 3.1), and over the following 16 samples its state-variable coefficients move linearly per sample
//! from the previous design to the new one (sample `n` of the interval at `(n + 1)/16`, the last
//! exactly the new design). Stepping the coefficients once per 16 samples steps the mixed outputs and
//! fails decision 3.3's click criterion for large Gain and Q moves; the glide meets it. The design
//! cost is unchanged; while a band moves, each of its second-order sections costs 24 more operations a
//! sample (eight coefficients, one subtraction, multiplication and addition each), computed once per
//! band and shared by its channels; a band at rest runs its plain design, bit for bit. Every
//! interpolated set is stable (the glide test checks the state matrix's spectral radius at every
//! point of every interval over the full ranges).

use alloc::vec::Vec;

use crate::chain::TICK;
use crate::design::{self, Coefficients, MAX_SOS, Used};
use crate::math;
use crate::section::{OnePole, Svf};
use crate::smooth::{Ramp, Smoother};
use crate::{BandParams, BandType, Placement, Slope, clamp_freq, clamp_gain, clamp_q};

/// The per-sample coefficients of one control interval while a band glides, filled once per chunk
/// and shared by every channel.
#[derive(Clone, Debug)]
pub(crate) struct Interp {
    pub(crate) sos: [[Svf; MAX_SOS]; TICK],
    pub(crate) fos: [OnePole; TICK],
}

impl Default for Interp {
    fn default() -> Self {
        Self { sos: [[Svf::default(); MAX_SOS]; TICK], fos: [OnePole::default(); TICK] }
    }
}

/// One channel's section states.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ChanState {
    pub(crate) sos: [[f64; 2]; MAX_SOS],
    pub(crate) fos: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Band {
    /// The stored values (continuous ones clamped), the targets of the smoothers.
    pub(crate) target: BandParams,
    pub(crate) log2_f: Smoother,
    pub(crate) log2_q: Smoother,
    pub(crate) gain: Smoother,
    /// The sample clock the smoothers have been advanced to.
    last: u64,
    /// The smoothers moved since the last design.
    dirty: bool,
    /// The configuration that is running (switched only at `w = 0`, decision 3.2).
    pub(crate) kind: BandType,
    pub(crate) slope: Slope,
    pub(crate) place: Placement,
    pub(crate) ramp: Ramp,
    /// The current design is the identity (skipped, decision 2.5).
    pub(crate) identity: bool,
    /// The band runs in the current control interval.
    pub(crate) processed: bool,
    pub(crate) coefs: Coefficients,
    /// The design the current control interval glides from, and whether it glides.
    pub(crate) prev: Coefficients,
    pub(crate) interp: bool,
    pub(crate) states: Vec<ChanState>,
    /// The states are known zero (no need to clear again).
    clean: bool,
    /// Clamped fits in the current design.
    pub(crate) clamped: u32,
}

impl Default for Band {
    fn default() -> Self {
        let t = BandParams::default();
        Self {
            target: t,
            log2_f: Smoother::new(math::log2(t.freq_hz)),
            log2_q: Smoother::new(math::log2(t.q)),
            gain: Smoother::new(t.gain_db),
            last: 0,
            dirty: false,
            kind: t.kind,
            slope: t.slope,
            place: t.place,
            ramp: Ramp::default(),
            identity: true,
            processed: false,
            coefs: Coefficients::default(),
            prev: Coefficients::default(),
            interp: false,
            states: Vec::new(),
            clean: true,
            clamped: 0,
        }
    }
}

/// Whether a placement has a signal to act on with `channels` channels (decision 4: on mono, Left,
/// Right and Side bands are inactive).
pub(crate) fn placement_active(place: Placement, channels: usize) -> bool {
    channels >= 2 || matches!(place, Placement::Stereo | Placement::Mid)
}

impl Band {
    /// Allocates the per-channel states (prepare only).
    pub(crate) fn allocate(&mut self, channels: usize) {
        self.states = alloc::vec![ChanState::default(); channels];
        self.clean = true;
    }

    pub(crate) fn set_coefs(&mut self, smooth: f64) {
        self.log2_f.coef = smooth;
        self.log2_q.coef = smooth;
        self.gain.coef = smooth;
    }

    /// Steps the smoothers over the samples before `clock`.
    pub(crate) fn catch_up(&mut self, clock: u64) {
        if clock > self.last {
            let n = clock - self.last;
            let moved = self.log2_f.advance(n) | self.log2_q.advance(n) | self.gain.advance(n);
            self.dirty |= moved;
        }
        self.last = clock;
    }

    pub(crate) fn set_freq(&mut self, hz: f64, clock: u64) {
        self.catch_up(clock);
        self.target.freq_hz = clamp_freq(hz);
        self.log2_f.target = math::log2(self.target.freq_hz);
    }

    pub(crate) fn set_gain(&mut self, db: f64, clock: u64) {
        self.catch_up(clock);
        self.target.gain_db = clamp_gain(db);
        self.gain.target = self.target.gain_db;
    }

    pub(crate) fn set_q(&mut self, q: f64, clock: u64) {
        self.catch_up(clock);
        self.target.q = clamp_q(q);
        self.log2_q.target = math::log2(self.target.q);
    }

    /// Restarts at the sample clock 0 (reset): smoothers snapped, the configuration switched to the
    /// stored one, the ramp at its end (`w` = 1 when used and on, else 0), states cleared, designed.
    pub(crate) fn restart(&mut self, fs: f64, channels: usize, ramp_len: u32) {
        self.log2_f.snap();
        self.log2_q.snap();
        self.gain.snap();
        self.last = 0;
        self.kind = self.target.kind;
        self.slope = self.target.slope;
        self.place = self.target.place;
        self.ramp = if self.target.audible() { Ramp { pos: ramp_len, dir: 0 } } else { Ramp::default() };
        self.clear();
        self.redesign(fs, false);
        self.update_processed(channels);
    }

    fn clear(&mut self) {
        if !self.clean {
            for s in self.states.iter_mut() {
                *s = ChanState::default();
            }
            self.clean = true;
        }
    }

    /// Designs from the smoothed values. With `glide`, the next control interval moves the
    /// coefficients linearly per sample from the previous design to this one (when both have the
    /// same sections and neither is the identity); otherwise the new design applies at once.
    fn redesign(&mut self, fs: f64, glide: bool) {
        let used = Used::from_smoothed(self.log2_f.cur, self.gain.cur, self.log2_q.cur, fs);
        let d = design::design(self.kind, self.slope, used, fs);
        let old = self.coefs;
        let was_identity = self.identity;
        self.identity = d.is_identity();
        self.coefs = d.coefficients();
        self.interp = glide && !was_identity && !self.identity && old.n_sos == self.coefs.n_sos && old.fos.is_some() == self.coefs.fos.is_some();
        self.prev = old;
        self.clamped = d.clamped;
        self.dirty = false;
        if self.identity {
            self.clear();
        }
    }

    fn update_processed(&mut self, channels: usize) {
        self.processed = !self.identity && placement_active(self.place, channels) && !self.ramp.silent();
    }

    /// The control tick at `clock` (every 16 samples): advances the smoothers, decides the ramp and
    /// any configuration switch, and redesigns a band whose smoothers moved.
    pub(crate) fn tick(&mut self, clock: u64, fs: f64, channels: usize) {
        self.catch_up(clock);
        self.interp = false;
        let t = self.target;
        let wanted = t.audible();
        let same = self.kind == t.kind && self.place == t.place && (!t.kind.uses_slope() || self.slope == t.slope);
        if self.ramp.silent() {
            if wanted {
                // Switch in at w = 0: the stored configuration, cleared states, the smoothers at their
                // targets (the ramp makes the entry click-free), then ramp in.
                self.kind = t.kind;
                self.slope = t.slope;
                self.place = t.place;
                self.log2_f.snap();
                self.log2_q.snap();
                self.gain.snap();
                self.clear();
                self.redesign(fs, false);
                self.ramp.dir = 1;
            } else {
                self.ramp.dir = 0;
                self.clear();
            }
        } else if !wanted || !same {
            self.ramp.dir = -1;
        } else {
            self.ramp.dir = 1;
        }
        if self.dirty {
            self.redesign(fs, true);
        }
        self.update_processed(channels);
    }

    /// Fills `w` with this chunk's wet amounts and advances the ramp.
    #[inline]
    pub(crate) fn wet(&mut self, w: &mut [f64], len: u32) {
        for v in w.iter_mut() {
            *v = self.ramp.wet(len);
            self.ramp.step(len);
        }
    }

    /// Advances the ramp without computing `w` (a band that does not run this chunk).
    #[inline]
    pub(crate) fn skip(&mut self, n: usize, len: u32) {
        if self.ramp.moving(len) {
            for _ in 0..n {
                self.ramp.step(len);
            }
        }
    }

    /// Fills `s` with this chunk's per-sample coefficients while the band glides: sample `n` of the
    /// control interval (`n = phase + i`) is `(n + 1)/16` of the way, the last exactly the new design.
    pub(crate) fn fill(&self, phase: usize, len: usize, s: &mut Interp) {
        if !self.interp {
            return;
        }
        let (a, b) = (&self.prev, &self.coefs);
        for i in 0..len {
            let n = phase + i;
            if n + 1 >= TICK {
                s.sos[i] = b.sos;
                s.fos[i] = b.fos.unwrap_or_default();
                continue;
            }
            let t = (n + 1) as f64 / TICK as f64;
            for j in 0..b.n_sos {
                s.sos[i][j] = Svf::lerp(&a.sos[j], &b.sos[j], t);
            }
            if let (Some(fa), Some(fb)) = (&a.fos, &b.fos) {
                s.fos[i] = OnePole::lerp(fa, fb, t);
            }
        }
    }

    /// Runs the band over one channel's buffer: `x + w·(band(x) − x)`, exactly `band(x)` at `w = 1`
    /// and exactly `x` at `w = 0`. While the band glides, sample `i` uses `s`'s coefficients.
    #[inline]
    pub(crate) fn run(&mut self, ch: usize, buf: &mut [f64], w: &[f64], s: &Interp) {
        self.clean = false;
        let interp = self.interp;
        let co = &self.coefs;
        let Some(st) = self.states.get_mut(ch) else { return };
        let n_sos = co.n_sos;
        for (i, (x, &wi)) in buf.iter_mut().zip(w).enumerate() {
            let input = *x;
            let mut y = input;
            let (sos, fos) = if interp { (&s.sos[i][..n_sos], co.fos.map(|_| s.fos[i])) } else { (&co.sos[..n_sos], co.fos) };
            for (sec, state) in sos.iter().zip(st.sos.iter_mut()) {
                y = sec.tick(state, y);
            }
            if let Some(f) = &fos {
                y = f.tick(&mut st.fos, y);
            }
            *x = if wi == 1.0 {
                y
            } else if wi == 0.0 {
                input
            } else {
                input + wi * (y - input)
            };
        }
    }
}

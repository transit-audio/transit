//! The main bus monitoring set (#366): Mono, Dim and Ref with the per-machine monitor level, on
//! the monitor path only, and the live integrated loudness of the mix.
//!
//! **Monitor-only.** The stage acts on Main's channels in the device callback after the plan
//! rendered the block, after the real-time render's tap took its copy and after the browser
//! preview joined (`player/audio.rs`, `render_next`): what reaches the device, never a bounce, a
//! freeze, a stem, a send or the meters. The offline bounce never runs this code at all.
//!
//! **Controls.** [`MonitorControls`] is a set of plain atomics in the Player's shared state: Main
//! stores the four flags and the two levels (each a `f32` as bits), the audio thread reads them
//! once per render block. Neither side allocates, locks or waits. The monitor gain is the monitor
//! level, or the stored reference level while Ref is on, times [`DIM_DB`] while Dim is on. Mono
//! sums the two sides at equal power: each side reaches both outputs at the matrix's −3 dB mono
//! pan law (`transit_dsp::pan::gains(0.0, PanLaw::Minus3)`).
//!
//! **Ramp.** A change moves linearly across one render block, from the block's first sample to its
//! last, the endpoints a track fader's change uses (`transit_mix`, `strip.rs`); Mono crossfades the
//! same way. With every control at rest at unity the stage touches nothing, so the output is
//! bit-identical to a build without it.
//!
//! **Loudness.** [`MonitorStage::measure`] feeds Main's post-fader mix (before the preview and the
//! monitor gain) to a [`LiveLoudness`] held in fixed-size state, and publishes the integrated
//! figure as `f32` bits (NaN until a block passes the absolute gate). Main resets it by bumping a
//! counter; the audio thread also resets it when the transport starts from a stop.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use transit_dsp::loudness::LiveLoudness;
use transit_dsp::pan::{PanLaw, gains};

/// Dim: a fixed cut on the monitor level while it is on.
pub const DIM_DB: f32 = -20.0;
/// The monitor level's floor: at or below it the level is silence (−∞).
pub const LEVEL_FLOOR_DB: f32 = transit_dsp::MIN_DB;

/// A level in dB as a linear gain: −∞ to 0 dB; at or below [`LEVEL_FLOOR_DB`] silence, NaN unity.
#[must_use]
pub fn level_gain(db: f32) -> f32 {
    if db.is_nan() { 1.0 } else { transit_dsp::db_to_gain(db.min(0.0)) }
}

/// The monitoring set as Main keeps it (`ui-studio`'s per-machine preference): what
/// [`MonitorControls::store`] hands the audio thread every UI frame, and what a device opens with
/// (`Player::with_config_and_monitor`), so a reopened device's first block already plays at the
/// saved Dim, Ref and level instead of unity until the next frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorSettings {
    pub mono: bool,
    pub dim: bool,
    pub reference: bool,
    /// The monitor level and the level Ref recalls, in dB (−∞ to 0; see [`level_gain`]).
    pub level_db: f32,
    pub reference_db: f32,
}

impl Default for MonitorSettings {
    /// Unity: every control off, both levels 0 dB.
    fn default() -> Self {
        Self { mono: false, dim: false, reference: false, level_db: 0.0, reference_db: 0.0 }
    }
}

/// The monitor controls Main sets and the published loudness it reads.
#[derive(Debug)]
pub struct MonitorControls {
    mono: AtomicBool,
    dim: AtomicBool,
    reference: AtomicBool,
    /// Linear gains, as `f32` bits.
    level: AtomicU32,
    reference_level: AtomicU32,
    /// Integrated LUFS as `f32` bits; NaN before the first gated block.
    lufs: AtomicU32,
    /// Bumped by Main to reset the loudness measurement.
    reset: AtomicU32,
}

impl Default for MonitorControls {
    fn default() -> Self {
        Self {
            mono: AtomicBool::new(false),
            dim: AtomicBool::new(false),
            reference: AtomicBool::new(false),
            level: AtomicU32::new(1.0f32.to_bits()),
            reference_level: AtomicU32::new(1.0f32.to_bits()),
            lufs: AtomicU32::new(f32::NAN.to_bits()),
            reset: AtomicU32::new(0),
        }
    }
}

impl MonitorControls {
    /// Stores the whole monitoring set: atomic stores only, safe every frame. The UI frame and
    /// the device open both come through here.
    pub fn store(&self, settings: &MonitorSettings) {
        self.set_mono(settings.mono);
        self.set_dim(settings.dim);
        self.set_reference(settings.reference);
        self.set_level_db(settings.level_db);
        self.set_reference_db(settings.reference_db);
    }
    pub fn set_mono(&self, on: bool) {
        self.mono.store(on, Ordering::Relaxed);
    }
    pub fn set_dim(&self, on: bool) {
        self.dim.store(on, Ordering::Relaxed);
    }
    pub fn set_reference(&self, on: bool) {
        self.reference.store(on, Ordering::Relaxed);
    }
    /// The monitor level in dB (−∞ to 0; see [`level_gain`]).
    pub fn set_level_db(&self, db: f32) {
        self.level.store(level_gain(db).to_bits(), Ordering::Relaxed);
    }
    /// The level Ref recalls, in dB (−∞ to 0).
    pub fn set_reference_db(&self, db: f32) {
        self.reference_level.store(level_gain(db).to_bits(), Ordering::Relaxed);
    }
    /// The integrated loudness of the mix since the last reset, in LUFS; `None` before the first
    /// gated block.
    #[must_use]
    pub fn integrated_lufs(&self) -> Option<f32> {
        let v = f32::from_bits(self.lufs.load(Ordering::Relaxed));
        v.is_finite().then_some(v)
    }
    /// Starts the loudness measurement over at the next render block (one atomic add).
    pub fn reset_loudness(&self) {
        self.reset.fetch_add(1, Ordering::Relaxed);
        self.lufs.store(f32::NAN.to_bits(), Ordering::Relaxed);
    }
    /// The gain the monitor path is heading for.
    #[must_use]
    pub fn target_gain(&self) -> f32 {
        let level = if self.reference.load(Ordering::Relaxed) { &self.reference_level } else { &self.level };
        let g = f32::from_bits(level.load(Ordering::Relaxed));
        let g = if g.is_finite() { g.clamp(0.0, 1.0) } else { 1.0 };
        if self.dim.load(Ordering::Relaxed) { g * transit_dsp::db_to_gain(DIM_DB) } else { g }
    }
    fn target_mono(&self) -> f32 {
        if self.mono.load(Ordering::Relaxed) { 1.0 } else { 0.0 }
    }
}

/// The audio thread's side: where the ramps are and the loudness accumulator. Built on Main (in a
/// box, with the callback state); nothing here allocates afterwards.
pub(crate) struct MonitorStage {
    gain: f32,
    mono: f32,
    /// Each side's share of the mono sum: the −3 dB center of the mono pan law.
    side: f32,
    loudness: LiveLoudness,
    seen_reset: u32,
    running: bool,
}

impl MonitorStage {
    /// At rest where the controls point, so a device that opens with Dim on starts dimmed.
    pub(crate) fn new(sample_rate: u32, controls: &MonitorControls) -> Box<Self> {
        Box::new(Self {
            gain: controls.target_gain(),
            mono: controls.target_mono(),
            side: gains(0.0, PanLaw::Minus3).0,
            loudness: LiveLoudness::new(sample_rate as f32, 2),
            seen_reset: controls.reset.load(Ordering::Relaxed),
            running: false,
        })
    }

    /// Feeds the block of Main's `channels` the plan just rendered to the loudness meter, after a
    /// reset Main asked for or a transport start from a stop (`running`: playing or counting in),
    /// and publishes the integrated figure.
    pub(crate) fn measure(&mut self, controls: &MonitorControls, main: &[Vec<f32>], channels: usize, frames: usize, running: bool) {
        let asked = controls.reset.load(Ordering::Relaxed);
        let started = running && !self.running;
        self.running = running;
        self.loudness.set_channels(channels);
        if asked != self.seen_reset || started {
            self.seen_reset = asked;
            self.loudness.reset();
        }
        self.loudness.process(main.get(..channels.min(main.len())).unwrap_or(&[]), frames);
        controls.lufs.store(self.loudness.integrated_lufs().unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
    }

    /// Applies Mono, Dim, Ref and the monitor level to Main's first `channels` buffers, ramping
    /// any change across the block's `frames`. Mono acts on a stereo Main only.
    pub(crate) fn apply(&mut self, controls: &MonitorControls, main: &mut [Vec<f32>], channels: usize, frames: usize) {
        let (g0, g1) = (self.gain, controls.target_gain());
        let stereo = channels == 2;
        let (m0, m1) = (self.mono, if stereo { controls.target_mono() } else { 0.0 });
        self.gain = g1;
        self.mono = m1;
        if g0 == 1.0 && g1 == 1.0 && m0 == 0.0 && m1 == 0.0 {
            return;
        }
        let step = if frames > 1 { 1.0 / (frames - 1) as f32 } else { 1.0 };
        let ramp = |a: f32, b: f32, i: usize| if frames > 1 { a + (b - a) * (i as f32 * step) } else { b };
        if stereo && (m0 > 0.0 || m1 > 0.0) {
            if let [left, right, ..] = main {
                mono_block(left, right, frames, &ramp, (g0, g1), (m0, m1), self.side);
            }
            return;
        }
        for buffer in main.iter_mut().take(channels) {
            for (i, v) in buffer.iter_mut().take(frames).enumerate() {
                *v *= ramp(g0, g1, i);
            }
        }
    }
}

/// The stereo pair through the mono crossfade and the gain ramp.
fn mono_block(
    left: &mut [f32],
    right: &mut [f32],
    frames: usize,
    ramp: &dyn Fn(f32, f32, usize) -> f32,
    (g0, g1): (f32, f32),
    (m0, m1): (f32, f32),
    side: f32,
) {
    for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).take(frames).enumerate() {
        let (g, m) = (ramp(g0, g1, i), ramp(m0, m1, i));
        let sum = side * (*l + *r);
        *l = g * ((1.0 - m) * *l + m * sum);
        *r = g * ((1.0 - m) * *r + m * sum);
    }
}

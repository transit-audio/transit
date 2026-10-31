//! The random sequencer oracle (#202): the firmware's t generator (gates) and X and Y generators
//! (voltages through the quantizer), with deja vu and the scale recorder, rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawSequencer`] and [`crate::ffi::RawRecorder`]. One
//! [`SequencerOracle`] is the module's generators with their own random generator, driven as the
//! firmware's main loop drives them: at 32 kHz ([`SEQUENCER_SAMPLE_RATE`]) in its blocks of 5 frames
//! ([`SEQUENCER_BLOCK_SIZE`]), the t generator first, then the X and Y generators, with the knob
//! values the CV reader would hand them ([`Controls`]). Each frame yields three gates (t1, the
//! master clock t2, t3) and four voltages (X1, X2, X3, Y).

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CSequencerControls, RawRecorder, RawSequencer, Session};

/// The module's block, frames (`kBlockSize` in `marbles/io_buffer.h`).
pub const SEQUENCER_BLOCK_SIZE: usize = 5;

/// The module's sample rate, Hz (`kSampleRate` in `marbles/marbles.cc`).
pub const SEQUENCER_SAMPLE_RATE: f32 = 32_000.0;

/// The number of scale slots (`kNumScales`).
pub const SCALE_SLOTS: usize = 6;

/// Gate flags of a clock input, one byte per frame (`stmlib/utils/gate_flags.h`).
pub mod gate {
    /// The input is high.
    pub const HIGH: u8 = 1;
    /// The input rose at this frame.
    pub const RISING: u8 = 2;
    /// The input fell at this frame.
    pub const FALLING: u8 = 4;

    /// `ExtractGateFlags(previous, current)`.
    #[must_use]
    pub fn extract(previous: u8, current: bool) -> u8 {
        let previous = previous & HIGH;
        match (current, previous != 0) {
            (true, true) => HIGH,
            (true, false) => RISING | HIGH,
            (false, true) => FALLING,
            (false, false) => 0,
        }
    }
}

/// What the firmware's main loop reads each block: the knobs (after the CV reader), the settings'
/// state bytes, the explicit reset gates and the clock routing (`OracleSequencerControls`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    /// The t generator's model (`TGeneratorModel`): 0 complementary Bernoulli, 1 clusters, 2 drums,
    /// 3 independent Bernoulli, 4 divider, 5 three states, 6 Markov (the module offers 0 to 5).
    pub t_model: i32,
    /// The rate range (`TGeneratorRange`): 0 a quarter, 1 normal, 2 four times.
    pub t_range: i32,
    /// The rate knob, semitones around 2 Hz (-60 to 60 on the module); with the t clock patched it
    /// picks the clock's division instead.
    pub t_rate: f32,
    /// The t bias knob, 0 to 1.
    pub t_bias: f32,
    /// The jitter knob, 0 to 1.
    pub t_jitter: f32,
    /// The deja vu knob, 0 to 1 (the main loop's dead band around 12 o'clock applies).
    pub deja_vu: f32,
    /// The loop length knob, 0 to 1, stepped through the firmware's 12 lengths (1 to 16).
    pub deja_vu_length: f32,
    /// Deja vu on the t section (`DejaVuState`): 0 off, 1 on, 2 locked.
    pub t_deja_vu: i32,
    /// Deja vu on the X section: 0 off, 1 on, 2 locked.
    pub x_deja_vu: i32,
    /// The gate length (the state byte, 0 to 255, over 256).
    pub t_pulse_width_mean: i32,
    /// The gate length's randomness (the state byte, 0 to 255).
    pub t_pulse_width_std: i32,
    /// How the three X channels share spread, bias and steps (`ControlMode`): 0 identical, 1 bump,
    /// 2 tilt.
    pub x_control_mode: i32,
    /// The X voltage range (`VoltageRange`): 0 0 to 2 V, 1 0 to 5 V, 2 -5 to 5 V.
    pub x_range: i32,
    /// The X section samples its input (`register_mode`): 0 or 1.
    pub x_register_mode: i32,
    /// The input sample the X section takes in that mode, 0 to 1 (the note filter's output).
    pub x_register_value: f32,
    /// The spread knob, 0 to 1.
    pub x_spread: f32,
    /// The X bias knob, 0 to 1.
    pub x_bias: f32,
    /// The steps knob, 0 to 1 (below 0.5 smooth, above quantized ever more coarsely).
    pub x_steps: f32,
    /// The scale slot, 0 to 5.
    pub x_scale: i32,
    /// The Y channel's spread, bias and steps (state bytes, 0 to 255).
    pub y_spread: i32,
    /// See [`Controls::y_spread`].
    pub y_bias: i32,
    /// See [`Controls::y_spread`].
    pub y_steps: i32,
    /// The Y channel's clock division (the state byte, 0 to 255, picks one of 12 divisions).
    pub y_divider: i32,
    /// The Y voltage range (as [`Controls::x_range`]).
    pub y_range: i32,
    /// The t section's explicit reset fires this block (0 or 1).
    pub t_reset: i32,
    /// The X section's explicit reset fires this block (0 or 1).
    pub x_reset: i32,
    /// The t clock input is patched (0 or 1): the t generator follows it.
    pub t_external: i32,
    /// The X section's clock (`ClockSource`): 0 its own three (t1, t2, t3), 1 t1, 2 t2, 3 t3,
    /// 4 the X clock input.
    pub xy_clock_source: i32,
}

impl Default for Controls {
    /// The module's state after a factory reset (`Settings::Init`), the knobs at 12 o'clock and
    /// deja vu off.
    fn default() -> Self {
        Self {
            t_model: 0,
            t_range: 1,
            t_rate: 0.0,
            t_bias: 0.5,
            t_jitter: 0.0,
            deja_vu: 0.0,
            deja_vu_length: 0.5,
            t_deja_vu: 0,
            x_deja_vu: 0,
            t_pulse_width_mean: 128,
            t_pulse_width_std: 0,
            x_control_mode: 0,
            x_range: 2,
            x_register_mode: 0,
            x_register_value: 0.5,
            x_spread: 0.5,
            x_bias: 0.5,
            x_steps: 0.5,
            x_scale: 0,
            y_spread: 128,
            y_bias: 128,
            y_steps: 0,
            y_divider: 128,
            y_range: 2,
            t_reset: 0,
            x_reset: 0,
            t_external: 0,
            xy_clock_source: 0,
        }
    }
}

impl Controls {
    pub(crate) fn to_c(self) -> CSequencerControls {
        CSequencerControls {
            t_model: self.t_model,
            t_range: self.t_range,
            t_rate: self.t_rate,
            t_bias: self.t_bias,
            t_jitter: self.t_jitter,
            deja_vu: self.deja_vu,
            deja_vu_length: self.deja_vu_length,
            t_deja_vu: self.t_deja_vu,
            x_deja_vu: self.x_deja_vu,
            t_pulse_width_mean: self.t_pulse_width_mean,
            t_pulse_width_std: self.t_pulse_width_std,
            x_control_mode: self.x_control_mode,
            x_range: self.x_range,
            x_register_mode: self.x_register_mode,
            x_register_value: self.x_register_value,
            x_spread: self.x_spread,
            x_bias: self.x_bias,
            x_steps: self.x_steps,
            x_scale: self.x_scale,
            y_spread: self.y_spread,
            y_bias: self.y_bias,
            y_steps: self.y_steps,
            y_divider: self.y_divider,
            y_range: self.y_range,
            t_reset: self.t_reset,
            x_reset: self.x_reset,
            t_external: self.t_external,
            xy_clock_source: self.xy_clock_source,
        }
    }
}

/// Whether the C++ can take these controls (invariant 4 of the FFI; its wrapper checks the same):
/// every knob within 0 to 1 (the rate within -64 to 64 semitones: the pitch table it indexes covers
/// -128 to 127), every state byte within 0 to 255 and every enumeration within its values (they
/// index the firmware's tables and arrays).
#[must_use]
pub fn controls_in_range(c: &Controls) -> bool {
    let unit = |x: f32| (0.0..=1.0).contains(&x);
    let byte = |x: i32| (0..=255).contains(&x);
    (0..=6).contains(&c.t_model)
        && (0..=2).contains(&c.t_range)
        && (-64.0..=64.0).contains(&c.t_rate)
        && unit(c.t_bias)
        && unit(c.t_jitter)
        && unit(c.deja_vu)
        && unit(c.deja_vu_length)
        && (0..=2).contains(&c.t_deja_vu)
        && (0..=2).contains(&c.x_deja_vu)
        && byte(c.t_pulse_width_mean)
        && byte(c.t_pulse_width_std)
        && (0..=2).contains(&c.x_control_mode)
        && (0..=2).contains(&c.x_range)
        && (0..=1).contains(&c.x_register_mode)
        && unit(c.x_register_value)
        && unit(c.x_spread)
        && unit(c.x_bias)
        && unit(c.x_steps)
        && (0..=5).contains(&c.x_scale)
        && byte(c.y_spread)
        && byte(c.y_bias)
        && byte(c.y_steps)
        && byte(c.y_divider)
        && (0..=2).contains(&c.y_range)
        && (0..=1).contains(&c.t_reset)
        && (0..=1).contains(&c.x_reset)
        && (0..=1).contains(&c.t_external)
        && (0..=4).contains(&c.xy_clock_source)
}

/// A scale: the interval it repeats at (volts) and up to 16 degrees, each a voltage within one
/// interval and a weight (0 to 255; the quantizer keeps the heavier degrees as steps grow).
#[derive(Clone, Debug, PartialEq)]
pub struct Scale {
    /// The interval the degrees repeat at, volts (1 V: an octave).
    pub base_interval: f32,
    /// Voltage and weight per degree, ascending.
    pub degrees: Vec<(f32, u8)>,
}

impl Scale {
    fn from_c(base: f32, n: i32, v: &[f32; 16], w: &[u8; 16]) -> Self {
        let n = usize::try_from(n).unwrap_or(0).min(16);
        Self { base_interval: base, degrees: v.iter().zip(w).take(n).map(|(v, w)| (*v, *w)).collect() }
    }
}

impl Session {
    /// The wrapper's copy of the firmware's six preset scales, as [`Scale`]s.
    #[must_use]
    pub fn sequencer_preset_scales(&self) -> Vec<Scale> {
        (0..SCALE_SLOTS).filter_map(|i| self.sequencer_preset_scale(i)).map(|(b, n, v, w)| Scale::from_c(b, n, &v, &w)).collect()
    }

    /// New generators as the firmware's `Init` leaves them, the random generator seeded with `seed`
    /// (the module seeds it with 1) and the six preset scales loaded.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] when the allocation fails.
    pub fn sequencer(&self, seed: u32) -> Result<SequencerOracle<'_>, Error> {
        let raw = RawSequencer::new(self, seed).ok_or(Error::OutOfMemory)?;
        Ok(SequencerOracle { raw })
    }

    /// A new scale recorder (empty).
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] when the allocation fails.
    pub fn scale_recorder(&self) -> Result<RecorderOracle<'_>, Error> {
        let raw = RawRecorder::new(self).ok_or(Error::OutOfMemory)?;
        Ok(RecorderOracle { raw })
    }
}

/// The random sequencer's generators at 32 kHz.
pub struct SequencerOracle<'s> {
    raw: RawSequencer<'s>,
}

impl SequencerOracle<'_> {
    /// Whether the calls run inside the flush-to-zero scope (the default, as every oracle of the
    /// family): an evidence switch for the test that asks whether the flush moves a value.
    pub fn set_flush(&mut self, flush: bool) {
        self.raw.set_flush(flush);
    }

    /// Loads `scale` into slot `index` (0 to 5) of the three X channels.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] when the slot, the interval (above 0, at most 4 V) or a degree (1 to
    /// 16, ascending, within one interval, one of weight 255) is out of range.
    pub fn load_scale(&mut self, index: usize, scale: &Scale) -> Result<(), Error> {
        let v: Vec<f32> = scale.degrees.iter().map(|d| d.0).collect();
        let w: Vec<u8> = scale.degrees.iter().map(|d| d.1).collect();
        self.raw.load_scale(index, scale.base_interval, &v, &w).ok_or(Error::BadParameter)
    }

    /// One block of `gates.len()` frames (1 to [`SEQUENCER_BLOCK_SIZE`]): three gates per frame (t1,
    /// t2, t3; 0 or 1) and four voltages (X1, X2, X3, Y). `t_clock` and `xy_clock` hold the clock
    /// inputs' gate flags per frame ([`gate`]); each is needed when `controls` say the input is
    /// patched or used.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty, too long or the lengths differ;
    /// [`Error::BadParameter`] when a control is out of range ([`controls_in_range`]), a needed clock
    /// is missing or a flag byte is above 7.
    pub fn process(
        &mut self,
        controls: &Controls,
        t_clock: Option<&[u8]>,
        xy_clock: Option<&[u8]>,
        gates: &mut [[u8; 3]],
        voltages: &mut [[f32; 4]],
    ) -> Result<(), Error> {
        let n = gates.len();
        if n == 0
            || n > SEQUENCER_BLOCK_SIZE
            || voltages.len() != n
            || t_clock.is_some_and(|c| c.len() != n)
            || xy_clock.is_some_and(|c| c.len() != n)
        {
            return Err(Error::BadBlock);
        }
        if !controls_in_range(controls) {
            return Err(Error::BadParameter);
        }
        self.raw.process(&controls.to_c(), t_clock, xy_clock, gates, voltages).ok_or(Error::BadParameter)
    }
}

/// One event of the scale recorder, as the firmware feeds it from the note CV and the X clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecorderEvent {
    /// A rising gate: a new note at this voltage.
    NewNote(f32),
    /// The gate held: the voltage it reads.
    Update(f32),
    /// A falling gate: the note is accepted.
    Accept,
}

/// The firmware's scale recorder.
pub struct RecorderOracle<'s> {
    raw: RawRecorder<'s>,
}

impl RecorderOracle<'_> {
    /// Feeds one event.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] for a voltage outside -5 to 5 V (or not finite).
    pub fn event(&mut self, e: RecorderEvent) -> Result<(), Error> {
        let (kind, v) = match e {
            RecorderEvent::NewNote(v) => (0, v),
            RecorderEvent::Update(v) => (1, v),
            RecorderEvent::Accept => (2, 0.0),
        };
        self.raw.event(kind, v).ok_or(Error::BadParameter)
    }

    /// `ExtractScale`: the recorded scale (degrees sorted, weights from their counts), or `None`
    /// below two degrees.
    pub fn extract(&mut self) -> Option<Scale> {
        self.raw.extract().map(|(b, n, v, w)| Scale::from_c(b, n, &v, &w))
    }
}

//! The voltage generators: `marbles/random/x_y_generator.{h,cc}`. Three X channels (on the three
//! gate clocks, or one shared clock) and a Y channel on a division of the master clock, each with
//! its own deja vu sequence and output channel.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::output_channel::{OutputChannel, ScaleOffset};
use crate::quantizer::Scale;
use crate::ramp::{RampDivider, Ratio};
use crate::ramp_extractor::RampExtractor;
use crate::random::{RandomSequence, RandomStream};
use crate::t_generator::Ramps;

/// `kNumXChannels`, `kNumChannels` (the three X channels and Y).
pub const NUM_X_CHANNELS: usize = 3;
pub const NUM_CHANNELS: usize = 4;
/// The firmware's scale slots per channel (`kNumScales`).
pub const NUM_SCALES: usize = 6;

/// `VoltageRange`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VoltageRange {
    /// 0 to 2 V.
    Narrow,
    /// 0 to 5 V.
    Positive,
    /// -5 to 5 V.
    #[default]
    Full,
}

impl VoltageRange {
    /// The firmware's range by index (0 to 2; beyond, full).
    #[must_use]
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Self::Narrow,
            1 => Self::Positive,
            _ => Self::Full,
        }
    }

    /// The channel's scaling for this range.
    #[must_use]
    pub fn scale_offset(self) -> ScaleOffset {
        match self {
            Self::Narrow => ScaleOffset::new(2.0, 0.0),
            Self::Positive => ScaleOffset::new(5.0, 0.0),
            Self::Full => ScaleOffset::new(10.0, -5.0),
        }
    }
}

/// `ClockSource`: the X channels' clocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ClockSource {
    /// X1 on t1, X2 on t2, X3 on t3.
    #[default]
    InternalT1T2T3,
    InternalT1,
    InternalT2,
    InternalT3,
    /// The X clock input.
    External,
}

impl ClockSource {
    /// The firmware's source by index (0 to 4; beyond, external).
    #[must_use]
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Self::InternalT1T2T3,
            1 => Self::InternalT1,
            2 => Self::InternalT2,
            3 => Self::InternalT3,
            _ => Self::External,
        }
    }
}

/// `ControlMode`: how the three X channels share spread, bias and steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ControlMode {
    /// All three the same.
    #[default]
    Identical,
    /// X2 as set, X1 and X3 mirrored around the center.
    Bump,
    /// X1 mirrored, X2 at the center, X3 as set.
    Tilt,
}

impl ControlMode {
    /// The firmware's mode by index (0 to 2; beyond, tilt).
    #[must_use]
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Self::Identical,
            1 => Self::Bump,
            _ => Self::Tilt,
        }
    }

    /// The amount channel `i` follows the controls by (`Process`).
    #[must_use]
    pub fn amount(self, i: usize) -> F {
        match self {
            Self::Identical => F::ONE,
            Self::Bump => F(if i == NUM_X_CHANNELS / 2 { 1.0 } else { -1.0 }),
            Self::Tilt => 2.0 * F::from_usize(i) / F::from_usize(NUM_X_CHANNELS - 1) - 1.0,
        }
    }
}

/// `GroupSettings`: the controls of the X group or of Y.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroupSettings {
    pub control_mode: ControlMode,
    pub voltage_range: VoltageRange,
    pub register_mode: bool,
    pub register_value: F,
    pub spread: F,
    pub bias: F,
    pub steps: F,
    pub deja_vu: F,
    pub scale_index: usize,
    pub length: i32,
    pub ratio: Ratio,
}

const HASHES: [u32; NUM_X_CHANNELS] = [0, 0xbeca_55e5, 0xf0ca_cc1a];

/// Which ramp clocks a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Master,
    External,
    Slave(usize),
}

/// `XYGenerator`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XYGenerator {
    pub(crate) random_sequence: [RandomSequence; NUM_CHANNELS],
    pub(crate) output_channel: [OutputChannel<NUM_SCALES>; NUM_CHANNELS],
    ramp_extractor: RampExtractor,
    ramp_divider: RampDivider,
    external_clock_stabilization_counter: i32,
    use_shifted_sequences: [bool; NUM_CHANNELS],
}

impl XYGenerator {
    /// `Init(random_stream, sr)`.
    #[must_use]
    pub fn new(stream: &mut RandomStream, sample_rate: f32) -> Self {
        let random_sequence: [RandomSequence; NUM_CHANNELS] = std::array::from_fn(|_| RandomSequence::new(stream));
        let mut output_channel = [OutputChannel::<NUM_SCALES>::default(); NUM_CHANNELS];
        for c in &mut output_channel {
            c.init();
        }
        let mut ramp_extractor = RampExtractor::default();
        ramp_extractor.init(F(8000.0) / sample_rate);
        let mut ramp_divider = RampDivider::default();
        ramp_divider.init();
        Self {
            random_sequence,
            output_channel,
            ramp_extractor,
            ramp_divider,
            external_clock_stabilization_counter: 16,
            use_shifted_sequences: [false; NUM_CHANNELS],
        }
    }

    /// `LoadScale(scale_index, scale)`: into the three X channels.
    pub fn load_scale(&mut self, scale_index: usize, scale: &Scale) {
        for c in self.output_channel.iter_mut().take(NUM_X_CHANNELS) {
            c.load_scale(scale_index, scale);
        }
    }

    /// `Process(clock_source, x_settings, y_settings, &reset, external_clock, ramps, output, size)`:
    /// `output` holds four voltages per frame (X1, X2, X3, Y). The ramps are the t generator's; the
    /// external clock's recovered ramp is written over its first slave ramp, and Y's divided ramp
    /// over the external ramp, as in the firmware.
    #[allow(clippy::too_many_arguments)]
    pub fn process<const B: usize>(
        &mut self,
        stream: &mut RandomStream,
        clock_source: ClockSource,
        x_settings: &GroupSettings,
        y_settings: &GroupSettings,
        reset: &mut bool,
        external_clock: &[u8],
        ramps: &mut Ramps<B>,
        output: &mut [F],
        size: usize,
    ) {
        let size = size.min(B);
        if clock_source != ClockSource::External {
            // For a couple of blocks the normalization pin still sends garbage to ignore.
            self.external_clock_stabilization_counter = 16;
        } else if self.external_clock_stabilization_counter > 0 {
            self.external_clock_stabilization_counter -= 1;
            if self.external_clock_stabilization_counter == 0 {
                self.ramp_extractor.reset();
            }
        }
        let mut channel_ramp: [Source; NUM_CHANNELS] = match clock_source {
            ClockSource::External => {
                let n = size.min(external_clock.len());
                self.ramp_extractor.process(Ratio { p: 1, q: 1 }, false, reset, &external_clock[..n], &mut ramps.slave[0][..n]);
                if self.external_clock_stabilization_counter > 0 {
                    ramps.slave[0][..size].fill(F::ZERO);
                }
                [Source::Slave(0); NUM_CHANNELS]
            }
            ClockSource::InternalT1 => [Source::Slave(0); NUM_CHANNELS],
            ClockSource::InternalT2 => [Source::Master; NUM_CHANNELS],
            ClockSource::InternalT3 => [Source::Slave(1); NUM_CHANNELS],
            ClockSource::InternalT1T2T3 => [Source::Slave(0), Source::Master, Source::Slave(1), Source::Master],
        };
        if *reset {
            self.ramp_divider.reset();
        }
        let divided_input: [F; B] = match channel_ramp[1] {
            Source::Master => ramps.master,
            Source::External => ramps.external,
            Source::Slave(k) => ramps.slave[k],
        };
        self.ramp_divider.process(y_settings.ratio, &divided_input[..size], &mut ramps.external[..size]);
        channel_ramp[NUM_CHANNELS - 1] = Source::External;

        for i in 0..NUM_CHANNELS {
            let settings = if i < NUM_X_CHANNELS { x_settings } else { y_settings };
            let channel = &mut self.output_channel[i];
            channel.set_scale_offset(settings.voltage_range.scale_offset());
            let amount = settings.control_mode.amount(i);
            channel.set_spread(0.5 + (settings.spread - 0.5) * amount);
            channel.set_bias(0.5 + (settings.bias - 0.5) * amount);
            channel.set_steps(0.5 + (settings.steps - 0.5) * (if settings.register_mode { F::ONE } else { amount }));
            channel.set_scale_index(settings.scale_index);
            channel.set_register_mode(settings.register_mode);
            channel.set_register_value(settings.register_value);
            channel.set_register_transposition(4.0 * settings.spread * (settings.bias - 0.5) * amount);

            let own = &mut self.random_sequence[i];
            own.record();
            own.set_length(settings.length);
            own.set_deja_vu(settings.deja_vu);
            if *reset {
                own.reset();
            }

            // When all channels follow one clock, the deja vu loop is shared and the input-sampling
            // mode's value is shifted from channel to channel.
            let mut seq = i;
            let mut use_shifted = false;
            if clock_source != ClockSource::InternalT1T2T3 && i > 0 && i < NUM_X_CHANNELS {
                seq = 0;
                if settings.register_mode {
                    use_shifted = true;
                    let shift = match settings.control_mode {
                        ControlMode::Identical => i as u32,
                        ControlMode::Bump => u32::from(i == 2),
                        ControlMode::Tilt => 0,
                    };
                    self.random_sequence[0].replay_shifted(shift);
                } else {
                    self.random_sequence[0].replay_pseudo_random(HASHES[i]);
                }
            }
            if !use_shifted && self.use_shifted_sequences[i] && seq != 0 {
                self.random_sequence[seq] = self.random_sequence[0];
            }
            self.use_shifted_sequences[i] = use_shifted;

            let phase: [F; B] = match channel_ramp[i] {
                Source::Master => ramps.master,
                Source::External => ramps.external,
                Source::Slave(k) => ramps.slave[k],
            };
            let out = output.get_mut(i..).unwrap_or_default();
            self.output_channel[i].process(&mut self.random_sequence[seq], stream, &phase[..size], out, NUM_CHANNELS);
        }
    }
}

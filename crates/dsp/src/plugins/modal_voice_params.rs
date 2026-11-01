//! The Modal Voice's parameter table (#198 slice 2): the module's controls (the exciter's contour,
//! bow, blow and strike; the resonator; space), the instrument's own (velocity, level, tune, bend),
//! and the shaper and modulation matrix it shares with the Macro Oscillator (#243), under the same
//! ids. Sessions, presets and automation lanes store the ids: never rename one.

use super::{DESTINATIONS, MODELS};
use crate::params::{choice, lin, log, toggle};
use crate::plugins::lfo::DIVISIONS;
use crate::plugins::macro_osc::matrix::{CURVES, ROWS, SOURCES};
use crate::plugins::macro_osc::shaper::{FILTER_MODES, LFO_SHAPES};
use crate::{ParamInfo, Unit};

pub(super) const MODEL: usize = 0;
pub(super) const CONTOUR: usize = 1;
pub(super) const BOW_LEVEL: usize = 2;
pub(super) const BOW_TIMBRE: usize = 3;
pub(super) const BLOW_LEVEL: usize = 4;
pub(super) const FLOW: usize = 5;
pub(super) const BLOW_TIMBRE: usize = 6;
pub(super) const STRIKE_LEVEL: usize = 7;
pub(super) const MALLET: usize = 8;
pub(super) const STRIKE_TIMBRE: usize = 9;
pub(super) const GEOMETRY: usize = 10;
pub(super) const BRIGHTNESS: usize = 11;
pub(super) const DAMPING: usize = 12;
pub(super) const POSITION: usize = 13;
pub(super) const SPACE: usize = 14;
pub(super) const VELOCITY: usize = 15;
pub(super) const LEVEL: usize = 16;
pub(super) const TUNE: usize = 17;
pub(super) const BEND_RANGE: usize = 18;
pub(super) const SHAPER: usize = 19;
pub(super) const AMP_ATTACK: usize = 20;
pub(super) const AMP_DECAY: usize = 21;
pub(super) const AMP_SUSTAIN: usize = 22;
pub(super) const AMP_RELEASE: usize = 23;
pub(super) const AMP_VELOCITY: usize = 24;
pub(super) const FILTER_MODE: usize = 25;
pub(super) const FILTER_SLOPE: usize = 26;
pub(super) const CUTOFF: usize = 27;
pub(super) const RESONANCE: usize = 28;
pub(super) const DRIVE: usize = 29;
pub(super) const KEY_TRACK: usize = 30;
pub(super) const FILTER_ENV_AMOUNT: usize = 31;
pub(super) const FILTER_ATTACK: usize = 32;
pub(super) const FILTER_DECAY: usize = 33;
pub(super) const FILTER_SUSTAIN: usize = 34;
pub(super) const FILTER_RELEASE: usize = 35;
/// The first parameter of LFO 1 and of LFO 2; each has seven, in the order of the `LFO_*` offsets.
pub(super) const LFO1: usize = 36;
pub(super) const LFO2: usize = 43;
pub(super) const LFO_SHAPE: usize = 0;
pub(super) const LFO_RATE: usize = 1;
pub(super) const LFO_SYNC: usize = 2;
pub(super) const LFO_DIVISION: usize = 3;
pub(super) const LFO_RETRIGGER: usize = 4;
pub(super) const LFO_FADE: usize = 5;
pub(super) const LFO_AMOUNT: usize = 6;
/// The first matrix row's first parameter; each row has [`ROW_PARAMS`], in the order of the
/// `ROW_*` offsets (the Macro Oscillator's).
pub(super) const ROW1: usize = 50;
pub(super) const ROW_PARAMS: usize = 5;
pub(super) const ROW_SOURCE: usize = 0;
pub(super) const ROW_VIA: usize = 1;
pub(super) const ROW_DEST: usize = 2;
pub(super) const ROW_AMOUNT: usize = 3;
pub(super) const ROW_CURVE: usize = 4;

/// Parameters in all.
pub(super) const COUNT: usize = ROW1 + ROWS * ROW_PARAMS;

/// The table: the fixed parameters, then one LFO's seven per LFO, then five per matrix row (the
/// Macro Oscillator's ids and ranges, the Modal Voice's destinations).
macro_rules! table {
    ([$($fixed:expr),* $(,)?], [$($l:literal),*], [$($n:literal),*]) => {
        [
            $($fixed,)*
            $(
                choice(concat!("lfo", $l, "_shape"), concat!("LFO ", $l, " Shape"), &LFO_SHAPES, 0),
                log(concat!("lfo", $l, "_rate"), concat!("LFO ", $l, " Rate"), 0.05, 40.0, 2.0, Unit::Hz),
                toggle(concat!("lfo", $l, "_sync"), concat!("LFO ", $l, " Sync"), false),
                choice(concat!("lfo", $l, "_division"), concat!("LFO ", $l, " Division"), &DIVISIONS, 4),
                toggle(concat!("lfo", $l, "_retrigger"), concat!("LFO ", $l, " Retrigger"), true),
                lin(concat!("lfo", $l, "_fade"), concat!("LFO ", $l, " Fade In"), 0.0, 10_000.0, 0.0, Unit::Ms),
                lin(concat!("lfo", $l, "_amount"), concat!("LFO ", $l, " Amount"), 0.0, 100.0, 100.0, Unit::Percent),
            )*
            $(
                choice(concat!("mod", $n, "_source"), concat!("Mod ", $n, " Source"), &SOURCES, 0),
                choice(concat!("mod", $n, "_via"), concat!("Mod ", $n, " Via"), &SOURCES, 0),
                choice(concat!("mod", $n, "_dest"), concat!("Mod ", $n, " Destination"), &DESTINATIONS, 0),
                lin(concat!("mod", $n, "_amount"), concat!("Mod ", $n, " Amount"), -100.0, 100.0, 0.0, Unit::Percent),
                choice(concat!("mod", $n, "_curve"), concat!("Mod ", $n, " Curve"), &CURVES, 0),
            )*
        ]
    };
}

pub(super) static PARAMS: [ParamInfo; COUNT] = table!(
    [
        choice("model", "Model", &MODELS, 0),
        lin("contour", "Contour", 0.0, 100.0, 100.0, Unit::Percent),
        lin("bow_level", "Bow", 0.0, 100.0, 0.0, Unit::Percent),
        lin("bow_timbre", "Bow Timbre", 0.0, 100.0, 50.0, Unit::Percent),
        lin("blow_level", "Blow", 0.0, 100.0, 0.0, Unit::Percent),
        lin("flow", "Flow", 0.0, 100.0, 50.0, Unit::Percent),
        lin("blow_timbre", "Blow Timbre", 0.0, 100.0, 50.0, Unit::Percent),
        lin("strike_level", "Strike", 0.0, 100.0, 80.0, Unit::Percent),
        lin("mallet", "Mallet", 0.0, 100.0, 50.0, Unit::Percent),
        lin("strike_timbre", "Strike Timbre", 0.0, 100.0, 50.0, Unit::Percent),
        lin("geometry", "Geometry", 0.0, 100.0, 20.0, Unit::Percent),
        lin("brightness", "Brightness", 0.0, 100.0, 50.0, Unit::Percent),
        lin("damping", "Damping", 0.0, 100.0, 25.0, Unit::Percent),
        lin("position", "Position", 0.0, 100.0, 30.0, Unit::Percent),
        lin("space", "Space", 0.0, 200.0, 50.0, Unit::Percent),
        lin("velocity", "Velocity", 0.0, 100.0, 100.0, Unit::Percent),
        lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
        lin("tune", "Tune", -24.0, 24.0, 0.0, Unit::Semitones),
        lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
        toggle("shaper", "Shaper", false),
        log("amp_attack", "Amp Attack", 0.5, 10_000.0, 2.0, Unit::Ms),
        log("amp_decay", "Amp Decay", 1.0, 20_000.0, 300.0, Unit::Ms),
        lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 100.0, Unit::Percent),
        log("amp_release", "Amp Release", 1.0, 20_000.0, 300.0, Unit::Ms),
        lin("amp_velocity", "Amp Velocity", 0.0, 100.0, 50.0, Unit::Percent),
        choice("filter_mode", "Filter Mode", &FILTER_MODES, 0),
        choice("filter_slope", "Filter Slope", &["12 dB", "24 dB"], 0),
        log("cutoff", "Cutoff", 20.0, 20_000.0, 20_000.0, Unit::Hz),
        lin("resonance", "Resonance", 0.0, 100.0, 0.0, Unit::Percent),
        lin("drive", "Drive", 0.0, 24.0, 0.0, Unit::Db),
        lin("key_track", "Key Track", -100.0, 100.0, 0.0, Unit::Percent),
        lin("filter_env_amount", "Filter Env Amount", -100.0, 100.0, 0.0, Unit::Percent),
        log("filter_attack", "Filter Attack", 0.5, 10_000.0, 5.0, Unit::Ms),
        log("filter_decay", "Filter Decay", 1.0, 20_000.0, 300.0, Unit::Ms),
        lin("filter_sustain", "Filter Sustain", 0.0, 100.0, 0.0, Unit::Percent),
        log("filter_release", "Filter Release", 1.0, 20_000.0, 300.0, Unit::Ms),
    ],
    ["1", "2"],
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"]
);

//! The Resonator's parameter table (#198 slice 3): the module's controls, the instrument's own
//! (accent, tonic, level, tune, bend), and the shaper and modulation matrix it shares with the
//! Macro Oscillator (#243), under the same ids. Sessions, presets and automation lanes store the
//! ids: never rename one.

use super::{DESTINATIONS, MODELS, POLYPHONIES, TONIC_LABELS, TONICS};
use crate::params::{choice, lin, log, note_choice, toggle};
use crate::plugins::lfo::DIVISIONS;
use crate::plugins::macro_osc::matrix::{CURVES, ROWS, SOURCES};
use crate::plugins::macro_osc::shaper::{FILTER_MODES, LFO_SHAPES};
use crate::{ParamInfo, Unit};

pub(super) const MODEL: usize = 0;
pub(super) const POLYPHONY: usize = 1;
pub(super) const STRUCTURE: usize = 2;
pub(super) const BRIGHTNESS: usize = 3;
pub(super) const DAMPING: usize = 4;
pub(super) const POSITION: usize = 5;
pub(super) const ACCENT: usize = 6;
pub(super) const TONIC: usize = 7;
pub(super) const SPREAD: usize = 8;
pub(super) const LEVEL: usize = 9;
pub(super) const TUNE: usize = 10;
pub(super) const BEND_RANGE: usize = 11;
pub(super) const SHAPER: usize = 12;
pub(super) const AMP_ATTACK: usize = 13;
pub(super) const AMP_DECAY: usize = 14;
pub(super) const AMP_SUSTAIN: usize = 15;
pub(super) const AMP_RELEASE: usize = 16;
pub(super) const AMP_VELOCITY: usize = 17;
pub(super) const FILTER_MODE: usize = 18;
pub(super) const FILTER_SLOPE: usize = 19;
pub(super) const CUTOFF: usize = 20;
pub(super) const RESONANCE: usize = 21;
pub(super) const DRIVE: usize = 22;
pub(super) const KEY_TRACK: usize = 23;
pub(super) const FILTER_ENV_AMOUNT: usize = 24;
pub(super) const FILTER_ATTACK: usize = 25;
pub(super) const FILTER_DECAY: usize = 26;
pub(super) const FILTER_SUSTAIN: usize = 27;
pub(super) const FILTER_RELEASE: usize = 28;
/// The first parameter of LFO 1 and of LFO 2; each has seven, in the order of the `LFO_*` offsets.
pub(super) const LFO1: usize = 29;
pub(super) const LFO2: usize = 36;
pub(super) const LFO_SHAPE: usize = 0;
pub(super) const LFO_RATE: usize = 1;
pub(super) const LFO_SYNC: usize = 2;
pub(super) const LFO_DIVISION: usize = 3;
pub(super) const LFO_RETRIGGER: usize = 4;
pub(super) const LFO_FADE: usize = 5;
pub(super) const LFO_AMOUNT: usize = 6;
/// The first matrix row's first parameter; each row has [`ROW_PARAMS`], in the order of the
/// `ROW_*` offsets (the Macro Oscillator's).
pub(super) const ROW1: usize = 43;
pub(super) const ROW_PARAMS: usize = 5;
pub(super) const ROW_SOURCE: usize = 0;
pub(super) const ROW_VIA: usize = 1;
pub(super) const ROW_DEST: usize = 2;
pub(super) const ROW_AMOUNT: usize = 3;
pub(super) const ROW_CURVE: usize = 4;

/// Parameters in all.
pub(super) const COUNT: usize = ROW1 + ROWS * ROW_PARAMS;

/// The table: the fixed parameters, then one LFO's seven per LFO, then five per matrix row (the
/// Macro Oscillator's ids and ranges, the Resonator's destinations).
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
        choice("polyphony", "Polyphony", &POLYPHONIES, 2),
        lin("structure", "Structure", 0.0, 100.0, 50.0, Unit::Percent),
        lin("brightness", "Brightness", 0.0, 100.0, 50.0, Unit::Percent),
        lin("damping", "Damping", 0.0, 100.0, 50.0, Unit::Percent),
        lin("position", "Position", 0.0, 100.0, 50.0, Unit::Percent),
        lin("accent", "Accent", 0.0, 100.0, 50.0, Unit::Percent),
        note_choice("tonic", "Tonic", &TONICS, 24, TONIC_LABELS),
        lin("spread", "Spread", 0.0, 100.0, 50.0, Unit::Percent),
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

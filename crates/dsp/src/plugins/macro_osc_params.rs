//! The Macro Oscillator's parameter table: the module's controls (#195), the voices (#196), the
//! shaper and the modulation matrix (#243). Sessions, presets and automation lanes store the ids:
//! never rename one. The note-to-macro mapping's ids of #196 (`velocity_curve`,
//! `velocity_harmonics` .. `key_decay`) are gone; `transit_model::legacy_mapping` turns them into
//! matrix rows when an old session or preset loads.

use super::matrix::{CURVES, DESTINATIONS, ROWS, Row, SOURCES};
use super::shaper::{self, FILTER_MODES, LFO_SHAPES, LfoSettings, Settings};
use super::{MODELS, VOICE_COUNTS, WHEEL_DESTINATIONS};
use crate::params::{choice, choice_index, lin, log, on, toggle};
use crate::plugins::instrument_queue::AdsrRates;
use crate::plugins::lfo::DIVISIONS;
use crate::{PageInfo, PageSlot, ParamInfo, Unit, db_to_gain};

pub(super) const MODEL: usize = 0;
pub(super) const HARMONICS: usize = 1;
pub(super) const TIMBRE: usize = 2;
pub(super) const MORPH: usize = 3;
pub(super) const DECAY: usize = 4;
pub(super) const LPG_COLOR: usize = 5;
pub(super) const AUX_MIX: usize = 6;
pub(super) const LEVEL: usize = 7;
pub(super) const TUNE: usize = 8;
pub(super) const BEND_RANGE: usize = 9;
pub(super) const MOD_WHEEL: usize = 10;
pub(super) const VOICES: usize = 11;
pub(crate) const SHAPER: usize = 12;
pub(super) const AMP_ATTACK: usize = 13;
pub(super) const AMP_DECAY: usize = 14;
pub(super) const AMP_SUSTAIN: usize = 15;
pub(crate) const AMP_RELEASE: usize = 16;
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
/// `ROW_*` offsets.
pub(super) const ROW1: usize = 43;
pub(super) const ROW_PARAMS: usize = 5;
pub(super) const ROW_SOURCE: usize = 0;
pub(super) const ROW_VIA: usize = 1;
pub(super) const ROW_DEST: usize = 2;
pub(super) const ROW_AMOUNT: usize = 3;
pub(super) const ROW_CURVE: usize = 4;

/// Parameters in all.
pub(crate) const COUNT: usize = ROW1 + ROWS * ROW_PARAMS;

/// The table: the fixed parameters, then one LFO's seven per LFO, then five per matrix row.
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

pub(crate) static PARAMS: [ParamInfo; COUNT] = table!(
    [
        choice("model", "Model", &MODELS, 8),
        lin("harmonics", "Harmonics", 0.0, 100.0, 50.0, Unit::Percent),
        lin("timbre", "Timbre", 0.0, 100.0, 50.0, Unit::Percent),
        lin("morph", "Morph", 0.0, 100.0, 50.0, Unit::Percent),
        lin("decay", "Decay", 0.0, 100.0, 50.0, Unit::Percent),
        lin("lpg_color", "LPG Color", 0.0, 100.0, 50.0, Unit::Percent),
        lin("aux_mix", "Aux Mix", 0.0, 100.0, 0.0, Unit::Percent),
        lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
        lin("tune", "Tune", -24.0, 24.0, 0.0, Unit::Semitones),
        lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
        choice("mod_wheel", "Mod Wheel", &WHEEL_DESTINATIONS, 0),
        choice("voices", "Voices", &VOICE_COUNTS, 7),
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

/// The factory pages of the macro strip (ADR 0047 decision 2.3). Page 1, "Main": the module's
/// four macros and level, then the shaper's cutoff and amp envelope ends, which act only while
/// `shaper` is on and so dim with it. The second page ("Shaper") comes with #356.
pub(crate) static PAGES: [PageInfo; 1] = [PageInfo {
    name: "Main",
    slots: &[
        PageSlot::new("model"),
        PageSlot::new("harmonics"),
        PageSlot::new("timbre"),
        PageSlot::new("morph"),
        PageSlot::new("level"),
        PageSlot::enabled_by("cutoff", "shaper"),
        PageSlot::enabled_by("amp_attack", "shaper"),
        PageSlot::enabled_by("amp_release", "shaper"),
    ],
}];

/// The id of matrix row `row`'s (0-based) parameter at offset `field` (a `ROW_*` offset).
#[must_use]
pub fn row_param(row: usize, field: usize) -> Option<&'static str> {
    (row < ROWS && field < ROW_PARAMS).then(|| PARAMS.get(ROW1 + row * ROW_PARAMS + field).map(|p| p.id)).flatten()
}

/// The shaper's settings at rate `sr` from the parameter values `v` gives by index into [`PARAMS`]
/// (the shaper block, [`SHAPER`] to the last LFO parameter): the Macro Oscillator reads its own
/// table, the Sampler a zone's slot values laid out the same way (ADR 0041), so both build the
/// same bits from the same values.
pub(crate) fn shaper_settings(v: &dyn Fn(usize) -> f32, bar_quarters: f64, bar_origin: (i64, f64), sr: f32) -> Settings {
    let pct = |i: usize| v(i) / 100.0;
    let lfo = |first: usize| {
        LfoSettings::new(
            choice_index(v(first + LFO_SHAPE)),
            v(first + LFO_RATE),
            on(v(first + LFO_SYNC)),
            choice_index(v(first + LFO_DIVISION)),
            on(v(first + LFO_RETRIGGER)),
            v(first + LFO_FADE),
            pct(first + LFO_AMOUNT),
            bar_quarters,
            bar_origin,
            sr,
        )
    };
    Settings {
        on: on(v(SHAPER)),
        amp: AdsrRates::new(v(AMP_ATTACK), v(AMP_DECAY), pct(AMP_SUSTAIN), v(AMP_RELEASE), sr),
        amp_velocity: pct(AMP_VELOCITY),
        filter_env: AdsrRates::new(v(FILTER_ATTACK), v(FILTER_DECAY), pct(FILTER_SUSTAIN), v(FILTER_RELEASE), sr),
        mode: shaper::filter_mode(choice_index(v(FILTER_MODE))),
        slope24: choice_index(v(FILTER_SLOPE)) == 1,
        cutoff_log2: v(CUTOFF).max(1.0).log2(),
        resonance: pct(RESONANCE),
        drive: db_to_gain(v(DRIVE)),
        key_track: pct(KEY_TRACK),
        env_amount: pct(FILTER_ENV_AMOUNT),
        lfo: [lfo(LFO1), lfo(LFO2)],
        sr,
    }
}

/// Matrix row `r` (0-based) from the parameter values `v` gives by index into [`PARAMS`].
pub(crate) fn matrix_row(v: &dyn Fn(usize) -> f32, r: usize) -> Row {
    let at = ROW1 + r * ROW_PARAMS;
    Row {
        source: choice_index(v(at + ROW_SOURCE)),
        via: choice_index(v(at + ROW_VIA)),
        dest: choice_index(v(at + ROW_DEST)),
        curve: choice_index(v(at + ROW_CURVE)),
        amount: v(at + ROW_AMOUNT) / 100.0,
    }
}

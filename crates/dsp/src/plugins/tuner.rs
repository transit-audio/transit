//! Tuner (#266): a chromatic tuner as an insert. The audio passes through untouched, bit for bit
//! and with no latency; the device only asks the mix for a sample tap
//! ([`Plugin::wants_sample_tap`]) so its panel can read the input.
//!
//! The pitch estimate is not made here. [`TunerModel`] is what the panel runs on the UI thread, on
//! the newest window of the tap: McLeod's method ([`crate::pitch_detect::PitchDetector`]), a median
//! of the last five estimates, the note and cents against the reference pitch, and a "no signal"
//! state under -50 dBFS RMS or when the detector is not sure. The audio thread does nothing.

use crate::params::{Params, log, param_plumbing, toggle};
use crate::pitch_detect::{DEFAULT_CLARITY, PitchDetector, nearest_note};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};

static TUNER_PARAMS: [ParamInfo; 2] = [log("reference_hz", "Reference", 415.0, 466.0, 440.0, Unit::Hz), toggle("hold", "Hold", false)];

pub static TUNER_INFO: PluginInfo = PluginInfo {
    id: "tuner",
    name: "Tuner",
    short_name: "Tuner",
    category: Category::Other,
    params: &TUNER_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

/// The device: leaves the buffers alone. Its parameters are read by the panel.
pub struct Tuner {
    p: Params,
}

impl Default for Tuner {
    fn default() -> Self {
        Self::new()
    }
}

impl Tuner {
    pub fn new() -> Self {
        Self { p: Params::new(&TUNER_PARAMS) }
    }

    fn update(&mut self) {}
}

impl Plugin for Tuner {
    param_plumbing!(TUNER_INFO);

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}

    fn reset(&mut self) {}

    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}

    fn wants_sample_tap(&self) -> bool {
        true
    }
}

/// Under this RMS (-50 dBFS) the panel shows "no signal".
pub const GATE_RMS: f32 = 0.003_162_277_6;
/// Estimates the median is taken over.
const HISTORY: usize = 5;
/// Analyses without a pitch before the display lets go of the last note.
const MISSES_TO_LOSE: u32 = 3;
/// A new estimate this far (in semitones) from the median starts the median over: a new note.
const JUMP_SEMITONES: f32 = 0.8;

/// One reading as the panel shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// MIDI note number of the nearest note (A4 = 69).
    pub note: i32,
    /// How far the pitch is from that note, -50 to +50 cents.
    pub cents: f32,
    pub hz: f32,
}

impl Reading {
    /// The note's name with its octave, C4 = 60 ("A4", "C#3").
    #[must_use]
    pub fn name(&self) -> String {
        note_label(self.note)
    }

    /// "A4 +0.0 cents".
    #[must_use]
    pub fn text(&self) -> String {
        format!("{} {} cents", self.name(), signed_tenths(self.cents))
    }
}

/// "+0.0", "-12.3": one decimal, never a negative zero.
#[must_use]
pub fn signed_tenths(v: f32) -> String {
    let t = (v * 10.0).round() / 10.0;
    if t == 0.0 { "+0.0".to_string() } else { format!("{t:+.1}") }
}

/// The name of a MIDI note with its octave, C4 = 60 (sharps).
#[must_use]
pub fn note_label(note: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let class = usize::try_from(note.rem_euclid(12)).ok().and_then(|i| NAMES.get(i)).copied().unwrap_or("?");
    format!("{class}{}", note.div_euclid(12) - 1)
}

/// What the Tuner's panel keeps between frames: the detector, the window it reads, the recent
/// estimates and the reading. UI thread only; nothing here allocates after [`TunerModel::new`].
pub struct TunerModel {
    det: PitchDetector,
    window: Vec<f32>,
    hist: [f32; HISTORY],
    count: usize,
    misses: u32,
    reading: Option<Reading>,
    sample_rate: f32,
}

impl TunerModel {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let det = PitchDetector::new(sample_rate);
        let window = vec![0.0; det.window()];
        TunerModel { det, window, hist: [0.0; HISTORY], count: 0, misses: 0, reading: None, sample_rate }
    }

    /// The session rate this model was made for.
    #[must_use]
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// The window to fill from the tap (the newest samples last) before [`TunerModel::analyze`].
    pub fn window_mut(&mut self) -> &mut [f32] {
        &mut self.window
    }

    /// The reading as of the last analysis (`None`: no signal).
    #[must_use]
    pub fn reading(&self) -> Option<Reading> {
        self.reading
    }

    /// Analyzes the window against `reference_hz`. With `hold` on nothing changes: the display
    /// keeps the last reading.
    pub fn analyze(&mut self, reference_hz: f32, hold: bool) {
        if hold {
            return;
        }
        let rms = (self.window.iter().map(|x| x * x).sum::<f32>() / self.window.len().max(1) as f32).sqrt();
        let found = if rms >= GATE_RMS { self.det.detect(&self.window, DEFAULT_CLARITY) } else { None };
        let Some(e) = found else {
            self.miss();
            return;
        };
        self.misses = 0;
        if self.count > 0 && (12.0 * (e.hz / self.median()).log2()).abs() > JUMP_SEMITONES {
            self.count = 0;
        }
        self.hist[self.count % HISTORY] = e.hz;
        self.count += 1;
        let hz = self.median();
        self.reading = nearest_note(hz, reference_hz).map(|(note, cents)| Reading { note, cents, hz });
    }

    /// An analysis round that found nothing to analyze: no new frames reached the tap since the
    /// last one (a bypassed Tuner, a stopped source), or no pitch in them. Three in a row let go
    /// of the note. Does nothing while the display is held (hold never calls it).
    pub fn miss(&mut self) {
        self.misses += 1;
        if self.misses >= MISSES_TO_LOSE {
            self.count = 0;
            self.reading = None;
        }
    }

    /// Median of the estimates kept (the newest five).
    fn median(&self) -> f32 {
        let n = self.count.min(HISTORY);
        let mut v = [0.0f32; HISTORY];
        v.iter_mut().zip(self.hist.iter()).take(n).for_each(|(d, s)| *d = *s);
        let v = v.get_mut(..n).unwrap_or(&mut []);
        v.sort_by(f32::total_cmp);
        v.get(n / 2).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
#[path = "tuner_tests.rs"]
mod tests;

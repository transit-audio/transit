//! A picture of the current model for the Macro Oscillator's panel (#326, job 326-W): one cycle
//! of a pitched model, or the first 50 ms of a noise or percussion model, rendered from the
//! parameters by a firmware voice of its own. The panel's well draws it instead of scoping the
//! summed output: one voice's shape is what tells you what a knob does.
//!
//! - **Off the audio thread.** [`render`] builds its own [`Voice`] at 48 kHz (concert tuning and
//!   same-pitch restart, as every voice of the instrument) and allocates; the panel runs it on a
//!   worker. Never the instrument's voices.
//! - **The voice's own path.** The render feeds the firmware voice the same `Patch` fields a voice
//!   of the instrument does (`note` 0, the request's Harmonics, Timbre, Morph, model, Decay and
//!   color) and holds its trigger high at the request's note, in 12-frame blocks. The port has no
//!   caller-side bypass of the low-pass gate while the trigger is patched (the six-op models bypass
//!   it from inside their engine), so the level input is patched and held at 1, as a shaped voice
//!   holds it: the gate follows the level and stays fully open whatever the Decay.
//! - **Pitched models (1 to 16):** a [`SETTLE`] of 200 ms, then exactly one period at the note
//!   (`48000 / (440 · 2^((note − 69) / 12))` frames, rounded) from the first rising zero crossing
//!   after the settle (the frame before it below zero, the frame at or above zero); with no
//!   crossing within two periods, from the settle's end.
//! - **Noise and percussion models (17 to 24):** the first [`BURST`] frames (50 ms) from the
//!   trigger, no settle.
//! - The main output only (not aux), as -1..1 floats, not normalized (the well scales).
//!   Deterministic: the same request renders the same samples.

use super::{BLOCK, to_f32};
use transit_macro_osc::utils::random::Random;
use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc::{SampleRate, Tuning};

/// The preview's rate.
pub const RATE: f32 = 48_000.0;
/// Frames rendered before a cycle is taken: 200 ms.
pub const SETTLE: usize = 9_600;
/// A burst's length: 50 ms.
pub const BURST: usize = 2_400;
/// Models from this index on (0-based; the 17th on) render a burst.
pub const BURST_FROM: usize = 16;
/// The instance's random seed: voice 0's (the firmware's power-on seed).
pub const INSTANCE_SEED: u32 = Random::DEFAULT_SEED;

/// What to render: the model (0-based firmware index), the note, and the knobs (0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Request {
    pub engine: usize,
    pub note: f32,
    pub harmonics: f32,
    pub timbre: f32,
    pub morph: f32,
    pub decay: f32,
    pub lpg_color: f32,
    pub seed: u32,
}

/// What a render holds: one period of a pitched model, or a burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cycle,
    Burst,
}

/// Which kind a model renders.
#[must_use]
pub fn kind_of(engine: usize) -> Kind {
    if engine >= BURST_FROM { Kind::Burst } else { Kind::Cycle }
}

/// One period at `note` in frames at [`RATE`] (concert pitch), rounded; the note is clamped to
/// 0..127 (a non-finite one reads 60).
#[must_use]
pub fn period(note: f32) -> usize {
    let n = clamp_note(note);
    let hz = 440.0 * 2f32.powf((n - 69.0) / 12.0);
    ((RATE / hz).round() as usize).max(1)
}

/// The first rising zero crossing in `x` at or after `from` and before `from + span`: the frame
/// whose predecessor is below zero while it is at or above zero.
#[must_use]
pub fn rising_crossing(x: &[f32], from: usize, span: usize) -> Option<usize> {
    (from.max(1)..from.saturating_add(span).min(x.len())).find(|&i| x.get(i - 1).is_some_and(|a| *a < 0.0) && x.get(i).is_some_and(|b| *b >= 0.0))
}

/// Renders `req` into `out` (cleared first) and says which kind it holds. Allocates: never on
/// the audio thread.
pub fn render(req: &Request, out: &mut Vec<f32>) -> Kind {
    out.clear();
    let engine = req.engine.min(transit_macro_osc::voice::NUM_ENGINES - 1);
    let kind = kind_of(engine);
    let note = clamp_note(req.note);
    let p = period(note);
    let total = match kind {
        Kind::Cycle => SETTLE + 3 * p,
        Kind::Burst => BURST,
    };
    let samples = stream(req, engine, note, total);
    match kind {
        Kind::Cycle => {
            let start = rising_crossing(&samples, SETTLE, 2 * p).unwrap_or(SETTLE);
            out.extend_from_slice(samples.get(start..start + p).unwrap_or(&[]));
        }
        Kind::Burst => out.extend_from_slice(samples.get(..BURST).unwrap_or(&[])),
    }
    kind
}

/// The note a render plays: clamped to 0..127, a non-finite one reads 60.
fn clamp_note(note: f32) -> f32 {
    if note.is_finite() { note.clamp(0.0, 127.0) } else { 60.0 }
}

/// The voice's main output from the trigger on, at least `total` frames (whole blocks).
pub(crate) fn stream(req: &Request, engine: usize, note: f32, total: usize) -> Vec<f32> {
    let mut voice = Box::new(Voice::new(SampleRate::Hz48000));
    voice.set_tuning(Tuning::Concert);
    voice.set_same_pitch_restart(true);
    voice.seed(req.seed);
    let knob = |x: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    let patch = Patch {
        note: 0.0,
        harmonics: knob(req.harmonics),
        timbre: knob(req.timbre),
        morph: knob(req.morph),
        engine,
        decay: knob(req.decay),
        lpg_colour: knob(req.lpg_color),
        ..Patch::default()
    };
    let mods = Modulations { note, trigger: 1.0, level: 1.0, trigger_patched: true, level_patched: true, ..Modulations::default() };
    let mut samples = Vec::with_capacity(total.div_ceil(BLOCK) * BLOCK);
    let mut frames = [Frame::default(); BLOCK];
    while samples.len() < total {
        voice.render(&patch, &mods, &mut frames);
        samples.extend(frames.iter().map(|f| to_f32(f.out)));
    }
    samples
}

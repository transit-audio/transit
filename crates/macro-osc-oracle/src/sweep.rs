//! The fixed parameter sweeps every engine is rendered with (ADR 0033 gate, #194).
//!
//! A sweep is a list of points. Each point is rendered from a fresh engine or voice, after
//! seeding the random state with [`SEED`], at 48 kHz in the firmware's 12-frame blocks, for a
//! fixed number of frames. Fresh state per point keeps a residual local: a failing point names
//! its parameters, not the history before it.
//!
//! Points, per engine:
//!
//! - **Engine level** (float out/aux before post-processing): for each note in [`NOTES`] and each
//!   trigger mode (free-running, then triggered), harmonics, timbre and morph are swept one at a
//!   time over [`LEVELS`] with the other two at 0.5; the all-0.5 center is rendered once.
//!   3 × 2 × 13 = 78 points.
//! - **Voice level** (int16 frames through the voice's envelopes, low-pass gate and limiter): the
//!   same 78 points with decay 0.5, plus the low-pass gate decay swept over the other values of
//!   [`DECAYS`] for each note at the triggered center. 84 points.
//!
//! A triggered point fires on its first block and holds the gate for the first half of the
//! point. The port (#195) renders the same points through [`EnginePoint::parameters`] and
//! [`VoicePoint::inputs`], so both sides see the same values block for block.

#![forbid(unsafe_code)]

use crate::{BLOCK_SIZE, EngineParameters, Error, Frame, Model, Modulations, Patch, SAMPLE_RATE, Session, Trigger};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0194_5EED;

/// Notes every point is rendered at (MIDI semitones).
pub const NOTES: [f32; 3] = [36.0, 60.0, 84.0];

/// Values each macro is swept over.
pub const LEVELS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Low-pass gate decays swept at the voice level.
pub const DECAYS: [f32; 3] = [0.0, 0.5, 1.0];

/// Frames per engine-level point: 50 ms at 48 kHz, 200 blocks.
pub const ENGINE_POINT_FRAMES: usize = 2400;

/// Frames per voice-level point: 100 ms at 48 kHz, 400 blocks, so the envelopes decay.
pub const VOICE_POINT_FRAMES: usize = 4800;

/// The accent the voice passes to an engine when the level input is unpatched.
pub const ACCENT: f32 = 0.8;

/// One engine-level point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnginePoint {
    /// Pitch, MIDI semitones.
    pub note: f32,
    /// Harmonics macro.
    pub harmonics: f32,
    /// Timbre macro.
    pub timbre: f32,
    /// Morph macro.
    pub morph: f32,
    /// Fire a trigger on the first block and hold the gate for half the point; otherwise the
    /// trigger input is unpatched and the engine free-runs.
    pub triggered: bool,
}

impl EnginePoint {
    /// The parameters for block `block` of a point `blocks` long.
    #[must_use]
    pub fn parameters(&self, block: usize, blocks: usize) -> EngineParameters {
        let trigger = if !self.triggered {
            Trigger::UNPATCHED
        } else if block == 0 {
            Trigger::RISING_EDGE.with(Trigger::HIGH)
        } else if block < blocks / 2 {
            Trigger::HIGH
        } else {
            Trigger::LOW
        };
        EngineParameters { trigger, note: self.note, timbre: self.timbre, morph: self.morph, harmonics: self.harmonics, accent: ACCENT }
    }

    /// A stable label for reports, such as `n60 h0.25 t0.50 m0.50 trig`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("n{} h{:.2} t{:.2} m{:.2} {}", self.note, self.harmonics, self.timbre, self.morph, if self.triggered { "trig" } else { "free" })
    }
}

/// One voice-level point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoicePoint {
    /// The engine-level settings.
    pub engine: EnginePoint,
    /// Low-pass gate and envelope decay.
    pub decay: f32,
}

impl VoicePoint {
    /// The patch and modulation inputs for block `block` of a point `blocks` long.
    #[must_use]
    pub fn inputs(&self, model: Model, block: usize, blocks: usize) -> (Patch, Modulations) {
        let e = &self.engine;
        let patch = Patch { note: e.note, harmonics: e.harmonics, timbre: e.timbre, morph: e.morph, decay: self.decay, ..Patch::centered(model) };
        let trigger = if e.triggered && block < blocks / 2 { 1.0 } else { 0.0 };
        let modulations = Modulations { trigger, trigger_patched: e.triggered, ..Modulations::default() };
        (patch, modulations)
    }

    /// A stable label for reports.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} d{:.2}", self.engine.label(), self.decay)
    }
}

/// The 78 engine-level points, in a fixed order.
#[must_use]
pub fn engine_points() -> Vec<EnginePoint> {
    let mut points = Vec::new();
    for note in NOTES {
        for triggered in [false, true] {
            for axis in 0..3 {
                for level in LEVELS {
                    #[allow(clippy::float_cmp)] // Exact constants: the center is rendered on the first axis only.
                    if axis > 0 && level == 0.5 {
                        continue;
                    }
                    let mut macros = [0.5_f32; 3];
                    macros[axis] = level;
                    points.push(EnginePoint { note, harmonics: macros[0], timbre: macros[1], morph: macros[2], triggered });
                }
            }
        }
    }
    points
}

/// The 84 voice-level points, in a fixed order.
#[must_use]
pub fn voice_points() -> Vec<VoicePoint> {
    let mut points: Vec<VoicePoint> = engine_points().into_iter().map(|engine| VoicePoint { engine, decay: 0.5 }).collect();
    for note in NOTES {
        for decay in DECAYS {
            #[allow(clippy::float_cmp)] // Exact constants: decay 0.5 is already in the points above.
            if decay == 0.5 {
                continue;
            }
            let engine = EnginePoint { note, harmonics: 0.5, timbre: 0.5, morph: 0.5, triggered: true };
            points.push(VoicePoint { engine, decay });
        }
    }
    points
}

/// An engine-level render: float out and aux.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineRender {
    /// Main output.
    pub out: Vec<f32>,
    /// Auxiliary output.
    pub aux: Vec<f32>,
}

/// Renders one engine-level point from a fresh engine.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_engine_point(session: &Session, model: Model, point: &EnginePoint) -> Result<EngineRender, Error> {
    session.seed(SEED);
    let mut engine = session.engine(model)?;
    let blocks = ENGINE_POINT_FRAMES / BLOCK_SIZE;
    let mut render = EngineRender { out: vec![0.0; ENGINE_POINT_FRAMES], aux: vec![0.0; ENGINE_POINT_FRAMES] };
    let (outs, _) = render.out.as_chunks_mut::<BLOCK_SIZE>();
    let (auxes, _) = render.aux.as_chunks_mut::<BLOCK_SIZE>();
    for (block, (out, aux)) in outs.iter_mut().zip(auxes.iter_mut()).enumerate() {
        engine.render(&point.parameters(block, blocks), out, aux)?;
    }
    Ok(render)
}

/// Renders one voice-level point from a fresh voice.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_voice_point(session: &Session, model: Model, point: &VoicePoint) -> Result<Vec<Frame>, Error> {
    session.seed(SEED);
    let mut voice = session.voice(SAMPLE_RATE)?;
    let blocks = VOICE_POINT_FRAMES / BLOCK_SIZE;
    let mut frames = vec![Frame::default(); VOICE_POINT_FRAMES];
    for (block, chunk) in frames.as_chunks_mut::<BLOCK_SIZE>().0.iter_mut().enumerate() {
        let (patch, modulations) = point.inputs(model, block, blocks);
        voice.render(&patch, &modulations, chunk)?;
    }
    Ok(frames)
}

/// FNV-1a 64 over the bit patterns of a float render, for comparing renders across processes.
#[must_use]
pub fn digest_f32(samples: &[f32]) -> u64 {
    digest(samples.iter().flat_map(|s| s.to_bits().to_le_bytes()))
}

/// FNV-1a 64 over the voice frames (out then aux, little-endian).
#[must_use]
pub fn digest_frames(frames: &[Frame]) -> u64 {
    digest(frames.iter().flat_map(|f| {
        let [a, b] = f.out.to_le_bytes();
        let [c, d] = f.aux.to_le_bytes();
        [a, b, c, d]
    }))
}

fn digest(bytes: impl Iterator<Item = u8>) -> u64 {
    bytes.fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// One line per point and level, `<engine> <level> <point> <digest>`, for the whole sweep of
/// every engine. Two runs with equal lines rendered bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    for model in Model::ALL {
        for point in engine_points() {
            let r = render_engine_point(session, model, &point)?;
            lines.push(format!("{} engine {} {:016x}{:016x}", model.label(), point.label(), digest_f32(&r.out), digest_f32(&r.aux)));
        }
        for point in voice_points() {
            let frames = render_voice_point(session, model, &point)?;
            lines.push(format!("{} voice {} {:016x}", model.label(), point.label(), digest_frames(&frames)));
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(engine_points().len(), 78);
        assert_eq!(voice_points().len(), 84);
        assert_eq!(ENGINE_POINT_FRAMES % BLOCK_SIZE, 0);
        assert_eq!(VOICE_POINT_FRAMES % BLOCK_SIZE, 0);
        let labels: std::collections::HashSet<_> = voice_points().iter().map(VoicePoint::label).collect();
        assert_eq!(labels.len(), 84, "every point is rendered once");
    }

    #[test]
    fn a_triggered_point_fires_once_and_releases_at_half() {
        let p = EnginePoint { note: 60.0, harmonics: 0.5, timbre: 0.5, morph: 0.5, triggered: true };
        assert_eq!(p.parameters(0, 200).trigger, Trigger(5));
        assert_eq!(p.parameters(1, 200).trigger, Trigger::HIGH);
        assert_eq!(p.parameters(100, 200).trigger, Trigger::LOW);
        let free = EnginePoint { triggered: false, ..p };
        assert_eq!(free.parameters(0, 200).trigger, Trigger::UNPATCHED);
    }
}

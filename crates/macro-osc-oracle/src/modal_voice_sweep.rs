//! The fixed sweep every modal voice configuration is rendered with (ADR 0033 gate, #198 slice 2).
//!
//! A point is rendered from a fresh part, after seeding the random state with [`SEED`], at 32 kHz in
//! the firmware's 16-frame blocks, for [`POINT_FRAMES`] frames (250 ms). Fresh state per point
//! keeps a residual local: a failing point names its parameters, not the history before it.
//!
//! Every point plays the same gate pattern ([`Point::gate`]): on for the first 40% (a strike on the
//! rising edge, the bow and blow sustaining), off for 20% (the release, the strike's palm mute), on
//! again for 20% (a retrigger while the resonator still rings), then off. Points, per configuration
//! (the three resonator models and the easter-egg voice, [`Configuration`]):
//!
//! - for each note in [`NOTES`], each of the fourteen panel controls and the strength swept one at
//!   a time over [`LEVELS`] (space over [`SPACE_LEVELS`], whose top freezes the reverb) with the
//!   others at the center ([`center`]: every control 0.5, strength 0.5); the center is rendered
//!   once: 15 x 4 + 1 = 61 points per note;
//! - for each note, each exciter alone at the center (bow, blow, strike): 3 points;
//! - for each note, the center with the FM input at each of [`MODULATIONS`]: 2 points;
//! - for each note, the center after `Part::Seed` with each of [`SERIALS`]: 2 points;
//! - for each note, the center with both audio inputs patched ([`Point::external`]): 1 point.
//!
//! 69 points per note, 207 per configuration, 828 renders. The port (#198 slice 2) renders the same
//! points through [`Point::performance`], [`Point::inputs`] and [`Point::serial`], so both sides see
//! the same values block for block.

#![forbid(unsafe_code)]

use crate::modal_voice::{MAX_CONTROL, MAX_STRIKE_TIMBRE, MODAL_VOICE_BLOCK_SIZE, ModalVoiceModel, ModalVoicePatch, Performance};
use crate::{Error, Session};

/// The seed written to the random state before each point.
pub const SEED: u32 = 0x0198_E1E5;

/// Notes every point is rendered at (MIDI semitones).
pub const NOTES: [f32; 3] = [36.0, 60.0, 84.0];

/// Values each control and the strength are swept over. The top is the CV scaler's cap, 0.9995
/// (the strike timbre's own cap, 0.995, replaces it there).
pub const LEVELS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, MAX_CONTROL];

/// Values space is swept over: dry, the spread, the reverb, and from 1.75 the frozen reverb.
pub const SPACE_LEVELS: [f32; 5] = [0.0, 0.5, 1.0, 1.5, 2.0];

/// FM input values (semitones) rendered at each note's center.
pub const MODULATIONS: [f32; 2] = [-12.0, 7.5];

/// Serial numbers `Part::Seed` is called with at each note's center (the module passes its chip's
/// three-word serial).
pub const SERIALS: [[u32; 3]; 2] = [[0x0033_0021, 0x3436_5111, 0x2035_3233], [0xDEAD_BEEF, 0x0000_0001, 0x8000_0000]];

/// Frames per point: 250 ms at 32 kHz, 500 blocks.
pub const POINT_FRAMES: usize = 8000;

/// What renders the voice: one of the three resonator models, or the easter-egg voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Configuration {
    /// The exciters into a resonator model.
    Model(ModalVoiceModel),
    /// The easter-egg voice (two FM oscillators; the resonator model is not used).
    EasterEgg,
}

impl Configuration {
    /// Every configuration, in sweep order.
    pub const ALL: [Self; 4] =
        [Self::Model(ModalVoiceModel::Modal), Self::Model(ModalVoiceModel::String), Self::Model(ModalVoiceModel::Strings), Self::EasterEgg];

    /// A stable label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Model(m) => m.label(),
            Self::EasterEgg => "easter-egg",
        }
    }
}

/// The center every axis is swept around: every control 0.5.
#[must_use]
pub fn center() -> ModalVoicePatch {
    ModalVoicePatch {
        envelope_shape: 0.5,
        bow_level: 0.5,
        bow_timbre: 0.5,
        blow_level: 0.5,
        blow_meta: 0.5,
        blow_timbre: 0.5,
        strike_level: 0.5,
        strike_meta: 0.5,
        strike_timbre: 0.5,
        geometry: 0.5,
        brightness: 0.5,
        damping: 0.5,
        position: 0.5,
        space: 0.5,
    }
}

/// The swept axes, in order: the fourteen controls, then the strength.
pub const AXES: [&str; 15] = [
    "envelope",
    "bow-level",
    "bow-timbre",
    "blow-level",
    "blow-meta",
    "blow-timbre",
    "strike-level",
    "strike-meta",
    "strike-timbre",
    "geometry",
    "brightness",
    "damping",
    "position",
    "space",
    "strength",
];

fn control(patch: &mut ModalVoicePatch, axis: usize) -> Option<&mut f32> {
    Some(match axis {
        0 => &mut patch.envelope_shape,
        1 => &mut patch.bow_level,
        2 => &mut patch.bow_timbre,
        3 => &mut patch.blow_level,
        4 => &mut patch.blow_meta,
        5 => &mut patch.blow_timbre,
        6 => &mut patch.strike_level,
        7 => &mut patch.strike_meta,
        8 => &mut patch.strike_timbre,
        9 => &mut patch.geometry,
        10 => &mut patch.brightness,
        11 => &mut patch.damping,
        12 => &mut patch.position,
        13 => &mut patch.space,
        _ => return None,
    })
}

/// One point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Pitch, MIDI semitones.
    pub note: f32,
    /// The panel controls.
    pub patch: ModalVoicePatch,
    /// The strength input.
    pub strength: f32,
    /// The FM input, semitones.
    pub modulation: f32,
    /// `Part::Seed` words, if the part is seeded before the first block.
    pub serial: Option<[u32; 3]>,
    /// Both audio inputs patched ([`Point::inputs`]).
    pub external: bool,
    label: &'static str,
    level: f32,
}

impl Point {
    /// The gate in block `block` of a point `blocks` long: on for the first 40%, off for 20%, on
    /// for 20%, then off.
    #[must_use]
    pub fn gate(block: usize, blocks: usize) -> bool {
        let tenth = blocks / 10;
        block < 4 * tenth || (6 * tenth..8 * tenth).contains(&block)
    }

    /// The performance state for block `block` of a point `blocks` long.
    #[must_use]
    pub fn performance(&self, block: usize, blocks: usize) -> Performance {
        Performance { gate: Self::gate(block, blocks), note: self.note, modulation: self.modulation, strength: self.strength }
    }

    /// The two audio inputs (blow, strike) of the whole point: silence, or for an external point a
    /// low noise into the blow input (a 32-bit linear congruential generator, 0.1 peak) and on the
    /// strike input an impulse of 0.8 at each rising gate edge, decaying by 0.98 per sample (in
    /// `f32`).
    #[must_use]
    pub fn inputs(&self, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut blow = vec![0.0_f32; frames];
        let mut strike = vec![0.0_f32; frames];
        if !self.external {
            return (blow, strike);
        }
        let mut state = 0x1234_5678_u32;
        for s in &mut blow {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            #[allow(clippy::cast_precision_loss)] // A value below 2^24, exactly representable.
            let unit = (state >> 8) as f32 / 16_777_216.0;
            *s = (unit - 0.5) * 0.2;
        }
        let blocks = frames / MODAL_VOICE_BLOCK_SIZE;
        let mut impulse = 0.0_f32;
        for (block, chunk) in strike.chunks_mut(MODAL_VOICE_BLOCK_SIZE).enumerate() {
            if Self::gate(block, blocks) && (block == 0 || !Self::gate(block - 1, blocks)) {
                impulse = 0.8;
            }
            for s in chunk {
                *s = impulse;
                impulse *= 0.98;
            }
        }
        (blow, strike)
    }

    /// A stable label for reports, such as `n60 geometry=0.25` or `n36 center`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.label == "center" || self.label.starts_with("solo") || self.label == "external" {
            format!("n{} {}", self.note, self.label)
        } else if self.label == "serial" {
            let s = self.serial.unwrap_or_default();
            format!("n{} serial={:08x}", self.note, s[0])
        } else {
            format!("n{} {}={}", self.note, self.label, self.level)
        }
    }
}

/// The 207 points, in a fixed order.
#[must_use]
pub fn points() -> Vec<Point> {
    let mut points = Vec::new();
    for note in NOTES {
        let base = Point { note, patch: center(), strength: 0.5, modulation: 0.0, serial: None, external: false, label: "center", level: 0.5 };
        points.push(base);
        for (axis, name) in AXES.iter().enumerate() {
            let levels = if *name == "space" { SPACE_LEVELS } else { LEVELS };
            for level in levels {
                #[allow(clippy::float_cmp)] // Exact constants: the center is rendered once.
                if level == 0.5 {
                    continue;
                }
                let mut point = Point { label: name, level, ..base };
                let value = if *name == "strike-timbre" { level.min(MAX_STRIKE_TIMBRE) } else { level };
                match control(&mut point.patch, axis) {
                    Some(c) => *c = value,
                    None => point.strength = if level >= MAX_CONTROL { 1.0 } else { level },
                }
                points.push(point);
            }
        }
        for (label, which) in [("solo-bow", 1), ("solo-blow", 3), ("solo-strike", 6)] {
            let mut patch = center();
            patch.bow_level = 0.0;
            patch.blow_level = 0.0;
            patch.strike_level = 0.0;
            if let Some(c) = control(&mut patch, which) {
                *c = 0.75;
            }
            points.push(Point { patch, label, ..base });
        }
        for modulation in MODULATIONS {
            points.push(Point { modulation, label: "fm", level: modulation, ..base });
        }
        for serial in SERIALS {
            points.push(Point { serial: Some(serial), label: "serial", ..base });
        }
        points.push(Point { external: true, label: "external", ..base });
    }
    points
}

/// A render: float main and aux.
#[derive(Clone, Debug, PartialEq)]
pub struct Render {
    /// Main output (the module's right/out jack: the center minus the side).
    pub main: Vec<f32>,
    /// Auxiliary output (the left jack: the center plus the side, with the dry exciter mixed in at
    /// low space).
    pub aux: Vec<f32>,
}

/// Renders one point from a fresh part.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn render_point(session: &Session, configuration: Configuration, point: &Point) -> Result<Render, Error> {
    session.seed(SEED);
    let mut part = session.modal_voice()?;
    match configuration {
        Configuration::Model(model) => part.set_model(model),
        Configuration::EasterEgg => part.set_easter_egg(true),
    }
    if let Some(words) = point.serial {
        part.seed(words);
    }
    let blocks = POINT_FRAMES / MODAL_VOICE_BLOCK_SIZE;
    let (blow, strike) = point.inputs(POINT_FRAMES);
    let mut render = Render { main: vec![0.0; POINT_FRAMES], aux: vec![0.0; POINT_FRAMES] };
    let blows = blow.as_chunks::<MODAL_VOICE_BLOCK_SIZE>().0;
    let strikes = strike.as_chunks::<MODAL_VOICE_BLOCK_SIZE>().0;
    let mains = render.main.as_chunks_mut::<MODAL_VOICE_BLOCK_SIZE>().0;
    let auxes = render.aux.as_chunks_mut::<MODAL_VOICE_BLOCK_SIZE>().0;
    for (block, (((b, s), m), a)) in blows.iter().zip(strikes).zip(mains.iter_mut()).zip(auxes.iter_mut()).enumerate() {
        part.process(&point.performance(block, blocks), &point.patch, b, s, m, a)?;
    }
    Ok(render)
}

/// One line per configuration and point, `<configuration> <point> <digest>`, for the whole sweep.
/// Two runs with equal lines rendered bit-identical output.
///
/// # Errors
///
/// An [`Error`] from the oracle.
pub fn digest_lines(session: &Session) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    for configuration in Configuration::ALL {
        for point in points() {
            let r = render_point(session, configuration, &point)?;
            let (main, aux) = (crate::sweep::digest_f32(&r.main), crate::sweep::digest_f32(&r.aux));
            lines.push(format!("{} {} {main:016x}{aux:016x}", configuration.label(), point.label()));
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sweep_has_the_documented_shape() {
        assert_eq!(points().len(), 207);
        assert_eq!(POINT_FRAMES % MODAL_VOICE_BLOCK_SIZE, 0);
        let labels: std::collections::HashSet<_> = points().iter().map(Point::label).collect();
        assert_eq!(labels.len(), 207, "every point is rendered once");
        assert!(points().iter().all(|p| crate::modal_voice::inputs_in_range(&p.performance(0, 500), &p.patch, &[0.0], &[0.0])));
    }

    #[test]
    fn the_gate_pattern_strikes_twice() {
        let gates: Vec<bool> = (0..500).map(|b| Point::gate(b, 500)).collect();
        let rising = (0..500).filter(|&b| gates[b] && (b == 0 || !gates[b - 1])).count();
        let falling = (1..500).filter(|&b| !gates[b] && gates[b - 1]).count();
        assert_eq!((rising, falling), (2, 2));
        assert!(gates[0] && !gates[200] && gates[300] && !gates[499]);
    }

    #[test]
    fn external_inputs_follow_the_gate_edges() {
        let p = points().into_iter().find(|p| p.external).unwrap();
        let (blow, strike) = p.inputs(POINT_FRAMES);
        assert_eq!(strike[0].to_bits(), 0.8_f32.to_bits());
        assert_eq!(strike[300 * MODAL_VOICE_BLOCK_SIZE].to_bits(), 0.8_f32.to_bits());
        assert!(blow.iter().all(|s| s.abs() <= 0.1) && blow.iter().any(|s| s.abs() > 0.05));
    }
}

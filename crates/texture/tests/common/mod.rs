//! Shared by the texture processor's null tests: the oracle's controls and qualities in the port's
//! types, and the port's render of a sweep point.

#![forbid(unsafe_code)]
#![allow(dead_code)] // Each test binary uses a different subset.

use transit_macro_osc_oracle::residual::{Residual, residual_f32};
use transit_macro_osc_oracle::texture as oracle;
use transit_macro_osc_oracle::texture_sweep::{Point, Quality, SEED};
use transit_texture::{BufferSize, Controls, Media, Parameter, SeedButton, Texture};

/// The port's quality.
pub fn media(m: oracle::Media) -> Media {
    match m {
        oracle::Media::DirtyTape => Media::DirtyTape,
        oracle::Media::CleanTape => Media::CleanTape,
        oracle::Media::VintageDigital => Media::VintageDigital,
        oracle::Media::Digital => Media::Digital,
    }
}

fn parameter(p: &oracle::Parameter) -> Parameter {
    Parameter { base: p.base, attenuverter: p.attenuverter, cv: p.cv, has_cv: p.has_cv }
}

/// The port's controls.
pub fn controls(c: &oracle::Controls) -> Controls {
    Controls {
        time: parameter(&c.time),
        size: parameter(&c.size),
        shape: parameter(&c.shape),
        pitch: parameter(&c.pitch),
        density: c.density,
        density_cv: c.density_cv,
        feedback: c.feedback,
        dry_wet: c.dry_wet,
        reverb: c.reverb,
        seed_button: match c.seed_button {
            oracle::SeedButton::Off => SeedButton::Off,
            oracle::SeedButton::On => SeedButton::On,
            oracle::SeedButton::Locked => SeedButton::Locked,
        },
        monophonic: c.monophonic,
        output_gate: c.output_gate,
    }
}

/// Renders a sweep point through the port exactly as `texture_sweep::render_point` renders it
/// through the oracle: a fresh chain on the module's memory, seeded, in the module's blocks.
pub fn render_point(quality: Quality, point: &Point, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut texture = Texture::new(BufferSize::FIRMWARE, media(quality.media), quality.mono);
    texture.seed(SEED);
    let frames = point.frames(quality);
    let block = quality.media.block_size();
    let c = controls(&point.controls());
    let mut out = vec![[0.0_f32; 2]; frames];
    let mut freeze = vec![false; block];
    let mut seed = vec![0_u8; block];
    for (b, (i, o)) in input[..frames].chunks(block).zip(out.chunks_mut(block)).enumerate() {
        let start = b * block;
        for (k, f) in freeze.iter_mut().enumerate() {
            *f = point.freeze(quality.media, start + k, frames);
        }
        let seed = point.seed(quality.media, start, &mut seed).map(|()| &seed[..]);
        texture.process(&c, &freeze, seed, i, o);
    }
    out
}

/// The frames as one flat buffer.
pub fn flat(frames: &[[f32; 2]]) -> Vec<f32> {
    frames.iter().flatten().copied().collect()
}

/// The residual between two renders.
pub fn residual(a: &[[f32; 2]], b: &[[f32; 2]]) -> Residual {
    residual_f32(&flat(a), &flat(b)).expect("equal lengths")
}

/// Whether a render sounds: any sample of at least an int16 LSB (-90 dBFS) after `from` frames.
pub fn sounds(frames: &[[f32; 2]], from: usize) -> bool {
    frames.iter().skip(from).flatten().any(|s| s.abs() >= 1.0 / 32768.0)
}

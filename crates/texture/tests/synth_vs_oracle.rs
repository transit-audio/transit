//! The synth (what the module plays with nothing patched) against the oracle over its sweep, at
//! each resolution, bit for bit (#241).

#![forbid(unsafe_code)]

mod common;

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::texture::Media;
use transit_macro_osc_oracle::texture_sweep::{Point, SEED, SYNTH_POINT_FRAMES, render_synth_point, synth_points};
use transit_texture::{Synth, SynthResolution};

fn render_port(resolution: SynthResolution, point: &Point) -> Vec<[f32; 2]> {
    let mut synth = Synth::new();
    synth.seed(SEED);
    let block = Media::Digital.block_size();
    let c = common::controls(&point.controls());
    let mut out = vec![[0.0_f32; 2]; SYNTH_POINT_FRAMES];
    let mut freeze = vec![false; block];
    let mut seed = vec![0_u8; block];
    for (b, o) in out.chunks_mut(block).enumerate() {
        let start = b * block;
        for (k, f) in freeze.iter_mut().enumerate() {
            *f = point.freeze(Media::Digital, start + k, SYNTH_POINT_FRAMES);
        }
        let seed = point.seed(Media::Digital, start, &mut seed).map(|()| &seed[..]);
        synth.render(&c, resolution, &freeze, seed, o);
    }
    out
}

fn null_resolution(resolution: SynthResolution) {
    let mut failures = Vec::new();
    for point in synth_points() {
        let expected = {
            let session = Session::lock();
            render_synth_point(&session, resolution.index(), &point).unwrap()
        };
        let got = render_port(resolution, &point);
        let r = common::residual(&expected, &got);
        if !r.is_exact() {
            failures.push(format!("synth {resolution:?} {}: {r}", point.label()));
        }
        assert!(common::sounds(&expected, 24), "synth {resolution:?} {}: every synth point sounds", point.label());
    }
    assert!(failures.is_empty(), "{} of {} points differ:\n{}", failures.len(), synth_points().len(), failures.join("\n"));
}

#[test]
fn four_bits() {
    null_resolution(SynthResolution::Bits4);
}

#[test]
fn sixteen_bits_with_six_bit_grains() {
    null_resolution(SynthResolution::Bits16Grains6);
}

#[test]
fn seven_bits() {
    null_resolution(SynthResolution::Bits7);
}

#[test]
fn sixteen_bits() {
    null_resolution(SynthResolution::Bits16);
}

/// TIME at 1 with the last bank (FEEDBACK at 1): the oscillator and the grains read the last wave of
/// the table and interpolate toward the wave after it with a zero fraction, past the upstream
/// table, where the oracle's build and the port both read one wave of zeros.
#[test]
fn the_last_wave_of_the_last_bank() {
    let mut c = transit_macro_osc_oracle::texture_sweep::synth_center();
    c.time.base = 1.0;
    c.feedback = 1.0;
    c.density = 0.95;
    let point = Point { base: c, gate: transit_macro_osc_oracle::texture_sweep::Gate::Free };
    for resolution in SynthResolution::ALL {
        let expected = {
            let session = Session::lock();
            render_synth_point(&session, resolution.index(), &point).unwrap()
        };
        let got = render_port(resolution, &point);
        let r = common::residual(&expected, &got);
        assert!(r.is_exact(), "synth {resolution:?}: {r}");
        assert!(common::sounds(&expected, 24));
    }
}

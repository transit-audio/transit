//! A minute per quality against the oracle (#241): every control moving, freeze toggling, the
//! crossing into the delay mode and back, SEED pulses, held and latched, ten seconds of silence
//! (the tails decay into the subnormal range, where the flush to zero decides the bits), CVs on
//! every input, the monophonic mode and the gate output. Bit for bit, and the random generators
//! end in the same state. A failure is a defect to hunt, never a run to repeat.

#![forbid(unsafe_code)]

mod common;

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::texture::{Controls, Media, Parameter, SeedButton};
use transit_macro_osc_oracle::texture_sweep::{Quality, center, input_signal};
use transit_texture::{BufferSize, Texture};

const SECONDS: f32 = 60.0;
const SEED: u32 = 0x0241_0060;

/// A triangle from 0 to 1 and back with period `period` seconds.
fn tri(t: f32, period: f32) -> f32 {
    let x = (t / period).fract();
    if x < 0.5 { 2.0 * x } else { 2.0 - 2.0 * x }
}

/// The controls, freeze and the SEED gate at `t` seconds.
fn schedule(t: f32) -> (Controls, bool, bool) {
    let mut c = center();
    c.time.base = tri(t, 20.0);
    c.size.base = 0.1 + 0.8 * tri(t, 7.0);
    c.shape.base = tri(t, 5.0);
    c.pitch.base = [-24.0, -12.0, -5.0, 0.0, 7.0, 12.0, 24.0][((t / 2.5) as usize) % 7];
    c.density = tri(t, 13.0);
    c.time.attenuverter = 2.0 * tri(t, 11.0) - 1.0;
    c.size.attenuverter = 1.0 - 2.0 * tri(t, 17.0);
    c.shape.attenuverter = 2.0 * tri(t, 9.0) - 1.0;
    c.pitch.attenuverter = 0.5 - tri(t, 19.0);
    c.feedback = 0.9 * tri(t, 23.0);
    c.dry_wet = 0.5 + 0.5 * tri(t, 3.0);
    c.reverb = tri(t, 29.0);
    let mut freeze = (t % 6.0) >= 4.5;
    let mut seed = false;
    if (20.0..35.0).contains(&t) {
        // The delay mode (the crossing takes 1.5 s), with taps and slices.
        c.size.base = 1.0;
        if t >= 25.0 {
            seed = (t * 6.0).fract() < 0.05;
            c.seed_button = if t < 30.0 { SeedButton::Off } else { SeedButton::Locked };
        }
    } else if (35.0..40.0).contains(&t) {
        // Back to grains; DENSITY at noon, so only the held button's taps seed them.
        c.size.base = 0.6;
        c.density = 0.5;
        c.seed_button = if (t * 4.0).fract() < 0.5 { SeedButton::On } else { SeedButton::Off };
        freeze = false;
    } else if (40.0..50.0).contains(&t) {
        freeze = false;
        c.reverb = 0.8;
    } else if t >= 50.0 {
        for (p, cv) in [(&mut c.time, 1.6 * (2.0 * tri(t, 3.0) - 1.0)), (&mut c.size, 0.5 - tri(t, 4.0)), (&mut c.shape, tri(t, 2.0))] {
            p.cv = cv;
            p.has_cv = true;
        }
        c.pitch = Parameter { base: 0.0, attenuverter: 1.0 - 2.0 * tri(t, 5.0), cv: 60.0 * (2.0 * tri(t, 7.0) - 1.0), has_cv: true };
        c.density_cv = 2.0 * tri(t, 2.5) - 1.0;
        c.monophonic = t < 55.0;
        c.output_gate = t >= 58.0;
        seed = t >= 56.0 && (t * 8.0).fract() < 0.1;
    }
    (c, freeze, seed)
}

fn long_run(media: Media, mono: bool) {
    let quality = Quality { media, mono };
    let rate = media.sample_rate() as usize;
    let block = media.block_size();
    let frames = (SECONDS * rate as f32) as usize / block * block;
    let mut input = input_signal(frames);
    // Ten seconds of silence from 40 s.
    for f in &mut input[40 * rate..50 * rate] {
        *f = [0.0; 2];
    }
    let mut port = Texture::new(BufferSize::FIRMWARE, common::media(media), mono);
    port.seed(SEED);
    let session = Session::lock();
    session.seed(SEED);
    let mut oracle = session.texture(media, mono).unwrap();
    let mut expected = vec![[0.0_f32; 2]; frames];
    let mut got = vec![[0.0_f32; 2]; frames];
    let mut previous_gate = false;
    let mut flags = vec![0_u8; block];
    let mut freeze = vec![false; block];
    for (b, ((i, e), g)) in input.chunks(block).zip(expected.chunks_mut(block)).zip(got.chunks_mut(block)).enumerate() {
        let t = (b * block) as f32 / rate as f32;
        let (c, frozen, _) = schedule(t);
        for (k, f) in flags.iter_mut().enumerate() {
            let (_, _, gate) = schedule((b * block + k) as f32 / rate as f32);
            *f = transit_texture::stmlib::extract_gate_flags(u8::from(previous_gate), gate);
            previous_gate = gate;
        }
        let patched = (25.0..35.0).contains(&t) || t >= 56.0;
        freeze.fill(frozen);
        let seed = patched.then_some(&flags[..]);
        oracle.process(&c, &freeze, seed, i, e).unwrap();
        port.process(&common::controls(&c), &freeze, seed, i, g);
    }
    let r = common::residual(&expected, &got);
    assert!(r.is_exact(), "{} over {SECONDS} s: {r}", quality.label());
    assert!(common::sounds(&expected, 0), "{}: the run sounds", quality.label());
    assert_eq!(port.rng_state(), session.rng_state(), "{}: the random generators end in the same state", quality.label());
}

#[test]
fn digital_stereo_60_s() {
    long_run(Media::Digital, false);
}

#[test]
fn digital_mono_60_s() {
    long_run(Media::Digital, true);
}

#[test]
fn vintage_digital_stereo_60_s() {
    long_run(Media::VintageDigital, false);
}

#[test]
fn vintage_digital_mono_60_s() {
    long_run(Media::VintageDigital, true);
}

#[test]
fn clean_tape_stereo_60_s() {
    long_run(Media::CleanTape, false);
}

#[test]
fn clean_tape_mono_60_s() {
    long_run(Media::CleanTape, true);
}

#[test]
fn dirty_tape_stereo_60_s() {
    long_run(Media::DirtyTape, false);
}

#[test]
fn dirty_tape_mono_60_s() {
    long_run(Media::DirtyTape, true);
}

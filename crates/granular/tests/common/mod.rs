//! Helpers shared by the granular null tests: render the same block sequence through the port and
//! the oracle and compare the int16 output bit for bit.

#![forbid(unsafe_code)]
#![allow(dead_code)] // Each test binary uses part of it.

use transit_granular::{BufferSize, Granular, Parameters, Quality};
use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::granular::{Parameters as OracleParameters, Quality as OracleQuality};

/// One step of a scenario, applied to both sides in order.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// Process this many frames of the input with these parameters.
    Block(usize, OracleParameters),
    /// Change the quality.
    Quality(OracleQuality),
    /// Set bypass.
    Bypass(bool),
    /// Set silence.
    Silence(bool),
}

/// The port's quality for an oracle quality.
pub fn port_quality(q: OracleQuality) -> Quality {
    Quality::ALL[q.index()]
}

/// The port's parameters for the oracle's.
pub fn port_parameters(p: &OracleParameters) -> Parameters {
    Parameters {
        position: p.position,
        size: p.size,
        pitch: p.pitch,
        density: p.density,
        texture: p.texture,
        dry_wet: p.dry_wet,
        stereo_spread: p.stereo_spread,
        feedback: p.feedback,
        reverb: p.reverb,
        freeze: p.freeze,
        trigger: p.trigger,
        gate: p.gate,
    }
}

/// Runs `steps` over `input` through the oracle (on two blocks of `large` and `small` bytes) and
/// through the port (on the same sizes), from the same seed, and returns the first difference as
/// `(frame, port, oracle)`, or `None` when both outputs and the final random states are identical.
/// Panics if the random states differ while the outputs match.
pub fn compare(
    label: &str,
    large: usize,
    small: usize,
    quality: OracleQuality,
    seed: u32,
    input: &[[i16; 2]],
    steps: &[Step],
) -> Option<(usize, [i16; 2], [i16; 2])> {
    let consumed: usize = steps.iter().map(|s| if let Step::Block(n, _) = s { *n } else { 0 }).sum();
    assert!(consumed <= input.len(), "{label}: the steps consume {consumed} frames of {}", input.len());

    let (oracle, oracle_rng) = {
        let session = Session::lock();
        session.seed(seed);
        let mut o = session.granular(large, small, quality).unwrap();
        let mut out = vec![[0_i16; 2]; consumed];
        let mut at = 0;
        for step in steps {
            match *step {
                Step::Block(n, p) => {
                    o.process(&p, &input[at..at + n], &mut out[at..at + n]).unwrap();
                    at += n;
                }
                Step::Quality(q) => o.set_quality(q),
                Step::Bypass(b) => o.set_bypass(b),
                Step::Silence(s) => o.set_silence(s),
            }
        }
        (out, session.rng_state())
    };

    let buffer_size = BufferSize::new(large, small).expect("the port accepts every size the oracle does");
    let mut g = Granular::new(buffer_size, port_quality(quality));
    g.seed(seed);
    let mut port = vec![[0_i16; 2]; consumed];
    let mut at = 0;
    for step in steps {
        match *step {
            Step::Block(n, p) => {
                g.process(&port_parameters(&p), &input[at..at + n], &mut port[at..at + n]);
                at += n;
            }
            Step::Quality(q) => g.set_quality(port_quality(q)),
            Step::Bypass(b) => g.set_bypass(b),
            Step::Silence(s) => g.set_silence(s),
        }
    }

    if let Some(i) = port.iter().zip(&oracle).position(|(a, b)| a != b) {
        return Some((i, port[i], oracle[i]));
    }
    assert_eq!(g.rng_state(), oracle_rng, "{label}: identical output but the random state differs");
    // From the third frame block on: the first carries the dry input while dry/wet ramps from 0.
    assert!(oracle.iter().skip(64).any(|f| f[0].unsigned_abs() > 64), "{label}: the oracle rendered near-silence, so the comparison proves nothing");
    None
}

/// A triangle from 0 to 1 and back over `period` units, at `t`.
pub fn triangle(t: f32, period: f32) -> f32 {
    let phase = (t / period).fract();
    if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 }
}

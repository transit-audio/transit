//! Block-size independence at core level (Evidence "Block-size independence", decision 12): the same
//! input at block sizes 1, 7, 64 and 512 gives bit-identical output and end state (the fingerprint),
//! with band frequencies, gains and Qs set to new values after a reset at other values (so smoothers
//! move and designs recompute), discrete changes ramping, mid/side bands, and a second change at a
//! frame every block size shares. Negative control (recorded): redesigning at block starts instead of
//! on the sample clock differs at block size 7. In-process comparison.

use super::util::{band, noise};
use super::proc::{eq, fingerprint};
use crate::{BandParams, BandType, ParametricEq, Placement, Slope};

fn first(e: &mut ParametricEq) {
    e.set_band(0, band(BandType::Bell, 500.0, -8.0, 2.0));
    e.set_band(1, BandParams { place: Placement::Mid, ..band(BandType::LowShelf, 120.0, 6.0, 0.707) });
    e.set_band(2, BandParams { slope: Slope::Db48, ..band(BandType::HighCut, 15_000.0, 0.0, 1.0) });
    e.set_band(3, BandParams { place: Placement::Side, ..band(BandType::HighShelf, 7_000.0, 4.0, 3.0) });
    e.set_band(4, BandParams { place: Placement::Left, ..band(BandType::FlatTilt, 1_000.0, 5.0, 1.0) });
    e.set_band(5, band(BandType::Notch, 3_000.0, 0.0, 8.0));
}

fn second(e: &mut ParametricEq) {
    e.set_freq(0, 2_500.0);
    e.set_gain(0, 12.0);
    e.set_q(0, 0.4);
    e.set_freq(1, 300.0);
    e.set_gain(1, -9.0);
    e.set_kind(5, BandType::BandPass);
    e.set_place(4, Placement::Right);
    e.set_used(6, true);
    e.set_kind(6, BandType::TiltShelf);
    e.set_gain(6, -3.0);
    e.set_output_db(-2.0);
    e.set_gain_match(true);
}

fn third(e: &mut ParametricEq) {
    e.set_slope(2, Slope::Db12);
    e.set_on(1, false);
    e.set_q(3, 20.0);
    e.set_freq(3, 11_000.0);
}

/// Changes at frame 0 (after the reset) and at frame 3584, a multiple of 1, 7, 64 and 512.
fn render(block: usize) -> (Vec<Vec<f32>>, u64) {
    let mut e = eq(48_000.0, 2);
    first(&mut e);
    e.reset();
    second(&mut e);
    let mut io = noise(7_168, 2, 21);
    let mut at = 0;
    while at < 7_168 {
        if at == 3_584 {
            third(&mut e);
        }
        let n = block.min(7_168 - at);
        let mut views: Vec<&mut [f32]> = io.iter_mut().map(|c| &mut c[at..at + n]).collect();
        e.process(&mut views, n);
        at += n;
    }
    (io, fingerprint(&e))
}

#[test]
fn output_and_end_state_do_not_depend_on_the_block_size() {
    let (reference, fp) = render(512);
    for block in [1, 7, 64] {
        let (out, f) = render(block);
        for c in 0..2 {
            let diff = out[c].iter().zip(&reference[c]).position(|(a, b)| a.to_bits() != b.to_bits());
            assert_eq!(diff, None, "block {block}, channel {c}: first difference at frame {diff:?}");
        }
        assert_eq!(f, fp, "block {block}: end state differs");
    }
}

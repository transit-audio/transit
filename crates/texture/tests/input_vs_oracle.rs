//! The input stage against the oracle (#241): each codec rate with each soft-clipping mode, through
//! the automatic gain's wait and learning phase, a codec stall and a restart, manual gains and the
//! unpatched-input cases, bit for bit.

#![forbid(unsafe_code)]

mod common;

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::texture_sweep::{INPUT_RUNS, input_frames, input_step, render_input_run};
use transit_texture::{InputStage, SoftClipping};

#[test]
fn every_rate_and_mode_nulls() {
    for (rate, mode) in INPUT_RUNS {
        let expected = {
            let session = Session::lock();
            render_input_run(&session, rate, mode).unwrap()
        };
        let clipping = [SoftClipping::Off, SoftClipping::On, SoftClipping::Dynamic][mode];
        let mut stage = InputStage::new(rate, clipping);
        let block = rate as usize / 4000;
        let input = input_frames(rate);
        let mut got = vec![[0.0_f32; 2]; input.len()];
        for (b, (i, o)) in input.chunks(block).zip(got.chunks_mut(block)).enumerate() {
            let step = input_step(b);
            if step.reinit {
                stage.init(rate, clipping, false);
            }
            stage.process(step.active, step.gain, step.normalized, i, o);
        }
        let r = common::residual(&expected, &got);
        assert!(r.is_exact(), "input stage at {rate} Hz, {clipping:?}: {r}");
    }
}

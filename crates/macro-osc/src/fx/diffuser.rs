//! Granular diffuser.

// Based on MIT-licensed code (c) 2016 by Emilie Gillet (emilie.o.gillet@gmail.com)

use super::{Format12Bit, FxEngine, Lfo, Line, lines};
use crate::utils::ftz;

// ap1, ap2, ap3, ap4, dapa, dapb, del.
const LINES: [Line; 7] = lines([126, 180, 269, 444, 1653, 2010, 3411]);

#[derive(Debug, Default, Clone)]
pub struct Diffuser {
    engine: FxEngine<8192, Format12Bit>,
    lp_decay: f32,

    sample_rate_hz: f32,
}

impl Diffuser {
    pub fn new() -> Self {
        Self { engine: FxEngine::new(), lp_decay: 0.0, sample_rate_hz: 48000.0 }
    }

    pub fn init(&mut self, sample_rate_hz: f32) {
        self.sample_rate_hz = sample_rate_hz;
        self.engine.set_lfo_frequency(Lfo::One, 0.3 / sample_rate_hz);
        self.lp_decay = 0.0;
    }

    pub fn reset(&mut self) {
        self.clear();
    }

    pub fn clear(&mut self) {
        self.engine.clear();
    }

    #[inline]
    pub fn process(&mut self, amount: f32, rt: f32, in_out: &mut [f32]) {
        let [ap1, ap2, ap3, ap4, dapa, dapb, del] = LINES;
        let e = &mut self.engine;
        let kap = 0.625;
        let klp = 0.75;
        let mut lp = self.lp_decay;

        for in_out_sample in in_out.iter_mut() {
            let mut wet = 0.0;
            e.start();
            e.read_value(*in_out_sample);
            e.read_line(ap1, Line::TAIL, kap);
            e.write_all_pass(ap1, 0, -kap);
            e.read_line(ap2, Line::TAIL, kap);
            e.write_all_pass(ap2, 0, -kap);
            e.read_line(ap3, Line::TAIL, kap);
            e.write_all_pass(ap3, 0, -kap);
            e.interpolate_lfo(ap4, 400.0, Lfo::One, 43.0, kap);
            e.write_all_pass(ap4, 0, -kap);
            e.interpolate_lfo(del, 3070.0, Lfo::One, 340.0, rt);
            e.lp(&mut lp, klp);
            e.read_line(dapa, Line::TAIL, -kap);
            e.write_all_pass(dapa, 0, kap);
            e.read_line(dapb, Line::TAIL, kap);
            e.write_all_pass(dapb, 0, -kap);
            e.write_line(del, 0, 2.0);
            e.write(&mut wet, 0.0);
            *in_out_sample = ftz(*in_out_sample + ftz(amount * ftz(wet - *in_out_sample)));
        }

        self.lp_decay = lp;
    }
}

#[cfg(test)]
mod tests {
    use super::Diffuser;

    const BLOCK_SIZE: usize = 24;
    const RESET_PROBE_BLOCKS: usize = 400;

    #[test]
    fn reset_clears_delay_lines() {
        let mut fx = Diffuser::new();
        fx.init(48_000.0);

        let mut in_out = [1.0; BLOCK_SIZE];
        fx.process(1.0, 0.5, &mut in_out);

        // Clearing the delay lines must not clear the persistent LP or LFO state,
        // so use a clone with an explicit clear.
        let mut expected = fx.clone();
        fx.reset();
        expected.clear();

        for block in 0..RESET_PROBE_BLOCKS {
            in_out.fill(0.0);
            let mut expected_out = in_out;
            fx.process(1.0, 0.5, &mut in_out);
            expected.process(1.0, 0.5, &mut expected_out);
            assert_eq!(in_out, expected_out, "diffuser reset did not clear its delay lines in probe block {block}");
        }
    }
}

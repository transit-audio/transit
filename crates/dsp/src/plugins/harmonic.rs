//! Harmonic effects: Saturator (tanh).

use crate::params::{Params, lin, log, param_plumbing};
use crate::util::{DcBlocker, Smoother, flush, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

static SAT_PARAMS: [ParamInfo; 5] = [
    lin("drive", "Drive", 0.0, 36.0, 6.0, Unit::Db),
    lin("bias", "Asymmetry", 0.0, 100.0, 0.0, Unit::Percent),
    log("tone", "Tone", 1000.0, 20000.0, 20000.0, Unit::Hz),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static SAT_INFO: PluginInfo = PluginInfo {
    id: "saturator",
    name: "Saturator",
    short_name: "Satur",
    category: Category::Harmonic,
    params: &SAT_PARAMS,
    is_instrument: false,
    audiosuite: true,
    pages: &[],
};

/// Smooth tanh waveshaper, normalized so a full-scale input still peaks near full scale (more
/// drive = more harmonics and density). Asymmetry adds even harmonics (the DC is removed).
pub struct Saturator {
    p: Params,
    sr: f32,
    dc: Vec<DcBlocker>,
    lp: Vec<f32>,
    drive: Smoother,
    bias: Smoother,
    mix: Smoother,
    out: Smoother,
    tone: f32,
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

impl Saturator {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SAT_PARAMS),
            sr: 48_000.0,
            dc: Vec::new(),
            lp: Vec::new(),
            drive: Smoother::new(1.0),
            bias: Smoother::new(0.0),
            mix: Smoother::new(1.0),
            out: Smoother::new(1.0),
            tone: 0.0,
        };
        s.update();
        s
    }

    fn update(&mut self) {
        self.drive.set(db_to_gain(self.p.v(0)));
        self.bias.set(self.p.v(1) / 100.0 * 0.5);
        self.mix.set(self.p.v(3) / 100.0);
        self.out.set(db_to_gain(self.p.v(4)));
        let f = self.p.v(2);
        self.tone = if f >= 19_999.0 { 0.0 } else { (-std::f32::consts::TAU * f / self.sr).exp() };
    }
}

impl Plugin for Saturator {
    param_plumbing!(SAT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.dc = vec![DcBlocker::new(10.0, self.sr); ch];
        self.lp = vec![0.0; ch];
        for s in [&mut self.drive, &mut self.bias, &mut self.mix, &mut self.out] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.dc.iter_mut().for_each(DcBlocker::reset);
        self.lp.iter_mut().for_each(|v| *v = 0.0);
        for s in [&mut self.drive, &mut self.bias, &mut self.mix, &mut self.out] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.dc.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let a = self.tone;
        for n in 0..frames {
            let g = self.drive.next();
            let b = self.bias.next();
            let m = self.mix.next();
            let o = self.out.next();
            let tb = b.tanh();
            let norm = 1.0 / ((g + b).tanh() - tb).max(1e-3);
            for c in 0..ch {
                let (Some(buf), Some(dc), Some(lp)) = (io.get_mut(c), self.dc.get_mut(c), self.lp.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                let dry = if x.is_finite() { *x } else { 0.0 };
                let mut y = ((g * dry + b).tanh() - tb) * norm;
                if b > 0.0 {
                    y = dc.process(y);
                }
                if a > 0.0 {
                    *lp = flush(y + (*lp - y) * a);
                    y = *lp;
                }
                *x = (dry * (1.0 - m) + y * m) * o;
            }
        }
    }
}

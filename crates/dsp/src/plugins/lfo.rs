//! Modulation sources the built-in plug-ins share: the LFO and its tempo-synced divisions (moved
//! from the Filter, #242).

/// The LFO's shapes, in the order of the `lfo_shape` parameter's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Sine,
    Triangle,
    Saw,
    Square,
    Random,
}

impl Shape {
    pub(crate) fn from_index(i: usize) -> Shape {
        match i {
            1 => Shape::Triangle,
            2 => Shape::Saw,
            3 => Shape::Square,
            4 => Shape::Random,
            _ => Shape::Sine,
        }
    }
}

/// The tempo-synced divisions, in the order of the `lfo_division` parameter's choices.
pub(crate) const DIVISIONS: [&str; 14] =
    ["4 bars", "2 bars", "1 bar", "1/2", "1/4", "1/8", "1/16", "1/32", "1/4 T", "1/8 T", "1/16 T", "1/4 D", "1/8 D", "1/16 D"];

/// One cycle of division `i` in quarter notes; the bar divisions use the meter (`bar` quarter notes).
pub(crate) fn division_quarters(i: usize, bar: f64) -> f64 {
    match i {
        0 => 4.0 * bar,
        1 => 2.0 * bar,
        2 => bar,
        3 => 2.0,
        5 => 0.5,
        6 => 0.25,
        7 => 0.125,
        8 => 2.0 / 3.0,
        9 => 1.0 / 3.0,
        10 => 1.0 / 6.0,
        11 => 1.5,
        12 => 0.75,
        13 => 0.375,
        _ => 1.0,
    }
}

/// A free-running LFO: phase in cycles, advanced per sample; its value is read only when the
/// filter's coefficients are worked out. The phase restarts on a note when its retrigger is on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lfo {
    phase: f64,
    /// The random shape's current step, drawn at each cycle start.
    step: f32,
    rng: u32,
}

const SEED: u32 = 0x2545_F491;

impl Lfo {
    pub(crate) fn new() -> Lfo {
        let mut l = Lfo { phase: 0.0, step: 0.0, rng: SEED };
        l.step = l.draw();
        l
    }

    /// xorshift32: deterministic, so an export plays the same every time.
    fn draw(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }

    /// Advances one sample by `inc` cycles.
    #[inline]
    pub(crate) fn advance(&mut self, inc: f64) {
        self.phase += inc;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.step = self.draw();
        }
    }

    pub(crate) fn retrigger(&mut self) {
        self.phase = 0.0;
        self.step = self.draw();
    }

    /// The value now, -1..1.
    pub(crate) fn value(&self, shape: Shape) -> f32 {
        let p = self.phase as f32;
        match shape {
            Shape::Sine => (std::f32::consts::TAU * p).sin(),
            Shape::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            Shape::Saw => 2.0 * p - 1.0,
            Shape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Shape::Random => self.step,
        }
    }

    #[cfg(feature = "testing")]
    pub(crate) fn bits(&self) -> (u64, u32, u32) {
        (self.phase.to_bits(), self.step.to_bits(), self.rng)
    }
}

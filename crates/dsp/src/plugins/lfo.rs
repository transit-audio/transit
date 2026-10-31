//! Modulation sources the built-in plug-ins share: the LFO and its tempo-synced divisions (moved
//! from the Filter, #242).

/// The LFO's shapes, in the order of the `lfo_shape` parameter's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Sine,
    Triangle,
    Saw,
    Square,
    /// Sample and hold: a new random step at each cycle start.
    Random,
    /// The random steps joined by a smooth (cubic) glide across each cycle (#243).
    SmoothRandom,
}

impl Shape {
    pub(crate) fn from_index(i: usize) -> Shape {
        match i {
            1 => Shape::Triangle,
            2 => Shape::Saw,
            3 => Shape::Square,
            4 => Shape::Random,
            5 => Shape::SmoothRandom,
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
    /// The step before it: where the smooth random shape glides from.
    prev: f32,
    rng: u32,
}

const SEED: u32 = 0x2545_F491;

impl Lfo {
    pub(crate) fn new() -> Lfo {
        Lfo::seeded(SEED)
    }

    /// An LFO whose random shapes draw from `seed` (non-zero; 0 takes the default seed): each of
    /// the Macro Oscillator's voices has its own.
    pub(crate) fn seeded(seed: u32) -> Lfo {
        let mut l = Lfo { phase: 0.0, step: 0.0, prev: 0.0, rng: if seed == 0 { SEED } else { seed } };
        l.step = l.draw();
        l.prev = l.step;
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
            self.next_step();
        }
    }

    fn next_step(&mut self) {
        self.prev = self.step;
        self.step = self.draw();
    }

    pub(crate) fn retrigger(&mut self) {
        self.phase = 0.0;
        self.next_step();
    }

    /// Sets the phase (in cycles; the fraction is kept), as a tempo-synced LFO anchored to the song
    /// position does at each block (#245's rule, #243): a phase that went backwards (a new cycle,
    /// or a jump) starts a new random step.
    pub(crate) fn set_phase(&mut self, phase: f64) {
        let p = if phase.is_finite() { phase - phase.floor() } else { 0.0 };
        if p < self.phase {
            self.next_step();
        }
        self.phase = p;
    }

    /// The phase now, 0..1 cycles.
    #[cfg(test)]
    pub(crate) fn phase(&self) -> f64 {
        self.phase
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
            Shape::SmoothRandom => {
                let t = p.clamp(0.0, 1.0);
                self.prev + (self.step - self.prev) * (t * t * (3.0 - 2.0 * t))
            }
        }
    }

    #[cfg(feature = "testing")]
    pub(crate) fn bits(&self) -> (u64, u32, u32) {
        (self.phase.to_bits(), self.step.to_bits(), self.rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smooth random shape starts each cycle where the last one ended (no step), reaches the
    /// new step at the cycle's end, and stays within -1..1.
    #[test]
    fn smooth_random_glides_between_the_steps_without_jumps() {
        let mut l = Lfo::seeded(7);
        let mut last = l.value(Shape::SmoothRandom);
        for _ in 0..50_000 {
            l.advance(0.0011);
            let v = l.value(Shape::SmoothRandom);
            assert!((-1.0..=1.0).contains(&v), "{v}");
            assert!((v - last).abs() < 0.01, "a jump from {last} to {v}");
            last = v;
        }
    }

    /// Setting the phase keeps the fraction; a phase that wraps starts a new step, one that moves on
    /// within the cycle keeps it. `new` (the Filter's) is the default-seeded LFO.
    #[test]
    fn set_phase_wraps_and_draws_a_step_only_on_a_new_cycle() {
        let mut l = Lfo::new();
        let first = l.value(Shape::Random);
        l.set_phase(0.25);
        assert_eq!(l.value(Shape::Random).to_bits(), first.to_bits());
        l.set_phase(3.75);
        assert_eq!(l.phase(), 0.75);
        assert_eq!(l.value(Shape::Random).to_bits(), first.to_bits());
        l.set_phase(4.1);
        assert_ne!(l.value(Shape::Random).to_bits(), first.to_bits(), "a new cycle, a new step");
        let mut a = Lfo::new();
        let mut b = Lfo::seeded(SEED);
        for _ in 0..1000 {
            a.advance(0.01);
            b.advance(0.01);
            assert_eq!(a.value(Shape::Random).to_bits(), b.value(Shape::Random).to_bits());
        }
    }
}

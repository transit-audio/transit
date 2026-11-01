//! The tube (`elements/dsp/tube.{h,cc}`): a reed-and-bore waveguide the blow exciter's breath drives
//! once the blow level passes two thirds, added onto the blow signal.

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::integral_fractional;

/// `kTubeDelaySize`.
const DELAY_SIZE: usize = 2048;

/// `elements::Tube`. The voice's `Init` never calls the tube's `Init` (which would clear the two
/// filter states and the write pointer), so all of it starts at zero, the module's static storage,
/// and runs on from there; the port has no `init` for that reason.
#[derive(Clone, Debug)]
pub(crate) struct Tube {
    delay_ptr: i32,
    zero_state: F,
    pole_state: F,
    /// The bore. `Init` never clears it: it starts at zero (the module's static storage) and keeps
    /// its contents across re-initializations.
    delay_line: Box<[F]>,
}

impl Default for Tube {
    fn default() -> Self {
        Self { delay_ptr: 0, zero_state: F::ZERO, pole_state: F::ZERO, delay_line: vec![F::ZERO; DELAY_SIZE].into_boxed_slice() }
    }
}

impl Tube {
    /// This tube as [`Default`] builds it, moving its delay line out without allocating: `self` is
    /// left only fit to be dropped or replaced.
    #[must_use]
    pub(crate) fn take_zeroed(&mut self) -> Self {
        let mut delay_line = std::mem::take(&mut self.delay_line);
        delay_line.fill(F::ZERO);
        Self { delay_ptr: 0, zero_state: F::ZERO, pole_state: F::ZERO, delay_line }
    }

    /// `Process(frequency, envelope, damping, timbre, input_output, gain, size)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process(&mut self, frequency: F, envelope: F, damping: F, timbre: F, input_output: &mut [F], gain: F) {
        let mut delay = 1.0 / frequency;
        while delay >= DELAY_SIZE as f32 {
            delay *= 0.5;
        }
        let (delay_integral, delay_fractional) = integral_fractional(delay);
        let envelope = if envelope >= 1.0 { F(1.0) } else { envelope };
        let damping = 3.6 - damping * 1.8;
        let mut lpf_coefficient = frequency * (1.0 + timbre * timbre * 256.0);
        if lpf_coefficient >= 0.995 {
            lpf_coefficient = F(0.995);
        }

        let integral = usize::try_from(delay_integral).unwrap_or(0);
        let mut d = self.delay_ptr;
        for x in input_output.iter_mut() {
            let breath = *x * damping + 0.8;
            let at = usize::try_from(d).unwrap_or(0) + integral;
            let a = self.delay_line[at % DELAY_SIZE];
            let b = self.delay_line[(at + 1) % DELAY_SIZE];
            let input = a + (b - a) * delay_fractional;
            let pressure_delta = -0.95 * (input * envelope + self.zero_state) - breath;
            self.zero_state = input;

            let reed = pressure_delta * -0.2 + 0.8;
            let mut out = pressure_delta * reed + breath;
            out = if out < -5.0 {
                F(-5.0)
            } else if out > 5.0 {
                F(5.0)
            } else {
                out
            };
            self.delay_line[usize::try_from(d).unwrap_or(0)] = out * 0.5;

            d -= 1;
            if d < 0 {
                d = DELAY_SIZE as i32 - 1;
            }
            self.pole_state += lpf_coefficient * (out - self.pole_state);
            *x += gain * envelope * self.pole_state;
        }
        self.delay_ptr = d;
    }
}

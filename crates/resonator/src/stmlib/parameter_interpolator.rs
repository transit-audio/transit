//! `stmlib/dsp/parameter_interpolator.h`: a parameter ramped linearly over a block.

#![forbid(unsafe_code)]

use crate::float::F;

/// `stmlib::ParameterInterpolator(state, new_value, size)`. The C++ destructor writes the last
/// value back to `*state`; here the owner does it with [`ParameterInterpolator::value`] when the
/// block ends, whether or not every step was taken.
#[derive(Clone, Copy, Debug)]
pub struct ParameterInterpolator {
    value: F,
    increment: F,
}

impl ParameterInterpolator {
    /// Ramps from `state` to `new_value` over `size` steps.
    #[inline(always)]
    #[must_use]
    pub fn new(state: F, new_value: F, size: usize) -> Self {
        Self { value: state, increment: (new_value - state) / F::from_usize(size) }
    }

    /// `Next()`.
    #[inline(always)]
    #[allow(clippy::should_implement_trait)] // The firmware's name; not an iterator.
    pub fn next(&mut self) -> F {
        self.value += self.increment;
        self.value
    }

    /// The value the destructor writes back.
    #[inline(always)]
    #[must_use]
    pub fn value(&self) -> F {
        self.value
    }
}

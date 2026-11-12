//! The cuts are Butterworth (decision 2.1: "a cut is −3 dB at f"): every cut's analog prototype is
//! −10·log10(2) dB (half power, about −3.0103) at its band frequency, low and high cut at every slope
//! (the prototype to 1e-9 dB), and the matched digital design is within 0.1 dB of that at 100 Hz and
//! at fs/40, at 44.1, 48, 96 and 192 kHz (well below Nyquist, where decision 2.2's table puts the
//! matched cuts within hundredths of a decibel). Negative control
//! (recorded in the result note): the record's even-order formula for the 18 dB/oct section (Q 0.5774
//! instead of 1) puts both 18 dB/oct cuts at −7.78 dB. The prototype check is a closed form; the
//! digital check an in-process comparison with a tolerance.

use super::sweep::Case;
use super::util::{RATES, db};
use crate::{BandType, Slope};

/// −10·log10(2), correctly rounded (the exact half-power target, not a rounded −3.0103).
const HALF_POWER_DB: f64 = -3.010_299_956_639_812;

#[test]
fn every_cut_is_half_power_at_its_band_frequency() {
    let mut failures = Vec::new();
    for fs in RATES {
        for f0 in [100.0, fs / 40.0] {
            for kind in [BandType::LowCut, BandType::HighCut] {
                for slope in Slope::ALL {
                    let case = Case { kind, slope, f0, gain: 0.0, q: 1.0 };
                    let used = case.used(fs);
                    let analog = db(case.prototype(fs).power(used.f0));
                    if (analog - HALF_POWER_DB).abs() > 1e-9 {
                        failures.push(format!("{kind:?} {slope:?} prototype at {f0} Hz: {analog:.4} dB"));
                    }
                    let h = std::f64::consts::PI * used.f0 / fs;
                    let digital = db(case.design(fs).power(h.sin(), h.cos()));
                    if (digital - HALF_POWER_DB).abs() > 0.1 {
                        failures.push(format!("{kind:?} {slope:?} design at {f0} Hz, {fs} Hz: {digital:.4} dB"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

//! The panel's preview render (#326, 326-W): every model renders, the length and the kind follow
//! the group, the render is deterministic, the knobs reach it, and a cycle starts on a rising zero
//! crossing.

use super::MODELS;
use super::preview::{BURST, INSTANCE_SEED, Kind, Request, SETTLE, period, render, rising_crossing, stream};

const VIRTUAL_ANALOG: usize = 8;
const SIX_OP_FM_1: usize = 2;

fn req(engine: usize) -> Request {
    Request { engine, note: 60.0, harmonics: 0.5, timbre: 0.5, morph: 0.5, decay: 0.5, lpg_color: 0.5, seed: INSTANCE_SEED }
}

fn rendered(r: &Request) -> (Kind, Vec<f32>) {
    let mut out = Vec::new();
    let k = render(r, &mut out);
    (k, out)
}

#[test]
fn every_model_renders_finite_samples_of_its_documented_length() {
    assert_eq!(period(60.0), 183, "48000 / 261.63 Hz, rounded");
    assert_eq!(period(69.0), 109, "48000 / 440 Hz, rounded");
    for (engine, name) in MODELS.iter().enumerate() {
        let (kind, out) = rendered(&req(engine));
        let (want_kind, want_len) = if engine < 16 { (Kind::Cycle, 183) } else { (Kind::Burst, BURST) };
        assert_eq!(kind, want_kind, "{name}");
        assert_eq!(out.len(), want_len, "{name}");
        assert!(out.iter().all(|x| x.is_finite()), "{name}: finite");
        assert!(out.iter().any(|x| *x != 0.0), "{name}: not silent");
    }
}

#[test]
fn the_burst_is_fifty_milliseconds() {
    assert_eq!(BURST, 2400);
    for (engine, name) in MODELS.iter().enumerate().skip(16) {
        assert_eq!(rendered(&req(engine)).1.len(), 2400, "{name}");
    }
    // Negative control: a pitched model's render is one period, not a burst.
    assert_ne!(rendered(&req(VIRTUAL_ANALOG)).1.len(), 2400);
}

#[test]
fn the_same_request_renders_the_same_bits_and_timbre_reaches_the_render() {
    for engine in [VIRTUAL_ANALOG, SIX_OP_FM_1, 17, 22] {
        let (a, b) = (rendered(&req(engine)).1, rendered(&req(engine)).1);
        assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()) && a.len() == b.len(), "{}: bit-identical", MODELS[engine]);
    }
    let at = |timbre: f32| rendered(&Request { timbre, ..req(SIX_OP_FM_1) }).1;
    assert_ne!(at(0.2), at(0.8), "Six-op FM 1: Timbre 0.2 and 0.8 differ");
    // Negative control: identical requests do not differ.
    assert_eq!(at(0.2), at(0.2));
}

#[test]
fn a_cycle_starts_at_a_rising_zero_crossing() {
    let r = req(VIRTUAL_ANALOG);
    let p = period(60.0);
    let raw = stream(&r, VIRTUAL_ANALOG, 60.0, SETTLE + 3 * p);
    let start = (SETTLE..SETTLE + 2 * p).find(|&i| raw[i - 1] < 0.0 && raw[i] >= 0.0).expect("Virtual analog crosses zero rising");
    assert!(raw[start - 1] < 0.0 && raw[start] >= 0.0);
    assert_eq!(rendered(&r).1, raw[start..start + p], "the cycle is the period from the first rising crossing after the settle");
    // Negative control: the period from the settle's end (no crossing search) is a different cut.
    assert_ne!(start, SETTLE, "the settle does not end on a crossing here");
    assert_ne!(rendered(&r).1, raw[SETTLE..SETTLE + p]);
    assert_eq!(rising_crossing(&[0.5, -0.2, -0.1, 0.0, 0.3], 0, 5), Some(3));
    // Negative controls of the rule: a falling crossing and an all-positive run have none.
    assert_eq!(rising_crossing(&[0.3, 0.0, -0.3], 0, 3), None);
    assert_eq!(rising_crossing(&[0.1; 8], 0, 8), None);
}

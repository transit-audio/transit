//! The instrument-wide Output (#753): its table entry and default, the gain applied to the summed
//! zones, its glide on a change, and its clamping.

use super::*;
use crate::{SampleBuffer, SampleSource};
use std::sync::Arc;
use transit_sampler_core::patch::{PlayMode, Playback, Zone};
use transit_sampler_core::playhead::Region;

const SR: f32 = 48_000.0;

fn constant(value: f32, frames: usize) -> Arc<dyn SampleSource> {
    Arc::new(SampleBuffer { sample_rate: SR, channels: vec![vec![value; frames]] })
}

/// A gate zone on note `note` with no envelope (the source plays as it is while held).
fn gate(source: u16, frames: u64, note: u8) -> Zone {
    let playback = Playback {
        mode: PlayMode::Gate,
        key_tracking: false,
        root: note,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: 0.0,
        declick_ms: 0.0,
        ..Playback::pad(note)
    };
    Zone { playback, ..Zone::new(Region::whole(source, frames)) }
}

/// A stereo core-path instance with Output at `output` dB (`None`: untouched, the default), two
/// held zones of constant 0.25 and 0.125 on pads 0 and 1 (notes 60 and 61), both sounding.
fn sounding(output: Option<f32>) -> Sampler {
    let mut s = Sampler::new();
    if let Some(db) = output {
        assert!(s.set_param(OUTPUT_PARAM, db));
    }
    s.prepare(SR, 512, 2);
    let mut p = s.enable_core(3);
    let mut spec = PatchSpec {
        sources: vec![constant(0.25, 96_000), constant(0.125, 96_000)],
        zones: vec![Some(gate(0, 96_000, 60)), Some(gate(1, 96_000, 61))],
        ..PatchSpec::default()
    };
    spec.pads[0].zone = Some(0);
    spec.pads[1].zone = Some(1);
    p.publish(spec).unwrap();
    let _ = block(&mut s, 4);
    s.note_on(0, 60, 127);
    s.note_on(0, 61, 127);
    s
}

fn block(s: &mut Sampler, frames: usize) -> [Vec<f32>; 2] {
    let mut io = vec![vec![f32::NAN; frames]; 2];
    s.process(&mut io, frames);
    let [l, r]: [Vec<f32>; 2] = io.try_into().unwrap_or_default();
    [l, r]
}

#[test]
fn output_is_the_one_table_parameter_and_defaults_to_minus_12_db() {
    let info = SAMPLER_INFO.param(OUTPUT_PARAM).expect("Output is in the table");
    assert_eq!((info.name, info.min, info.max, info.default, info.unit), ("Output", -48.0, 12.0, -12.0, Unit::Db));
    assert_eq!(Sampler::new().param(OUTPUT_PARAM), Some(-12.0), "a new instance starts at the default");
    assert_eq!(resolve(OUTPUT_PARAM), None, "not a slot id");
}

#[test]
fn output_scales_the_summed_zones_after_they_are_summed() {
    let mut unity = sounding(Some(0.0));
    let mut default = sounding(None);
    let [ul, ur] = block(&mut unity, 512);
    let [dl, dr] = block(&mut default, 512);
    // At 0 dB the gain is exactly 1: the zones' own sum, 0.25 + 0.125, untouched.
    assert!(ul.iter().chain(&ur).all(|x| x.to_bits() == 0.375f32.to_bits()), "{:?}", &ul[..4]);
    // At the default every frame of the sum is scaled by the same -12 dB gain.
    let g = db_to_gain(-12.0);
    for (u, d) in ul.iter().chain(&ur).zip(dl.iter().chain(&dr)) {
        assert_eq!(d.to_bits(), (u * g).to_bits(), "{d} != {u} x {g}");
    }
}

#[test]
fn an_output_change_glides_without_a_step() {
    let mut s = sounding(Some(0.0));
    let _ = block(&mut s, 512);
    assert!(s.set_param(OUTPUT_PARAM, -48.0));
    let [l, _] = block(&mut s, 24_000);
    let target = 0.375 * db_to_gain(-48.0);
    // The first frame after the change moves by one 20 ms one-pole step, not to the target.
    assert!(l[0] > 0.37, "no jump: {}", l[0]);
    assert!(l.windows(2).all(|w| w[1] <= w[0]), "falls monotonically");
    let largest = l.windows(2).map(|w| w[0] - w[1]).fold(0.0f32, f32::max);
    assert!(largest < 0.375 * 0.002, "no frame steps by more than a 20 ms glide does: {largest}");
    assert!((l[23_999] - target).abs() < 1.0e-6, "settled at -48 dB: {} vs {target}", l[23_999]);
    assert_eq!(s.param(OUTPUT_PARAM), Some(-48.0));
}

#[test]
fn output_clamps_and_a_nan_takes_the_default() {
    let mut s = Sampler::new();
    assert!(s.set_param(OUTPUT_PARAM, 40.0));
    assert_eq!(s.param(OUTPUT_PARAM), Some(12.0));
    assert!(s.set_param(OUTPUT_PARAM, -200.0));
    assert_eq!(s.param(OUTPUT_PARAM), Some(-48.0));
    assert!(s.set_param(OUTPUT_PARAM, f32::NAN));
    assert_eq!(s.param(OUTPUT_PARAM), Some(-12.0));
}

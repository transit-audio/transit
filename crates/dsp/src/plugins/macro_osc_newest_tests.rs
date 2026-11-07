//! The newest voice the instrument publishes for the panel's well (#326, 326-W): its key and the
//! Harmonics, Timbre and Morph its patch carried that block, after the matrix; none when no key
//! holds a voice.

use super::matrix::{dst, src};
use super::voice::targets;
use super::*;

fn inst(voices: usize, params: &[(&str, f32)]) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    m.set_param("voices", (voices - 1) as f32);
    for (k, v) in params {
        assert!(m.set_param(k, *v), "{k}");
    }
    m.prepare(48_000.0, 512, 2);
    m
}

fn run(m: &mut MacroOscillator, frames: usize) {
    let mut io = vec![vec![0.0; frames]; 2];
    m.process(&mut io, frames);
}

#[test]
fn after_a_note_on_and_one_block_the_newest_voice_is_published() {
    let mut m = inst(4, &[("model", 8.0), ("harmonics", 20.0), ("timbre", 30.0), ("morph", 70.0)]);
    run(&mut m, BLOCK);
    assert_eq!(m.newest_voice(), None, "no key holds a voice");
    m.note_on(0, 64, 100);
    run(&mut m, BLOCK);
    let v = m.newest_voice().expect("a voice sounds");
    assert_eq!(v.note, 64);
    // The patch's values: the knobs through the matrix's rule (no row: the knob exactly).
    let [h, t, mo, _] = targets([0.2, 0.3, 0.7, 0.5], None, [0.0; 4]);
    assert_eq!((v.harmonics, v.timbre, v.morph), (h, t, mo));
    assert_eq!(v.timbre, 0.3f32);
    // A second note is the newest.
    m.note_on(0, 67, 100);
    run(&mut m, BLOCK);
    assert_eq!(m.newest_voice().map(|v| v.note), Some(67));
    // Released: none, though the tails still play.
    m.note_off(0, 64);
    m.note_off(0, 67);
    run(&mut m, BLOCK);
    assert_ne!(m.voice_activity() & 0xFFFF, 0, "the tails play");
    assert_eq!(m.newest_voice(), None, "no key holds a voice");
}

#[test]
fn an_lfo_route_to_timbre_moves_the_published_timbre_off_the_knob() {
    let published = |route: bool| {
        let mut m = inst(1, &[("model", 8.0), ("timbre", 30.0)]);
        if route {
            for (f, v) in [("source", src::LFO1 as f32), ("dest", dst::TIMBRE as f32), ("amount", 50.0)] {
                assert!(m.set_param(&format!("mod1_{f}"), v));
            }
        }
        m.note_on(0, 60, 100);
        run(&mut m, 4800);
        m.newest_voice().expect("a voice sounds").timbre
    };
    assert_ne!(published(true), 0.3, "the LFO moves the patch's Timbre");
    // Negative control: with no route the published Timbre is the knob.
    assert_eq!(published(false), 0.3);
}

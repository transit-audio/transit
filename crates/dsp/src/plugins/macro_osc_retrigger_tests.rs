//! #263: a same-key re-strike on the six-op FM models. The instrument turns on the port's
//! same-pitch restart, so the re-struck note restarts the internal voice already sounding instead
//! of starting on the engine's other internal voice under the old note's release (where the two
//! summed at the same pitch and cancelled the fundamental for up to 90 ms). Block-driven and
//! deterministic: the measures are pitch tracks and sample steps of fixed renders.

use super::*;
use crate::event::{Message, PluginEvent};

const SR: f32 = 48_000.0;
const KEY: u8 = 60;
/// "Six-op FM 1": bank A, whose patch at the default Harmonics (16, a brass pad) holds its
/// modulators at release level 99, so the old note's release stays almost as loud as the note.
const SIX_OP_1: f32 = 2.0;

fn inst(voices: usize, params: &[(&str, f32)]) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    m.set_param("voices", (voices - 1) as f32);
    for (k, v) in params {
        assert!(m.set_param(k, *v), "{k}");
    }
    m.prepare(SR, 512, 2);
    m
}

/// Renders `frames` frames in host blocks of `block`: the left channel.
fn render(m: &mut MacroOscillator, frames: usize, block: usize) -> Vec<f32> {
    let mut io = vec![vec![0.0; block]; 2];
    let mut l = Vec::new();
    while l.len() < frames {
        let n = block.min(frames - l.len());
        m.process(&mut io, n);
        l.extend_from_slice(&io[0][..n]);
    }
    l
}

/// Goertzel magnitude of `x` (Hann window) at `f` Hz, at 48 kHz.
fn goertzel(x: &[f32], f: f64) -> f64 {
    let n = x.len() as f64;
    let w = 2.0 * std::f64::consts::PI * f / f64::from(SR);
    let (c, mut s1, mut s2) = (2.0 * w.cos(), 0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
        let s0 = f64::from(*v) * win + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt()
}

/// The pitch track of the 300 ms from `at` (the method of the #263 hunt): one 2048-frame window
/// per 10 ms, the strongest of the first eight partials of `KEY` (1 is the fundamental) and the
/// fundamental's pitch in cents (the strongest bin within 100 cents, in 1-cent steps).
fn track(x: &[f32], at: usize) -> Vec<(usize, i32)> {
    let f0 = 440.0 * 2f64.powf((f64::from(KEY) - 69.0) / 12.0);
    let cents = |c: i32| f0 * 2f64.powf(f64::from(c) / 1200.0);
    (0..30)
        .map(|k| {
            let w = &x[at + k * 480..at + k * 480 + 2048];
            let h = (1..=8usize).max_by(|a, b| goertzel(w, f0 * *a as f64).total_cmp(&goertzel(w, f0 * *b as f64))).unwrap_or(1);
            let c = (-100..=100).max_by(|a, b| goertzel(w, cents(*a)).total_cmp(&goertzel(w, cents(*b)))).unwrap_or(0);
            (h, c)
        })
        .collect()
}

/// Windows led by an overtone, and the fundamental's worst pitch error in cents.
fn grade(t: &[(usize, i32)]) -> (usize, i32) {
    (t.iter().filter(|(h, _)| *h != 1).count(), t.iter().map(|(_, c)| c.abs()).max().unwrap_or(0))
}

/// The largest second difference of `x`: a step or a kink in the waveform.
fn largest_corner(x: &[f32]) -> f32 {
    x.windows(3).fold(0.0f32, |m, w| m.max((w[2] - 2.0 * w[1] + w[0]).abs()))
}

/// The level envelope of `x`: the RMS of a 192-frame window (one period of key 60 is 183.5
/// frames) at every 12-frame block.
fn level(x: &[f32]) -> Vec<f32> {
    (0..x.len().saturating_sub(192) / BLOCK).map(|b| (x[b * BLOCK..b * BLOCK + 192].iter().map(|v| v * v).sum::<f32>() / 192.0).sqrt()).collect()
}

/// A note of key 60 held for 0.5 s from frame 0, released, and struck again `gap_ms` later (the
/// off and the on in one host block at the same offset for 0 ms), in host blocks of 240 frames.
/// The left channel and the frame of the second note-on.
fn restrike(voices: usize, gap_ms: usize, pedal: bool) -> (Vec<f32>, usize) {
    let mut m = inst(voices, &[("model", SIX_OP_1)]);
    if pedal {
        m.midi(0, [0xB0, 64, 127]);
    }
    m.note_on(0, KEY, 100);
    let mut x = render(&mut m, 24_000, 240);
    let gap = gap_ms * 48;
    m.note_off(0, KEY);
    if gap > 0 {
        x.extend(render(&mut m, gap, 240));
    }
    let at = x.len();
    m.note_on(0, KEY, 100);
    x.extend(render(&mut m, 24_000, 240));
    (x, at)
}

/// The first note's own track: what every re-strike should keep (the patch's vibrato).
fn first_note() -> (usize, i32) {
    let (x, _) = restrike(1, 0, false);
    grade(&track(&x, 0))
}

/// Every re-strike gap keeps the first note's pitch: the fundamental leads every window and stays
/// within the patch's own vibrato (50 cents, as lap 1 of the hunt), mono and with 8 voices.
#[test]
fn a_same_key_restrike_keeps_the_fundamental_at_every_gap() {
    let (lead0, worst0) = first_note();
    assert_eq!(lead0, 0, "the first note is led by its fundamental");
    for voices in [1, 8] {
        for gap in [0, 10, 50, 100, 250] {
            let (x, at) = restrike(voices, gap, false);
            let t = track(&x, at);
            let (lead, worst) = grade(&t);
            println!("voices {voices} gap {gap} ms: overtones lead {lead}/30, worst {worst} cents (first note {worst0})");
            assert!(lead == 0 && worst <= 50, "voices {voices}, gap {gap} ms: overtones lead {lead}/30, worst {worst} cents: {t:?}");
        }
    }
}

/// A re-strike under the sustain pedal. Mono, the key the pedal keeps makes the new note legato:
/// no edge, so the note carries on exactly as if it had been held throughout. With 8 voices the
/// key's voice takes a new phrase and restarts, and the pitch holds.
#[test]
fn a_restrike_under_the_pedal_keeps_the_fundamental() {
    let (pedalled, _) = restrike(1, 0, true);
    let mut m = inst(1, &[("model", SIX_OP_1)]);
    m.note_on(0, KEY, 100);
    let held = render(&mut m, pedalled.len(), 240);
    assert!(pedalled == held, "mono under the pedal: the note carries on as if held");
    let (x, at) = restrike(8, 0, true);
    let t = track(&x, at);
    let (lead, worst) = grade(&t);
    println!("pedal, voices 8: overtones lead {lead}/30, worst {worst} cents");
    assert!(lead == 0 && worst <= 50, "pedal, voices 8: overtones lead {lead}/30, worst {worst} cents: {t:?}");
}

/// The restart is clean. A click is a corner in the waveform: the largest second difference in
/// the 5 ms after the re-strike (the trigger delay, the restart and its 1 ms phase crossfade all
/// land in it) passes neither the held note's nor that of the 50 ms of attack that follow. Without
/// the crossfade the operators' phase reset makes a corner about nine times the held note's. And no window
/// of the level envelope drops more than 12 dB under the held note's lowest level for longer than
/// one block: the envelopes restart from where they are, and the allocator's one low trigger block
/// does not reach the sound, since the six-op models bypass the low-pass gate.
#[test]
fn a_zero_gap_restrike_has_no_click_and_no_dropout() {
    for voices in [1, 8] {
        let (x, at) = restrike(voices, 0, false);
        let held = &x[at - 4800..at];
        let reference = largest_corner(held).max(largest_corner(&x[at + 240..at + 2640]));
        let corner = largest_corner(&x[at - 2..at + 242]);
        let pre_level = level(held).iter().copied().fold(f32::MAX, f32::min);
        let low: Vec<bool> = level(&x[at..at + 2592]).iter().map(|r| *r < pre_level * 0.25).collect();
        let longest = low.split(|l| !*l).map(<[bool]>::len).max().unwrap_or(0);
        println!("voices {voices}: largest corner {corner} (held and attack {reference}), level {pre_level}, longest dip {longest} blocks");
        assert!(corner <= reference, "voices {voices}: a corner of {corner} at the re-strike (reference {reference})");
        assert!(longest <= 1, "voices {voices}: the level drops for {longest} blocks");
    }
}

/// Different keys are not re-strikes: a note a semitone away starts on the other internal voice,
/// as the firmware does, so the old note's release still sounds under the new one in mono.
#[test]
fn a_different_key_still_switches_voices() {
    let play = |restart: bool| {
        let mut m = inst(1, &[("model", SIX_OP_1)]);
        m.slots[0].voice.set_same_pitch_restart(restart);
        m.note_on(0, KEY, 100);
        let mut x = render(&mut m, 24_000, 240);
        m.note_off(0, KEY);
        m.note_on(0, KEY + 1, 100);
        x.extend(render(&mut m, 24_000, 240));
        x
    };
    assert!(play(true) == play(false), "a new key renders as the firmware does");
}

fn ev(message: Message) -> PluginEvent {
    PluginEvent { channel: 0, note_id: -1, message }
}

/// A held bend is part of the pitch both strikes see, so a re-strike under it restarts: it renders
/// differently from the firmware's switch and keeps the fundamental.
#[test]
fn a_restrike_under_a_held_bend_restarts() {
    let play = |restart: bool| {
        let mut m = inst(1, &[("model", SIX_OP_1)]);
        m.slots[0].voice.set_same_pitch_restart(restart);
        m.event(0, &ev(Message::PitchBend { value: 12_000 }));
        m.note_on(0, KEY, 100);
        let mut x = render(&mut m, 24_000, 240);
        m.note_off(0, KEY);
        m.note_on(0, KEY, 100);
        x.extend(render(&mut m, 24_000, 240));
        x
    };
    assert!(play(true) != play(false), "the held bend does not stop the restart");
}

/// The restart and the crossfade that covers its phase reset share one condition: a same-pitch
/// re-strike in the same render as a tuning change restarts with the crossfade, so the phase reset
/// makes no step above the click bound (the held note's and the attack's own corner).
#[test]
fn a_restrike_in_the_render_of_a_tuning_change_has_no_click() {
    let play = |restart: bool| {
        let mut m = inst(1, &[("model", SIX_OP_1)]);
        m.slots[0].voice.set_same_pitch_restart(restart);
        m.note_on(0, KEY, 100);
        let mut x = render(&mut m, 24_000, 240);
        m.note_off(0, KEY);
        let at = x.len();
        m.slots[0].voice.set_tuning(transit_macro_osc::Tuning::Firmware);
        m.note_on(0, KEY, 100);
        x.extend(render(&mut m, 24_000, 240));
        (x, at)
    };
    let ((x, at), (off, _)) = (play(true), play(false));
    assert!(x != off, "the restart engages in the render of the tuning change");
    let held = &x[at - 4800..at];
    let reference = largest_corner(held).max(largest_corner(&x[at + 240..at + 2640]));
    let corner = largest_corner(&x[at - 2..at + 242]);
    println!("largest corner {corner} (held and attack {reference})");
    assert!(corner <= reference, "a corner of {corner} at the re-strike (reference {reference})");
}

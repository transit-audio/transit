//! The Macro Oscillator's voices (#196): the allocator, the release tails, the sum of independent
//! voices, the note-to-macro mapping. Block-driven and deterministic: events land at 12-frame
//! boundaries and nothing reads a clock.

use super::voice::{Mapping, Shared, shape, targets};
use super::*;

const SR: f32 = 48_000.0;

/// An instance of `voices` voices at 48 kHz.
fn poly(voices: usize) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    m.set_param("voices", (voices - 1) as f32);
    m.prepare(SR, 512, 2);
    m
}

/// Renders `frames` frames in 12-frame host blocks; the left channel.
fn run(m: &mut MacroOscillator, frames: usize) -> Vec<f32> {
    let mut io = vec![vec![0.0; BLOCK]; 2];
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let n = BLOCK.min(frames - out.len());
        m.process(&mut io, n);
        out.extend_from_slice(&io[0][..n]);
    }
    out
}

/// The voice holding `key`.
fn voice_of(m: &MacroOscillator, key: u8) -> Option<usize> {
    m.slots.iter().position(|s| s.notes.contains(key))
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn the_default_is_eight_voices_and_the_range_one_to_sixteen() {
    let p = MACRO_OSC_INFO.param("voices").unwrap();
    assert_eq!(p.format(p.default), "8");
    assert_eq!((p.format(p.min), p.format(p.max)), ("1".to_string(), "16".to_string()));
    let mut m = MacroOscillator::new();
    m.prepare(SR, 512, 2);
    assert_eq!(m.voices, 8);
    m.set_param("voices", 99.0);
    assert_eq!(m.voices, MAX_VOICES);
    m.set_param("voices", -3.0);
    assert_eq!(m.voices, 1);
}

/// Free voices first (lowest first), the same key back on its own voice, then the quietest voice
/// in its release tail, then the oldest held voice: the newest note always sounds.
#[test]
fn voices_steal_the_quietest_releasing_voice_then_the_oldest() {
    let mut m = poly(4);
    for key in [60, 62, 64, 65] {
        m.note_on(0, key, 100);
        run(&mut m, 24);
    }
    assert_eq!([60, 62, 64, 65].map(|k| voice_of(&m, k)), [Some(0), Some(1), Some(2), Some(3)], "free voices, lowest first");
    run(&mut m, 4800);
    // 62 released long before 65: its tail is quieter when the next note comes.
    m.note_off(0, 62);
    run(&mut m, 2400);
    m.note_off(0, 65);
    run(&mut m, 240);
    assert!(!m.slots[1].idle && !m.slots[3].idle, "both tails still play");
    assert!(m.slots[1].loudness < m.slots[3].loudness, "62's tail is the quieter: {} vs {}", m.slots[1].loudness, m.slots[3].loudness);
    m.note_on(0, 67, 100);
    run(&mut m, 24);
    assert_eq!(voice_of(&m, 67), Some(1), "the quietest releasing voice");
    // The same key comes back on its own voice, not on another.
    m.note_on(0, 64, 80);
    run(&mut m, 24);
    assert_eq!(voice_of(&m, 64), Some(2));
    m.note_on(0, 69, 100);
    run(&mut m, 24);
    assert_eq!(voice_of(&m, 69), Some(3), "the only releasing voice");
    // Every voice held: the oldest phrase (60, voice 0) gives way.
    m.note_on(0, 71, 100);
    run(&mut m, 24);
    assert_eq!(voice_of(&m, 71), Some(0), "the oldest held voice");
    assert_eq!(voice_of(&m, 60), None, "the stolen key is gone");
    m.note_off(0, 60);
    run(&mut m, 24);
    assert!(m.slots[0].sounding(), "a stolen key's late note-off leaves the new note alone");
    let held = m.activity() >> 16;
    assert_eq!(held, 0b1111, "four keys held");
}

/// Among equally quiet releasing voices, the oldest phrase goes first.
#[test]
fn equally_quiet_releasing_voices_give_way_oldest_first() {
    let mut m = poly(3);
    m.set_param("model", 21.0);
    for key in [40, 41, 42] {
        m.note_on(0, key, 100);
        run(&mut m, 12);
    }
    for key in [40, 41, 42] {
        m.note_off(0, key);
    }
    // Hold every voice in its tail at the same loudness: equal by construction.
    for s in &mut m.slots {
        s.loudness = 0.25;
    }
    m.note_on(0, 50, 100);
    run(&mut m, 12);
    assert_eq!(voice_of(&m, 50), Some(0));
}

/// A note-off leaves the voice's low-pass gate tail playing; the voice is free only once it has
/// been silent for `QUIET_SECONDS`, and then costs nothing.
#[test]
fn a_released_voice_plays_its_tail_then_goes_idle() {
    let mut m = poly(8);
    m.note_on(0, 57, 120);
    let held = run(&mut m, 9600);
    assert!(peak(&held) > 0.05);
    assert_eq!(m.activity(), 1 | 1 << 16, "voice 0 sounds, held");
    m.note_off(0, 57);
    let tail = run(&mut m, 480);
    assert!(peak(&tail) > 0.01 * peak(&held), "the tail continues after the note-off");
    assert_eq!(m.activity(), 1, "sounding, no longer held");
    let tail_len = m.tail_samples();
    let rest = run(&mut m, tail_len);
    assert_eq!(peak(&rest[rest.len() - 4800..]), 0.0);
    assert!(m.slots[0].idle && !m.slots[0].rendered, "silent for long enough: idle and not rendered");
    assert_eq!(m.activity(), 0);
    m.note_on(0, 60, 100);
    run(&mut m, 12);
    assert_eq!(voice_of(&m, 60), Some(0), "an idle voice is free again");
}

/// Lowering `voices` releases the voices above the new count; they play out and take no new note.
#[test]
fn lowering_the_voice_count_releases_the_voices_above_it() {
    let mut m = poly(8);
    for key in [60, 64, 67, 71] {
        m.note_on(0, key, 100);
    }
    run(&mut m, 120);
    assert_eq!(m.activity() >> 16, 0b1111);
    m.set_param("voices", 1.0);
    run(&mut m, 12);
    assert_eq!(m.activity() >> 16, 0b11, "voices 2 and 3 released");
    assert!(!m.slots[2].idle, "and still in their tails");
    m.note_on(0, 72, 100);
    run(&mut m, 12);
    assert!(voice_of(&m, 72).is_some_and(|v| v < 2), "a new note takes one of the two voices");
}

/// One instance of N voices plays, bit for bit, the sum of N mono instances (each seeded as its
/// voice) given the notes the allocator gave each voice; a stolen voice's instance gets the stolen
/// key's note-off first. The determinism claim: the voices are independent and the sum is in voice
/// order, so an export plays the same bits every time.
#[test]
fn n_voices_render_the_sum_of_n_mono_renders_bit_for_bit() {
    const N: usize = 4;
    // (12-frame block, key, velocity: 0 is a note-off), with chords, a held steal and a releasing steal.
    let script: &[(usize, u8, u8)] = &[
        (0, 48, 100),
        (0, 55, 70),
        (2, 60, 127),
        (40, 64, 40),
        (90, 55, 0),
        (120, 67, 90),
        (130, 72, 110),
        (200, 48, 0),
        (210, 60, 0),
        (260, 76, 100),
        (300, 64, 0),
        (300, 79, 64),
        (420, 67, 0),
        (420, 72, 0),
        (420, 76, 0),
        (420, 79, 0),
    ];
    for (model, harmonics) in [(0.0, 50.0), (18.0, 50.0), (15.0, 80.0)] {
        let params = [
            ("model", model),
            ("harmonics", harmonics),
            ("velocity_timbre", 60.0),
            ("velocity_curve", 2.0),
            ("key_morph", -40.0),
            ("key_curve", 1.0),
            ("velocity_decay", 25.0),
        ];
        let mut m = poly(N);
        for (id, v) in params {
            m.set_param(id, v);
        }
        let blocks = 1200;
        let mut out = Vec::new();
        let mut per_voice: Vec<Vec<(usize, Kind)>> = vec![Vec::new(); N];
        for b in 0..blocks {
            let held: Vec<Option<u8>> = m.slots.iter().take(N).map(|s| s.notes.top()).collect();
            let now: Vec<_> = script.iter().filter(|(at, ..)| *at == b).collect();
            for &&(_, key, vel) in &now {
                m.push(0, if vel == 0 { Kind::Off(key) } else { Kind::On(key, vel) });
            }
            out.extend(run(&mut m, BLOCK));
            for &&(_, key, vel) in &now {
                if vel == 0 {
                    if let Some(v) = held.iter().position(|h| *h == Some(key)) {
                        per_voice[v].push((b, Kind::Off(key)));
                    }
                    continue;
                }
                let v = voice_of(&m, key).expect("the newest note sounds");
                if let Some(old) = held[v] {
                    per_voice[v].push((b, Kind::Off(old)));
                }
                per_voice[v].push((b, Kind::On(key, vel)));
            }
        }
        assert!(per_voice.iter().filter(|e| !e.is_empty()).count() == N, "every voice played: {per_voice:?}");
        let mut sum = vec![0.0f32; out.len()];
        for (v, events) in per_voice.iter().enumerate() {
            let mut mono = poly(1);
            for (id, x) in params {
                mono.set_param(id, x);
            }
            mono.set_seed(Random::DEFAULT_SEED + v as u32);
            for b in 0..blocks {
                for (_, kind) in events.iter().filter(|(at, _)| *at == b) {
                    mono.push(0, *kind);
                }
                for (s, y) in sum[b * BLOCK..].iter_mut().zip(run(&mut mono, BLOCK)) {
                    *s += y;
                }
            }
        }
        assert!(peak(&out) > 0.01, "model {model} sounds");
        let first = out.iter().zip(&sum).position(|(a, b)| a.to_bits() != b.to_bits());
        assert!(first.is_none(), "model {model}: the voices' sum differs from frame {first:?}");
    }
}

/// Any host block size gives the output of 12-frame host blocks bit for bit with several voices.
#[test]
fn polyphonic_host_blocks_split_bit_for_bit() {
    let events: [(usize, Kind); 9] = [
        (5, Kind::On(60, 100)),
        (6, Kind::On(64, 90)),
        (7, Kind::On(67, 80)),
        (700, Kind::On(71, 90)),
        (1013, Kind::Bend(0.5)),
        (1500, Kind::Off(64)),
        (2049, Kind::On(72, 50)),
        (3001, Kind::Off(60)),
        (6000, Kind::AllOff),
    ];
    let render = |block: usize| {
        let mut m = poly(3);
        m.set_param("key_timbre", 50.0);
        let mut io = vec![vec![0.0; block]; 2];
        let mut out = Vec::new();
        let total = 9600;
        while out.len() < total {
            let start = out.len();
            let n = block.min(total - start);
            for (at, kind) in events.iter().filter(|(at, _)| (start..start + n).contains(at)) {
                assert!(m.push(at - start, *kind));
            }
            m.process(&mut io, n);
            out.extend(io[0][..n].iter().map(|v| v.to_bits()));
        }
        out
    };
    let reference = render(BLOCK);
    assert!(reference.iter().any(|b| *b != 0));
    for block in [1, 7, 13, 64, 250, 512] {
        assert!(render(block) == reference, "host blocks of {block} frames differ");
    }
}

/// Every curve maps 0 to 0 and 1 to 1 exactly; out-of-range and NaN sources clamp.
#[test]
fn mapping_curves_hold_their_extremes() {
    for c in 0..voice::CURVES.len() {
        assert_eq!(shape(c, 0.0), 0.0, "curve {c} at 0");
        assert_eq!(shape(c, 1.0), 1.0, "curve {c} at 1");
        assert_eq!(shape(c, -2.0), 0.0);
        assert_eq!(shape(c, 7.0), 1.0);
        assert_eq!(shape(c, f32::NAN), 0.0);
    }
    assert_eq!(shape(0, 0.5), 0.5);
    for (c, fade) in [(0, crate::offline::FadeShape::Linear), (1, crate::offline::FadeShape::Exponential), (2, crate::offline::FadeShape::SCurve)] {
        for t in [0.1, 0.25, 0.5, 0.9] {
            assert_eq!(shape(c, t), fade.gain(t), "curve {c} is the clip fade's {fade:?}");
        }
    }
    assert!((shape(1, 0.5) - 0.1192).abs() < 1e-4, "exponential: (e^4t - 1) / (e^4 - 1)");
    assert!((shape(2, 0.5) - 0.5).abs() < 1e-7, "the S-curve's midpoint");
    assert!(shape(2, 0.1) < 0.1 && shape(2, 0.9) > 0.9, "the S-curve is flat at both ends");
    assert_eq!(voice::key_source(60.0), 0.0);
    assert_eq!((voice::key_source(0.0), voice::key_source(120.0), voice::key_source(127.0)), (-1.0, 1.0, 1.0));
}

/// Full depths drive each target to the ends of its travel and clamp there; a negative depth
/// inverts; no mapping leaves every knob exactly as set.
#[test]
fn mapping_depths_reach_and_clamp_at_the_ends_of_the_travel() {
    let base = Shared {
        knobs: [0.5, 0.5, 0.5, 0.5],
        wheel: None,
        offset: 0.0,
        engine: 8,
        lpg_color: 0.5,
        aux_mix: 0.0,
        mapping: Mapping::default(),
        quiet_blocks: 200,
    };
    for (note, vel) in [(0.0, 0.0), (60.0, 0.5), (127.0, 1.0)] {
        assert_eq!(targets(&base, note, vel), base.knobs, "no mapping: the knobs exactly");
    }
    for curve in 0..3 {
        let mut s = base;
        s.mapping = Mapping { velocity_curve: curve, velocity: [1.0, -1.0, 0.5, 0.0], key_curve: curve, key: [0.0; 4] };
        assert_eq!(targets(&s, 60.0, 1.0), [1.0, 0.0, 1.0, 0.5], "curve {curve}: velocity 127 at full depth, clamped");
        assert_eq!(targets(&s, 60.0, 0.0), base.knobs, "velocity 0 adds nothing");
        s.mapping = Mapping { velocity_curve: 0, velocity: [0.0; 4], key_curve: curve, key: [0.0, 1.0, -0.25, 1.0] };
        assert_eq!(targets(&s, 120.0, 1.0), [0.5, 1.0, 0.25, 1.0], "curve {curve}: the top of the keyboard");
        assert_eq!(targets(&s, 0.0, 1.0), [0.5, 0.0, 0.75, 0.0], "curve {curve}: the bottom, mirrored");
        assert_eq!(targets(&s, 60.0, 1.0), base.knobs, "middle C is the center");
    }
    // The wheel adds after the mapping, capped at 1.
    let mut s = base;
    s.mapping.velocity = [0.0, 0.3, 0.0, 0.0];
    s.wheel = Some((1, 0.4));
    let t = targets(&s, 60.0, 1.0);
    assert!((t[1] - 1.0).abs() < 1e-6 && t[1] <= 1.0, "{t:?}");
}

/// Key tracking of Timbre (the "Key Tracked Timbre" factory preset): middle C plays exactly as
/// without it; keys away from it change the sound.
#[test]
fn key_tracking_leaves_middle_c_alone() {
    let play = |key: u8, depth: f32| {
        let mut m = poly(8);
        m.set_param("model", 10.0);
        m.set_param("key_timbre", depth);
        m.note_on(0, key, 100);
        run(&mut m, 4800)
    };
    assert!(play(60, 50.0) == play(60, 0.0), "middle C: bit-equal");
    assert!(play(84, 50.0) != play(84, 0.0), "two octaves up: brighter");
}

/// Voice `i` starts from the instance's seed plus `i`: the first voice of an instance plays a noise
/// model as a mono instance does, and a voice seeded one higher plays other noise.
#[test]
fn each_voice_has_its_own_random_state() {
    let play = |voices: usize, seed: Option<u32>| {
        let mut m = poly(voices);
        m.set_param("model", 17.0);
        if let Some(seed) = seed {
            m.set_seed(seed);
        }
        m.note_on(0, 60, 100);
        run(&mut m, 4800)
    };
    let first = play(8, None);
    assert!(first == play(1, None), "voice 0 is seeded as a mono instance");
    assert!(first != play(1, Some(Random::DEFAULT_SEED + 1)), "voice 1's seed plays other noise");
}

/// Every model's released voice goes idle within `tail_samples` (decay and color at their slowest),
/// so "a free voice costs nothing" holds for all 24: none keeps rendering forever.
#[test]
fn every_model_releases_to_an_idle_voice_within_the_tail() {
    let mut stuck = Vec::new();
    for (model, name) in MODELS.iter().enumerate() {
        let mut m = poly(2);
        m.set_param("model", model as f32);
        m.set_param("decay", 100.0);
        m.set_param("lpg_color", 100.0);
        m.note_on(0, 48, 127);
        run(&mut m, 4800);
        m.note_off(0, 48);
        let tail_len = m.tail_samples();
        let mut io = vec![vec![0.0; 512]; 2];
        let mut frames = 0;
        while frames < tail_len && !m.slots[0].idle {
            m.process(&mut io, 512);
            frames += 512;
        }
        println!("{name}: idle after {:.2} s", frames as f32 / SR);
        if !m.slots[0].idle {
            stuck.push(*name);
        }
    }
    assert!(stuck.is_empty(), "never idle: {stuck:?}");
}

/// The chiptune model holds while its key is held (its own envelope off, as on the module) and
/// decays after the note-off instead of sounding for ever.
#[test]
fn the_chiptune_model_holds_while_held_and_ends_after_the_note_off() {
    let mut m = poly(1);
    m.set_param("model", voice::CHIPTUNE as f32);
    m.note_on(0, 48, 127);
    let held = run(&mut m, 96_000);
    assert!(peak(&held[90_000..]) > 0.1, "still at level after 2 s held");
    m.note_off(0, 48);
    let tail_len = m.tail_samples();
    let tail = run(&mut m, tail_len);
    assert_eq!(peak(&tail[tail.len() - 4800..]), 0.0, "silent within the tail");
    assert!(m.slots[0].idle);
}

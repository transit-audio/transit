//! The transport-locked sequencer: the same notes at any block size, ticks on the beat grid
//! whatever the tempo, a latched seed that replays a path, and every note ended.

#![forbid(unsafe_code)]

use transit_random_sequencer::{Block, Event, EventKind, Model, RATES, Sequencer, Settings, Voices, division};

const SR: f64 = 48_000.0;

/// A transport path: piecewise-constant tempo changes at given samples, from beat 0.
struct Path {
    /// (start sample, tempo) in order; the first starts at 0.
    tempos: Vec<(i64, f64)>,
}

impl Path {
    fn constant(tempo: f64) -> Self {
        Self { tempos: vec![(0, tempo)] }
    }

    /// The beat at `sample` and the tempo there.
    fn at(&self, sample: i64) -> (f64, f64) {
        let mut beat = 0.0;
        for (i, &(start, tempo)) in self.tempos.iter().enumerate() {
            let end = self.tempos.get(i + 1).map_or(i64::MAX, |t| t.0);
            if sample < end {
                return (beat + (sample - start) as f64 * tempo / 60.0 / SR, tempo);
            }
            beat += (end - start) as f64 * tempo / 60.0 / SR;
        }
        (beat, 120.0)
    }
}

/// Plays `path` from `from` for `frames` samples in blocks of `block`, returning (absolute sample,
/// event) pairs. Tempo changes fall on block boundaries when they are multiples of `block`.
fn play(seq: &mut Sequencer, s: &Settings, path: &Path, from: i64, frames: i64, block: u32) -> Vec<(i64, EventKind)> {
    let mut out = Vec::new();
    let mut at = from;
    while at < from + frames {
        let n = u32::try_from((from + frames - at).min(i64::from(block))).unwrap();
        let (quarters, tempo) = path.at(at);
        let b = Block { playing: true, samples: at, quarters, tempo, sample_rate: SR, frames: n };
        seq.process(s, &b, &mut |e: Event| {
            assert!(e.offset < n, "offset {} in a block of {n}", e.offset);
            out.push((at + i64::from(e.offset), e.kind));
        });
        at += i64::from(n);
    }
    out
}

fn stop(seq: &mut Sequencer, s: &Settings) -> Vec<EventKind> {
    let mut out = Vec::new();
    let b = Block { playing: false, samples: 0, quarters: 0.0, tempo: 120.0, sample_rate: SR, frames: 64 };
    seq.process(s, &b, &mut |e: Event| out.push(e.kind));
    out
}

fn ons(events: &[(i64, EventKind)]) -> Vec<i64> {
    events.iter().filter(|(_, k)| matches!(k, EventKind::NoteOn { .. })).map(|(t, _)| *t).collect()
}

fn settings() -> Settings {
    Settings { voices: Voices::All, jitter: 0.0, deja_vu: 0.2, ..Settings::default() }
}

#[test]
fn the_same_seed_and_path_give_the_same_bits_at_64_and_512() {
    for s in [
        settings(),
        Settings { model: Model::Clusters, t_bias: 0.2, ..settings() },
        Settings { model: Model::Divider, t_bias: 0.9, gate_random: 0.7, ..settings() },
        Settings { model: Model::Drums, jitter: 0.6, ..settings() },
        Settings { model: Model::ThreeStates, rate: 12, spread: 0.9, steps: 0.9, ..settings() },
    ] {
        let path = Path::constant(133.0);
        let a = play(&mut Sequencer::new(7, true), &s, &path, 0, 48_000 * 20, 64);
        let b = play(&mut Sequencer::new(7, true), &s, &path, 0, 48_000 * 20, 512);
        let c = play(&mut Sequencer::new(7, true), &s, &path, 0, 48_000 * 20, 37);
        assert!(a.len() > 40, "{s:?}: {} events", a.len());
        assert_eq!(a, b, "{s:?}");
        assert_eq!(a, c, "{s:?}");
    }
}

#[test]
fn ticks_fall_on_the_grid_without_jitter() {
    // 120 BPM at 48 kHz: a sixteenth is 6000 samples; the clock voice starts a note every tick.
    let s = Settings { voices: Voices::Clock, rate: 16, ..settings() };
    assert_eq!(RATES[16].0, "1/16");
    let ev = play(&mut Sequencer::new(1, true), &s, &Path::constant(120.0), 0, 48_000 * 4, 256);
    let on = ons(&ev);
    assert_eq!(on.len(), 32);
    for (k, t) in on.iter().enumerate() {
        assert_eq!(*t, k as i64 * 6000, "tick {k}");
    }
    // Each clock note lasts half a tick.
    for (t, kind) in &ev {
        if let EventKind::NoteOff { .. } = kind {
            assert_eq!(t % 6000, 3000, "an off at {t}");
        }
    }
}

#[test]
fn a_tempo_change_mid_run_keeps_the_grid() {
    // 120 BPM for 2 s (4 beats), then 90 BPM: ticks stay on sixteenths of the beat.
    let s = Settings { voices: Voices::Clock, rate: 16, ..settings() };
    let path = Path { tempos: vec![(0, 120.0), (96_000, 90.0)] };
    for block in [64_u32, 500] {
        let ev = play(&mut Sequencer::new(1, true), &s, &path, 0, 48_000 * 6, block);
        for t in ons(&ev) {
            let (beat, _) = path.at(t);
            let (before, _) = path.at(t - 1);
            // The sample is the first at or after a sixteenth.
            let k = (beat * 4.0 - 1e-6).ceil();
            assert!(beat * 4.0 >= k - 1e-6 && before * 4.0 < k + 1e-9, "block {block}: a tick at {t} (beat {beat})");
        }
        // 16 ticks in the first 4 beats, then 4 beats at 90 BPM take 2.667 s: 16 more by 4.667 s.
        assert!(ons(&ev).len() >= 16 + 16 + 4, "block {block}: {} ticks", ons(&ev).len());
    }
}

#[test]
fn jitter_moves_ticks_and_keeps_the_count() {
    let s = Settings { voices: Voices::Clock, rate: 16, jitter: 0.8, ..settings() };
    let ev = play(&mut Sequencer::new(3, true), &s, &Path::constant(120.0), 0, 48_000 * 30, 128);
    let on = ons(&ev);
    let off_grid = on.iter().filter(|t| *t % 6000 != 0).count();
    assert!(off_grid > on.len() / 2, "{off_grid} of {} off the grid", on.len());
    // The jittery clock is pulled back to the straight one: about as many ticks as the grid's 240.
    assert!((220..=260).contains(&on.len()), "{} ticks", on.len());
}

#[test]
fn a_latched_seed_replays_a_locate_and_an_unlatched_one_moves_on() {
    let s = settings();
    let path = Path::constant(120.0);
    let mut latched = Sequencer::new(11, true);
    let first = play(&mut latched, &s, &path, 0, 96_000, 128);
    let _ = stop(&mut latched, &s);
    let again = play(&mut latched, &s, &path, 0, 96_000, 128);
    assert_eq!(first, again, "the latched seed plays the same notes from the same place");
    let mut free = Sequencer::new(11, false);
    let one = play(&mut free, &s, &path, 0, 96_000, 128);
    assert_eq!(one, first, "the first pass is the seed's");
    let _ = stop(&mut free, &s);
    let two = play(&mut free, &s, &path, 0, 96_000, 128);
    assert_ne!(one, two, "unlatched, the next pass draws on");
    // Another seed plays other notes.
    assert_ne!(play(&mut Sequencer::new(12, true), &s, &path, 0, 96_000, 128), first);
}

#[test]
fn a_locate_starts_on_the_next_grid_point() {
    let s = Settings { voices: Voices::Clock, ..settings() };
    let path = Path::constant(120.0);
    let mut seq = Sequencer::new(5, true);
    let _ = play(&mut seq, &s, &path, 0, 8_000, 100);
    // Jump to sample 100 000 (beat 4.1667): the next sixteenth is beat 4.25, sample 102 000.
    let ev = play(&mut seq, &s, &path, 100_000, 20_000, 100);
    assert!(matches!(ev.first(), Some((100_000, EventKind::NoteOff { .. }))), "the sounding note ends at the jump: {:?}", ev.first());
    assert_eq!(ons(&ev).first(), Some(&102_000));
}

#[test]
fn every_note_is_ended_and_none_is_doubled() {
    for s in [
        settings(),
        Settings { model: Model::Divider, t_bias: 0.0, ..settings() },
        Settings { model: Model::Divider, t_bias: 0.97, rate: 20, ..settings() },
        Settings { gate_length: 1.0, gate_random: 1.0, ..settings() },
        Settings { model: Model::Clusters, t_bias: 0.05, jitter: 0.7, spread: 0.0, ..settings() },
        Settings { model: Model::ThreeStates, gate_length: 0.9, spread: 1.0, steps: 1.0, ..settings() },
        Settings { model: Model::Drums, t_bias: 1.0, rate: 18, gate_random: 0.4, range: transit_random_sequencer::VoltageRange::Full, ..settings() },
        Settings { model: Model::Markov, x_mode: transit_random_sequencer::ControlMode::Bump, ..settings() },
    ] {
        let mut seq = Sequencer::new(9, false);
        let mut ev = play(&mut seq, &s, &Path::constant(140.0), 0, 48_000 * 10, 256);
        ev.extend(stop(&mut seq, &s).into_iter().map(|k| (i64::MAX, k)));
        let mut on = [false; 128];
        for (t, k) in &ev {
            match *k {
                EventKind::NoteOn { key, velocity } => {
                    assert!(!on[usize::from(key)], "{s:?}: key {key} started twice (at {t})");
                    assert!((1..=127).contains(&velocity));
                    on[usize::from(key)] = true;
                }
                EventKind::NoteOff { key } => on[usize::from(key)] = false,
            }
        }
        assert!(on.iter().all(|x| !x), "{s:?}: a note hangs after the stop");
        assert!(!seq.sounding());
    }
}

#[test]
fn frozen_the_loop_repeats_every_length_ticks() {
    let s = Settings { voices: Voices::Clock, freeze: true, length: 4, deja_vu_pitch: true, ..settings() };
    let ev = play(&mut Sequencer::new(21, true), &s, &Path::constant(120.0), 0, 48_000 * 8, 256);
    let keys: Vec<u8> = ev
        .iter()
        .filter_map(|(_, k)| match k {
            EventKind::NoteOn { key, .. } => Some(*key),
            EventKind::NoteOff { .. } => None,
        })
        .collect();
    assert!(keys.len() >= 60);
    // After the loop fills (the first pass), every key repeats four ticks later.
    for i in 8..keys.len() {
        assert_eq!(keys[i], keys[i - 4], "tick {i}: {keys:?}");
    }
    assert!(keys[8..12].iter().any(|k| *k != keys[8]), "the loop holds more than one pitch: {keys:?}");
}

#[test]
fn the_rate_table_is_slow_to_fast_and_the_firmwares() {
    let mut last = f64::INFINITY;
    for (i, (name, (p, q))) in RATES.iter().enumerate() {
        let d = division(i);
        assert!(d < last, "{name}");
        last = d;
        // The firmware's divisions of a patched clock: p/q from 1/4 to 4, times 1/4 or 4.
        let r = f64::from(*p) / f64::from(*q);
        let base = [0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0, 4.0];
        let ok = base.iter().any(|b| [0.25, 1.0, 4.0].iter().any(|m| (b * m - r).abs() < 1e-12));
        assert!(ok, "{name} = {r} is not a firmware division");
    }
    assert_eq!(RATES.len(), 21);
}

#[test]
fn a_stopped_transport_plays_nothing() {
    let s = settings();
    let mut seq = Sequencer::new(1, true);
    assert!(stop(&mut seq, &s).is_empty());
}

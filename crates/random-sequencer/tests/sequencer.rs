//! The transport-locked sequencer: the same notes at any block size, ticks on the beat grid
//! whatever the tempo, a latched seed that replays a path, and every note ended.

#![forbid(unsafe_code)]

use transit_random_sequencer::VoltageRange;
use transit_random_sequencer::{Block, Event, EventKind, Model, RATES, Sequencer, Settings, Voices, deja_vu_dead_band, division};

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

/// Every note-on is followed by its off before the key starts again, and the stop ends the rest.
fn assert_paired(label: &str, events: &[(i64, EventKind)]) {
    let mut on = [false; 128];
    for (t, k) in events {
        match *k {
            EventKind::NoteOn { key, .. } => {
                assert!(!on[usize::from(key)], "{label}: key {key} started twice (at {t})");
                on[usize::from(key)] = true;
            }
            EventKind::NoteOff { key } => on[usize::from(key)] = false,
        }
    }
    assert!(on.iter().all(|x| !x), "{label}: a note hangs after the stop");
}

/// The review's stress (#202 C1): dense settings across all 7 models, short gates, keys folded at
/// the top of the range, played forward, wrapped back to 0, located forward and stopped, at the
/// fastest rates and large host blocks. Seeded; nothing depends on a clock.
#[test]
fn dense_settings_at_fast_rates_and_large_blocks_never_drop_or_double() {
    let models = [Model::CoinToss, Model::Clusters, Model::Drums, Model::Independent, Model::Divider, Model::ThreeStates, Model::Markov];
    let gates = [(0.02, 0.0), (0.05, 1.0), (1.0, 0.0)];
    let rate_64 = RATES.len() - 1;
    let rate_32t = RATES.iter().position(|r| r.0 == "1/32T").expect("1/32T is a rate");
    assert_eq!(RATES[rate_64].0, "1/64");
    let mut runs = 0u32;
    let mut notes = 0usize;
    for (rate, tempo, blocks) in [
        (rate_64, 300.0, vec![1024, 4096, 8192, 16384]),
        (rate_32t, 300.0, vec![1024, 4096, 8192, 16384]),
        (rate_64, 140.0, vec![8192]),
        (rate_32t, 174.0, vec![8192]),
    ] {
        for block in blocks {
            for (mi, model) in models.iter().enumerate() {
                for t_bias in [0.0, 0.5, 0.97, 1.0] {
                    for (gi, &(gate_length, gate_random)) in gates.iter().enumerate() {
                        let s = Settings {
                            rate,
                            model: *model,
                            t_bias,
                            gate_length,
                            gate_random,
                            voices: Voices::All,
                            spread: 0.0,
                            range: VoltageRange::Full,
                            base_note: 125,
                            jitter: 0.0,
                            ..Settings::default()
                        };
                        let seed = 1 + mi as u32 * 31 + gi as u32 * 7 + runs;
                        let mut seq = Sequencer::new(seed, false);
                        let path = Path::constant(tempo);
                        let label = format!("rate {rate} {tempo} bpm block {block} {model:?} bias {t_bias} gate {gate_length}/{gate_random}");
                        let mut ev = play(&mut seq, &s, &path, 0, 4 * 48_000, block);
                        // A loop wrap (back to 0), then a locate forward.
                        ev.extend(play(&mut seq, &s, &path, 0, 2 * 48_000, block));
                        ev.extend(play(&mut seq, &s, &path, 10 * 48_000, 3 * 48_000, block));
                        ev.extend(stop(&mut seq, &s).into_iter().map(|k| (i64::MAX, k)));
                        assert_eq!(seq.dropped(), 0, "{label}: the schedule refused events");
                        assert_paired(&label, &ev);
                        notes += ons(&ev).len();
                        runs += 1;
                    }
                }
            }
        }
    }
    assert_eq!(runs, 840, "runs");
    assert!(notes > 100_000, "the stress is dense: {notes} note-ons");
    println!("stress: {runs} runs, {notes} note-ons");
}

/// The dead band of the deja vu knob (the firmware's main loop): 0.47 and 0.53 are its edges.
#[test]
#[allow(clippy::excessive_precision)] // The firmware's constant, as written.
fn the_deja_vu_dead_band_has_its_edges_at_0_47_and_0_53() {
    use transit_random_sequencer::float::F;
    let k = 1.063_829_787_23_f32;
    let at = |x: f32| deja_vu_dead_band(F(x)).0;
    assert_eq!(at(0.0), 0.0);
    assert_eq!(at(0.46), 0.46 * k);
    assert!(at(0.46) < 0.4895 && at(0.46) > 0.4893, "{}", at(0.46));
    assert!(at(0.469) < 0.5, "just below the lower edge it still moves: {}", at(0.469));
    assert_eq!(at(0.47), 0.5, "the lower edge is inside the band");
    assert_eq!(at(0.48), 0.5, "inside the band, away from the edge");
    assert_eq!(at(0.5), 0.5);
    assert_eq!(at(0.52), 0.5, "inside the band, away from the edge");
    assert_eq!(at(0.53), 0.5, "the upper edge is inside the band");
    assert!(at(0.531) > 0.5, "just above the upper edge it moves: {}", at(0.531));
    assert_eq!(at(0.54), 0.5 + (0.54 - 0.53) * k);
    assert!((at(1.0) - 1.0).abs() < 1e-6);
}

/// With Latch off the sequence runs on across passes (the module's rule), so a new Seed is heard
/// only if the sequencer has not played yet; latched, every start restarts from the current seed.
#[test]
fn a_new_seed_is_heard_unlatched_only_before_the_first_play() {
    let s = settings();
    let path = Path::constant(120.0);
    // Unlatched, changed before playing: the new seed plays from the first pass.
    let mut early = Sequencer::new(5, false);
    early.set_seed(6, false);
    let mut direct = Sequencer::new(6, false);
    assert_eq!(play(&mut early, &s, &path, 0, 96_000, 128), play(&mut direct, &s, &path, 0, 96_000, 128));
    // Unlatched, changed after the first play: the sequence runs on as if nothing changed.
    let mut a = Sequencer::new(5, false);
    let mut b = Sequencer::new(5, false);
    let _ = play(&mut a, &s, &path, 0, 96_000, 128);
    let _ = play(&mut b, &s, &path, 0, 96_000, 128);
    let _ = (stop(&mut a, &s), stop(&mut b, &s));
    a.set_seed(99, false);
    assert_eq!(a.seed(), 99);
    assert_eq!(play(&mut a, &s, &path, 0, 96_000, 128), play(&mut b, &s, &path, 0, 96_000, 128));
    // Latched, a new seed after playing is heard at the next start.
    let mut l = Sequencer::new(5, true);
    let first = play(&mut l, &s, &path, 0, 96_000, 128);
    let _ = stop(&mut l, &s);
    l.set_seed(99, true);
    let second = play(&mut l, &s, &path, 0, 96_000, 128);
    assert_ne!(first, second);
    assert_eq!(second, play(&mut Sequencer::new(99, true), &s, &path, 0, 96_000, 128));
}

/// #202 C2: Jitter toggled between 1.0 and 0 block by block leaves a tick just before a grid point;
/// the next tick must not come before this tick's events have ended (no double note-on).
#[test]
fn jitter_toggled_block_by_block_never_doubles_a_note() {
    let models = [Model::CoinToss, Model::Divider, Model::Clusters];
    let rate_64 = RATES.len() - 1;
    let rate_16 = RATES.iter().position(|r| r.0 == "1/16").expect("1/16 is a rate");
    let mut runs = 0u32;
    for (rate, tempo, block, toggles) in [(rate_64, 300.0, 600u32, 10_000i64), (rate_16, 120.0, 6000, 1_500)] {
        for seed in 1..=20u32 {
            for model in models {
                let mut seq = Sequencer::new(seed, false);
                let path = Path::constant(tempo);
                let mut ev = Vec::new();
                for i in 0..toggles {
                    let s = Settings {
                        rate,
                        model,
                        voices: Voices::All,
                        spread: 0.0,
                        jitter: if i % 2 == 0 { 1.0 } else { 0.0 },
                        ..Settings::default()
                    };
                    ev.extend(play(&mut seq, &s, &path, i * i64::from(block), i64::from(block), block));
                }
                ev.extend(stop(&mut seq, &Settings::default()).into_iter().map(|k| (i64::MAX, k)));
                assert_eq!(seq.dropped(), 0, "rate {rate} seed {seed} {model:?}: the schedule refused events");
                assert_paired(&format!("rate {rate} {tempo} bpm seed {seed} {model:?}"), &ev);
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 120);
}

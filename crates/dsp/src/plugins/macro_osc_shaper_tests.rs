//! The Macro Oscillator's shaper and matrix in the instrument (#243): each source reaches the
//! sound through a row, the destinations do what their units say, the shaper gates and releases
//! the voices, and the synced LFOs follow the song position. Block-driven and deterministic.

use super::matrix::{dst, src};
use super::*;
use crate::event::{Message, NoteId, PluginEvent};

const SR: f32 = 48_000.0;

fn inst(voices: usize, params: &[(&str, f32)]) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    m.set_param("voices", (voices - 1) as f32);
    for (k, v) in params {
        assert!(m.set_param(k, *v), "{k}");
    }
    m.prepare(SR, 512, 2);
    m
}

fn set_row(m: &mut MacroOscillator, n: usize, source: usize, dest: usize, amount: f32) {
    for (f, v) in [("source", source as f32), ("dest", dest as f32), ("amount", amount)] {
        assert!(m.set_param(&format!("mod{n}_{f}"), v));
    }
}

/// Renders `frames` frames in host blocks of `block`: (left, right).
fn render(m: &mut MacroOscillator, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut io = vec![vec![0.0; block]; 2];
    let (mut l, mut r) = (Vec::new(), Vec::new());
    while l.len() < frames {
        let n = block.min(frames - l.len());
        m.process(&mut io, n);
        l.extend_from_slice(&io[0][..n]);
        r.extend_from_slice(&io[1][..n]);
    }
    (l, r)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn ev(message: Message) -> PluginEvent {
    PluginEvent { channel: 0, note_id: -1, message }
}

/// Every controller source moves the sound through a row to Timbre, and does nothing without one.
#[test]
fn each_controller_source_reaches_the_sound_through_a_row() {
    let cases: [(usize, Message); 4] = [
        (src::AFTERTOUCH, Message::ChannelPressure { value: 127 }),
        (src::POLY_AFTERTOUCH, Message::PolyPressure { key: 60, value: 127, id: NoteId::NONE }),
        (src::MOD_WHEEL, Message::Control { cc: 1, value: 127 }),
        (src::PITCH_BEND, Message::PitchBend { value: 16383 }),
    ];
    for (source, message) in cases {
        let play = |row: bool, send: bool| {
            // The wheel's own destination is Off, so only the row hears it; no bend range either.
            let mut m = inst(1, &[("model", 8.0), ("timbre", 0.0), ("mod_wheel", 3.0), ("bend_range", 0.0)]);
            if row {
                set_row(&mut m, 1, source, dst::TIMBRE, 100.0);
            }
            m.note_on(0, 60, 100);
            if send {
                m.event(0, &ev(message));
            }
            render(&mut m, 4800, 256).0
        };
        assert!(play(true, true) != play(true, false), "{}: the row hears it", matrix::SOURCES[source]);
        assert!(play(false, true) == play(false, false), "{}: no row, no change", matrix::SOURCES[source]);
    }
}

/// The note-off velocity is a source from the release on.
#[test]
fn release_velocity_is_read_after_the_note_off() {
    let play = |vel: u8| {
        let mut m = inst(1, &[("model", 8.0), ("timbre", 0.0), ("decay", 80.0)]);
        set_row(&mut m, 1, src::RELEASE_VELOCITY, dst::TIMBRE, 100.0);
        m.note_on(0, 60, 100);
        let (held, _) = render(&mut m, 2400, 256);
        m.event(0, &ev(Message::NoteOff { key: 60, velocity: vel, id: NoteId::NONE }));
        (held, render(&mut m, 4800, 256).0)
    };
    let (a_held, a_tail) = play(0);
    let (b_held, b_tail) = play(127);
    assert!(a_held == b_held, "held: the release velocity is not known yet");
    assert!(a_tail != b_tail, "the tail hears it");
}

/// Pitch at 100% is 24 semitones: the wheel at full on a 50% Pitch row plays as Tune at +12, bit
/// for bit.
#[test]
fn a_pitch_row_moves_the_note_in_semitones() {
    let mut row = inst(1, &[("model", 8.0), ("mod_wheel", 3.0)]);
    set_row(&mut row, 1, src::MOD_WHEEL, dst::PITCH, 50.0);
    row.event(0, &ev(Message::Control { cc: 1, value: 127 }));
    row.note_on(0, 48, 100);
    let mut tuned = inst(1, &[("model", 8.0), ("mod_wheel", 3.0), ("tune", 12.0)]);
    tuned.note_on(0, 48, 100);
    assert!(render(&mut row, 9600, 256) == render(&mut tuned, 9600, 256));
}

/// Pan: hard right silences the left channel and leaves the right at the voice's level; Level at
/// -100% silences the voice. Without rows both channels carry the same bits.
#[test]
fn level_and_pan_rows_set_the_voice_gains() {
    let mut plain = inst(2, &[("model", 8.0)]);
    plain.note_on(0, 60, 100);
    let (pl, pr) = render(&mut plain, 4800, 256);
    assert!(pl == pr && peak(&pl) > 0.01);
    let mut right = inst(2, &[("model", 8.0), ("mod_wheel", 3.0)]);
    set_row(&mut right, 1, src::MOD_WHEEL, dst::PAN, 100.0);
    right.event(0, &ev(Message::Control { cc: 1, value: 127 }));
    right.note_on(0, 60, 100);
    let (l, r) = render(&mut right, 4800, 256);
    assert_eq!(peak(&l[256..]), 0.0, "hard right: the left is silent");
    assert!(r[256..] == pr[256..], "the right keeps the voice exactly");
    let mut muted = inst(2, &[("model", 8.0), ("mod_wheel", 3.0)]);
    set_row(&mut muted, 1, src::MOD_WHEEL, dst::LEVEL, -100.0);
    muted.event(0, &ev(Message::Control { cc: 1, value: 127 }));
    muted.note_on(0, 60, 100);
    assert_eq!(peak(&render(&mut muted, 4800, 256).0[256..]), 0.0);
}

/// With the shaper on, the voice lasts as long as its amp envelope: it sounds while held, ends
/// after the release, goes idle, and the tail covers the release.
#[test]
fn the_shaper_amp_envelope_ends_the_voice() {
    let mut m = inst(2, &[("model", 8.0), ("shaper", 1.0), ("amp_release", 100.0), ("decay", 100.0)]);
    m.note_on(0, 60, 127);
    let (held, _) = render(&mut m, 9600, 256);
    assert!(peak(&held[4800..]) > 0.05, "held at the sustain level");
    m.note_off(0, 60);
    let (tail, _) = render(&mut m, 19_200, 256);
    assert_eq!(peak(&tail[14_400..]), 0.0, "silent after the release");
    assert!(m.slots[0].idle, "the voice is free");
    assert!(m.tail_samples() >= (SR * 20.0) as usize);
    m.set_param("amp_release", 20_000.0);
    assert!(m.tail_samples() > (SR * 33.0) as usize, "a 20 s release to -60 dB ends at -100 dB after 33 s");
}

/// The shaper's filter: a low pass at its lowest cutoff takes most of a bright model's level away.
#[test]
fn the_shaper_filter_darkens() {
    let play = |cutoff: f32| {
        let mut m = inst(1, &[("model", 17.0), ("shaper", 1.0), ("cutoff", cutoff), ("filter_slope", 1.0)]);
        m.note_on(0, 60, 127);
        let (l, _) = render(&mut m, 9600, 256);
        l[2400..].iter().map(|v| v * v).sum::<f32>()
    };
    let (open, closed) = (play(20_000.0), play(80.0));
    assert!(closed < 0.1 * open, "noise through an 80 Hz low pass: {closed} of {open}");
}

/// A synced, free-running LFO follows the song position: an instance that starts playing at
/// quarter note 10 and one that played from 0 have the same phase at the same frame.
#[test]
fn a_synced_lfo_follows_the_song_position() {
    let params =
        [("lfo1_sync", 1.0), ("lfo1_retrigger", 0.0), ("lfo1_division", 4.0), ("mod1_source", 1.0), ("mod1_dest", 3.0), ("mod1_amount", 50.0)];
    let mut from_start = inst(1, &params);
    let mut from_bar = inst(1, &params);
    let quarter = (SR / 2.0) as i64; // 120 BPM
    let transport = |samples: i64| Transport {
        playing: true,
        tempo: 120.0,
        numerator: 4,
        denominator: 4,
        samples,
        quarters: samples as f64 / quarter as f64,
        ..Transport::default()
    };
    let mut io = vec![vec![0.0; 240]; 2];
    let mut pos = 0i64;
    while pos < 10 * quarter + 4800 {
        from_start.set_transport(&transport(pos));
        from_start.process(&mut io, 240);
        if pos >= 10 * quarter {
            from_bar.set_transport(&transport(pos));
            from_bar.process(&mut io, 240);
        }
        pos += 240;
    }
    from_start.note_on(0, 60, 100);
    from_bar.note_on(0, 60, 100);
    from_start.set_transport(&transport(pos));
    from_bar.set_transport(&transport(pos));
    from_start.process(&mut io, 240);
    from_bar.process(&mut io, 240);
    let (a, b) = (from_start.slots[0].shaper.lfo[0].phase(), from_bar.slots[0].shaper.lfo[0].phase());
    assert!((a - b).abs() < 1e-9, "{a} vs {b}");
}

/// Renders host blocks of 240 frames until the first `n` voices are idle (at most `cap` blocks):
/// the left channel and the block in which they all were idle.
fn render_until_idle(m: &mut MacroOscillator, n: usize, cap: usize) -> (Vec<f32>, Option<usize>) {
    let mut io = vec![vec![0.0; 240]; 2];
    let mut l = Vec::new();
    for b in 0..cap {
        m.process(&mut io, 240);
        l.extend_from_slice(&io[0]);
        if m.slots.iter().take(n).all(|s| s.idle) {
            return (l, Some(b));
        }
    }
    (l, None)
}

fn largest_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// The held-note rule's bound on a switch (#262): no step past the larger of the two paths' own
/// largest steps plus the crossfade's, the gap between the paths spread over the 12-frame block
/// (at most the sum of their peaks over 12).
fn switch_bound(before: &[f32], after: &[f32]) -> f32 {
    largest_step(before).max(largest_step(after)) + (peak(before) + peak(after)) / BLOCK as f32
}

/// A held sine (the two-operator FM model at Timbre 0, whose own steps are small, so a step at
/// the switch shows) and the shaper's settings that make it audible: a low-pass at 300 Hz, the
/// amp envelope sustaining at 70 %, a short release.
const DARK: [(&str, f32); 6] = [("model", 10.0), ("timbre", 0.0), ("decay", 60.0), ("cutoff", 300.0), ("amp_sustain", 70.0), ("amp_release", 10.0)];

/// Turning the shaper on under a held chord shapes it at once (#262): from the block after the
/// switch's 12-frame crossfade the voices sound as voices shaped from the start do (the
/// envelopes ran all along, so they are where a shaped phrase's would be: no attack restarts),
/// and the switch makes no step past the crossfade's bound. The voices then end on the amp
/// envelope's short release, not the low-pass gate's long decay.
#[test]
fn switching_the_shaper_on_shapes_held_notes_at_once() {
    let chord = [60u8, 64, 67];
    let play = |shaper: [bool; 2]| {
        let mut m = inst(4, &DARK);
        assert!(m.set_param("shaper", if shaper[0] { 1.0 } else { 0.0 }));
        for k in chord {
            m.note_on(0, k, 110);
        }
        let (before, _) = render(&mut m, 9600, 240);
        assert!(m.set_param("shaper", if shaper[1] { 1.0 } else { 0.0 }));
        let (after, _) = render(&mut m, 9600, 240);
        (m, before, after)
    };
    let (_, _, unshaped) = play([false, false]);
    let (_, _, shaped) = play([true, true]);
    let (mut m, before, after) = play([false, true]);
    let diff = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x - y).collect::<Vec<f32>>();
    // Within one block: from frame 12 on, far closer to the shaped render than the unshaped is.
    let window = 12..480;
    let to_shaped = rms(&diff(&after[window.clone()], &shaped[window.clone()]));
    let apart = rms(&diff(&unshaped[window.clone()], &shaped[window]));
    assert!(to_shaped < 0.1 * apart, "the held voices are filtered within one block: {to_shaped} from the shaped render, {apart} between the paths");
    // And they settle on it (the low-pass gate opens over a few blocks, the filter from rest).
    let late = rms(&diff(&after[4800..], &shaped[4800..]));
    println!("on: {to_shaped} from the shaped render in frames 12..480 ({apart} between the paths), {late} after 100 ms");
    assert!(late < 1e-3 * rms(&shaped[4800..]), "the held voices sound as shaped ones: {late}");
    let mut joined = before[before.len() - 2400..].to_vec();
    joined.extend_from_slice(&after[..2400]);
    let bound = switch_bound(&before[before.len() - 2400..], &after[12..2400]);
    println!("on: largest step {} at the switch, bound {bound}", largest_step(&joined));
    assert!(largest_step(&joined) <= bound, "no step past {bound} at the switch: {}", largest_step(&joined));
    for k in chord {
        m.note_off(0, k);
    }
    let (_, idle_at) = render_until_idle(&mut m, 4, 4000);
    assert!(idle_at.is_some_and(|b| b < 4), "the amp envelope's release ends the voices: {idle_at:?}");
}

/// Turning the shaper off under a held note takes it out at once (#262): no step past the
/// crossfade's bound, the low-pass gate carries the note on, and after the note-off the gate's
/// decay ends it (no stuck note).
#[test]
fn switching_the_shaper_off_under_a_held_note_leaves_no_stuck_note_and_no_click() {
    let mut m = inst(2, &DARK);
    assert!(m.set_param("shaper", 1.0));
    m.note_on(0, 60, 110);
    let (before, _) = render(&mut m, 9600, 240);
    assert!(m.set_param("shaper", 0.0));
    let (after, _) = render(&mut m, 9600, 240);
    let mut joined = before[before.len() - 2400..].to_vec();
    joined.extend_from_slice(&after[..2400]);
    let bound = switch_bound(&before[before.len() - 2400..], &after[12..2400]);
    assert!(largest_step(&joined) <= bound, "no step past {bound} at the switch: {}", largest_step(&joined));
    println!("off: largest step {} at the switch, bound {bound}", largest_step(&joined));
    assert!(peak(&after[4800..]) > 0.05, "the gate carries the note on");
    m.note_off(0, 60);
    let (_, idle_at) = render_until_idle(&mut m, 2, 4000);
    assert!(idle_at.is_some_and(|b| b > 4), "the gate's decay ends the note: {idle_at:?}");
}

/// Turning the shaper on while a chord tails in its low-pass gate does not cut it: a voice in its
/// release keeps its path, so the render is the one without the switch, with no step, and the
/// voices end when the gate's decay does. A phrase started after the switch is shaped (its short
/// amp release ends it long before the gate would).
#[test]
fn switching_the_shaper_on_keeps_released_voices_on_their_gate() {
    let chord = [60u8, 64, 67];
    let play = |switch: bool| {
        let mut m = inst(4, &[("model", 8.0), ("decay", 60.0), ("amp_release", 10.0)]);
        for k in chord {
            m.note_on(0, k, 110);
        }
        let (mut before, _) = render(&mut m, 9600, 240);
        for k in chord {
            m.note_off(0, k);
        }
        before.extend(render(&mut m, 480, 240).0);
        if switch {
            assert!(m.set_param("shaper", 1.0));
        }
        let (after, idle_at) = render_until_idle(&mut m, 4, 4000);
        (m, before, after, idle_at)
    };
    let (_, ref_before, ref_after, ref_idle) = play(false);
    let (mut m, before, after, idle_at) = play(true);
    assert_eq!(before, ref_before);
    assert!(ref_idle.is_some_and(|b| b > 10), "the gate's decay takes a while: {ref_idle:?}");
    assert_eq!(idle_at, ref_idle, "the old voices end when their gate's decay does");
    assert!(after == ref_after, "the voices tailing at the switch play on as before");
    m.note_on(0, 72, 110);
    let _ = render(&mut m, 2400, 240);
    m.note_off(0, 72);
    let (_, shaped_idle) = render_until_idle(&mut m, 4, 4000);
    assert!(shaped_idle.is_some_and(|b| b < 4), "the amp envelope ends the new voice: {shaped_idle:?}");
}

/// Turning the shaper off during a shaped voice's release does not cut it: the release plays out
/// on the amp envelope and the render is the one without the switch.
#[test]
fn switching_the_shaper_off_keeps_a_shaped_release() {
    let play = |switch: bool| {
        let mut m = inst(2, &[("model", 8.0), ("shaper", 1.0), ("amp_release", 500.0)]);
        m.note_on(0, 60, 120);
        let (mut l, _) = render(&mut m, 4800, 240);
        m.note_off(0, 60);
        l.extend(render(&mut m, 2400, 240).0);
        if switch {
            assert!(m.set_param("shaper", 0.0));
        }
        let (tail, idle_at) = render_until_idle(&mut m, 2, 4000);
        l.extend_from_slice(&tail);
        (l, idle_at)
    };
    let (reference, ref_idle) = play(false);
    let (switched, idle_at) = play(true);
    assert!(switched == reference, "the release plays out as if the shaper stayed on");
    assert!(idle_at.is_some() && idle_at == ref_idle);
}

/// #253: both matrix LFOs count bars after the meter change, wherever playback starts.
#[test]
fn bar_divisions_follow_meter_map_downbeats() {
    for (division, bars) in [(2, 1.0), (1, 2.0), (0, 4.0)] {
        for (quarters, bar, bar_start, expected_bars) in [(14.0, 4, 14.0, 4.0), (17.0, 5, 17.0, 5.0), (18.0, 5, 17.0, 5.0 + 1.0 / 3.0)] {
            let mut m = inst(
                1,
                &[
                    ("lfo1_sync", 1.0),
                    ("lfo1_retrigger", 0.0),
                    ("lfo1_division", division as f32),
                    ("lfo2_sync", 1.0),
                    ("lfo2_retrigger", 0.0),
                    ("lfo2_division", division as f32),
                ],
            );
            m.set_transport(&Transport {
                playing: true,
                tempo: 120.0,
                numerator: 3,
                denominator: 4,
                samples: (quarters * 24_000.0) as i64,
                quarters,
                bar,
                bar_start,
                ..Transport::default()
            });
            m.note_on(0, 60, 100);
            m.process(&mut [vec![0.0; 12], vec![0.0; 12]], 12);
            let expected = (expected_bars / bars) % 1.0;
            for lfo in &m.slots[0].shaper.lfo {
                assert!((lfo.phase() - expected).abs() < 1e-12, "division {division}, quarter {quarters}: {} != {expected}", lfo.phase());
            }
        }
    }
}

/// Continuous processing across bar 3: both matrix LFOs preserve the multi-bar count,
/// and every beat division keeps its absolute-quarter phase after the meter change.
#[test]
fn synced_divisions_cross_a_meter_change_during_playback() {
    for division in 0..14 {
        let mut m = inst(
            1,
            &[
                ("lfo1_sync", 1.0),
                ("lfo1_retrigger", 0.0),
                ("lfo1_division", division as f32),
                ("lfo2_sync", 1.0),
                ("lfo2_retrigger", 0.0),
                ("lfo2_division", division as f32),
            ],
        );
        m.note_on(0, 60, 100);
        for samples in (191_760..192_240).step_by(12) {
            let quarters = samples as f64 / 24_000.0;
            let changed = samples >= 192_000;
            let (numerator, bar, bar_start) = if changed { (3, 2, 8.0) } else { (4, 1, 4.0) };
            m.set_transport(&Transport {
                playing: true,
                tempo: 120.0,
                numerator,
                denominator: 4,
                samples,
                quarters,
                bar,
                bar_start,
                ..Transport::default()
            });
            m.process(&mut [vec![0.0; 12], vec![0.0; 12]], 12);
            let cycles = if division <= 2 {
                (bar as f64 + (quarters - bar_start) / f64::from(numerator)) / [4.0, 2.0, 1.0][division]
            } else {
                quarters / crate::plugins::lfo::division_quarters(division, 4.0)
            };
            for lfo in &m.slots[0].shaper.lfo {
                assert!((lfo.phase() - (cycles - cycles.floor())).abs() < 1e-12, "division {division}, sample {samples}");
            }
        }
    }
}

#[test]
fn bar_divisions_use_the_full_valid_meter_range() {
    for (numerator, denominator, length) in [(1, 64, 0.0625), (64, 1, 256.0)] {
        for (division, bars) in [(0, 4.0), (1, 2.0), (2, 1.0)] {
            let mut m = inst(
                1,
                &[
                    ("lfo1_sync", 1.0),
                    ("lfo1_retrigger", 0.0),
                    ("lfo1_division", division as f32),
                    ("lfo2_sync", 1.0),
                    ("lfo2_retrigger", 0.0),
                    ("lfo2_division", division as f32),
                ],
            );
            m.set_transport(&Transport {
                playing: true,
                tempo: 120.0,
                numerator,
                denominator,
                bar: 1,
                bar_start: length,
                quarters: length * 1.5,
                ..Transport::default()
            });
            m.note_on(0, 60, 100);
            m.process(&mut [vec![0.0; 12], vec![0.0; 12]], 12);
            for (lfo, settings) in m.slots[0].shaper.lfo.iter().zip(&m.shaper.lfo) {
                assert!((lfo.phase() - (1.5 / bars) % 1.0).abs() < 1e-12);
                assert!((settings.increment(120.0, 0.0, SR) - 1.0 / (24_000.0 * length * bars)).abs() < 1e-15);
            }
        }
    }
}

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

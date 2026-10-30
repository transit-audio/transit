use super::*;
use crate::event::NoteId;

const SR: f32 = 48_000.0;
/// The two-operator FM model: a sine at timbre 0, so zero crossings measure its pitch.
const FM: f32 = 10.0;

fn inst() -> MacroOscillator {
    let mut m = MacroOscillator::new();
    m.prepare(SR, 512, 2);
    m
}

/// Renders `frames` frames in host blocks of `block`, returning the left channel.
fn render(m: &mut MacroOscillator, frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut io = vec![vec![0.0; block]; 2];
    while out.len() < frames {
        let n = block.min(frames - out.len());
        m.process(&mut io, n);
        assert_eq!(io[0][..n], io[1][..n], "both channels carry the mono voice");
        out.extend_from_slice(&io[0][..n]);
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Frequency from the first and last rising zero crossings (interpolated), DC removed.
fn hz(x: &[f32], rate: f32) -> f64 {
    let mean = x.iter().map(|v| f64::from(*v)).sum::<f64>() / x.len() as f64;
    let c: Vec<f64> = x
        .windows(2)
        .enumerate()
        .map(|(i, w)| (i, f64::from(w[0]) - mean, f64::from(w[1]) - mean))
        .filter(|(_, a, b)| *a < 0.0 && *b >= 0.0)
        .map(|(i, a, b)| i as f64 - a / (b - a))
        .collect();
    assert!(c.len() > 50, "{} crossings", c.len());
    (c.len() - 1) as f64 / ((c[c.len() - 1] - c[0]) / f64::from(rate))
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn sine_patch(m: &mut MacroOscillator) {
    m.set_param("model", FM);
    m.set_param("timbre", 0.0);
}

/// The pitch of `note` held for 1 s (the first 0.25 s skipped).
fn pitch(m: &mut MacroOscillator, note: u8) -> f64 {
    m.note_on(0, note, 100);
    let x = render(m, 48_000, 256);
    m.note_off(0, note);
    render(m, 24_000, 256);
    hz(&x[12_000..], SR)
}

#[test]
fn a_note_sounds_then_decays_to_exact_silence() {
    let mut m = inst();
    assert_eq!(peak(&render(&mut m, 4800, 256)), 0.0, "silent before any note");
    m.note_on(0, 60, 100);
    let held = render(&mut m, 24_000, 256);
    assert!(peak(&held) > 0.05, "a held note sounds: peak {}", peak(&held));
    let tail_start = peak(&held[12_000..]);
    m.note_off(0, 60);
    let tail_len = m.tail_samples();
    let tail = render(&mut m, tail_len, 256);
    assert!(peak(&tail[..480]) > 0.01 * tail_start, "the release starts from the held level");
    let last = &tail[tail.len() - 4800..];
    assert_eq!(peak(last), 0.0, "silent within tail_samples: the low-pass gate closed and the int16 offset is taken off");
}

/// The slowest release (decay and color at 100%) dies out within `tail_samples`.
#[test]
fn the_slowest_release_ends_within_the_tail() {
    let mut m = inst();
    m.set_param("decay", 100.0);
    m.set_param("lpg_color", 100.0);
    m.set_param("level", 6.0);
    m.note_on(0, 48, 127);
    render(&mut m, 24_000, 512);
    m.note_off(0, 48);
    let tail_len = m.tail_samples();
    let tail = render(&mut m, tail_len, 512);
    let silent_from = tail.iter().rposition(|v| *v != 0.0).map_or(0, |i| i + 1);
    println!("silent after {:.2} s of {:.2} s", silent_from as f32 / SR, tail.len() as f32 / SR);
    assert!(silent_from < tail.len(), "still sounding at the end of tail_samples");
}

#[test]
fn velocity_sets_the_level() {
    let level = |vel| {
        let mut m = inst();
        sine_patch(&mut m);
        m.note_on(0, 60, vel);
        peak(&render(&mut m, 24_000, 256)[12_000..])
    };
    let (soft, loud) = (level(20), level(127));
    assert!(loud > 1.5 * soft && soft > 0.0, "velocity 20: {soft}, 127: {loud}");
}

#[test]
fn concert_pitch_by_default_and_the_hardware_toggle_restores_the_firmware_pitch() {
    let mut m = inst();
    sine_patch(&mut m);
    assert_eq!(m.param("hardware_tuning"), Some(0.0));
    let concert = pitch(&mut m, 69);
    m.set_param("hardware_tuning", 1.0);
    let hardware = pitch(&mut m, 69);
    let factor = f64::from(transit_macro_osc::REF_SAMPLE_RATE) / f64::from(transit_macro_osc::CORRECTED_SAMPLE_RATE);
    println!("concert {concert:.4} Hz, hardware {hardware:.4} Hz");
    assert!(cents(concert, 440.0).abs() < 0.05, "A4 at concert pitch: {concert} Hz");
    let off = cents(hardware / concert, factor);
    assert!(off.abs() < 0.01, "the toggle shifts by 48000 / 47872.34: {hardware} / {concert} ({off:+.5} cents off)");
}

#[test]
fn pitch_bend_tune_and_other_host_rates() {
    let mut m = inst();
    sine_patch(&mut m);
    m.event(0, &PluginEvent { channel: 3, note_id: -1, message: Message::PitchBend { value: 16383 } });
    let up = pitch(&mut m, 57);
    assert!(cents(up, 440.0 * 2f64.powf(-10.0 / 12.0)).abs() < 0.5, "full bend up at range 2 is +2 st (to 1/8192): {up} Hz");
    m.set_param("bend_range", 12.0);
    m.midi(0, [0xE0, 0, 0]);
    let down = pitch(&mut m, 57);
    assert!(cents(down, 110.0).abs() < 0.5, "full bend down at range 12 is -12 st: {down} Hz");
    m.midi(0, [0xB0, 121, 0]);
    m.set_param("tune", 12.0);
    let tuned = pitch(&mut m, 57);
    assert!(cents(tuned, 440.0).abs() < 0.05, "tune +12 with the bend reset: {tuned} Hz");
    // 32 kHz runs the voice at 44.1 kHz and corrects the pitch to the host rate.
    let mut m = MacroOscillator::new();
    m.prepare(32_000.0, 256, 2);
    sine_patch(&mut m);
    m.note_on(0, 69, 100);
    let x = render(&mut m, 32_000, 256);
    let at32 = hz(&x[8000..], 32_000.0);
    assert!(cents(at32, 440.0).abs() < 0.5, "A4 at a 32 kHz host: {at32} Hz");
}

/// Legato and last-note priority: a second key while one is held changes the pitch with no new
/// trigger; releasing it returns to the first; the trigger falls only when no key is held.
#[test]
fn legato_and_last_note_priority() {
    let mut m = inst();
    let mut triggers = Vec::new();
    let step = |m: &mut MacroOscillator, triggers: &mut Vec<bool>| {
        render(m, 120, 12);
        triggers.push(m.last_trigger);
    };
    m.note_on(0, 60, 100);
    step(&mut m, &mut triggers);
    assert_eq!((m.note, m.last_trigger), (60.0, true));
    m.note_on(0, 64, 50);
    step(&mut m, &mut triggers);
    assert_eq!(m.note, 64.0, "the newest key plays");
    assert_eq!(m.velocity, 50.0 / 127.0, "the newest key's velocity is the level");
    m.note_on(0, 67, 100);
    m.note_off(0, 64);
    step(&mut m, &mut triggers);
    assert_eq!(m.note, 67.0, "releasing an older key leaves the newest playing");
    m.note_off(0, 67);
    step(&mut m, &mut triggers);
    assert_eq!(m.note, 60.0, "releasing the newest returns to the most recent key held");
    assert!(triggers.iter().all(|t| *t), "legato: the trigger never fell while a key was held: {triggers:?}");
    m.note_off(0, 60);
    step(&mut m, &mut triggers);
    assert!(!m.last_trigger, "the trigger falls with the last key");
    assert_eq!(m.note, 60.0, "the release keeps the last pitch");
}

/// A new phrase gets a new trigger edge even when its note-on shares a 12-frame block with the
/// previous phrase's note-off: the voice sees one low block first.
#[test]
fn a_new_phrase_retriggers_inside_one_block() {
    let mut m = inst();
    m.note_on(0, 60, 100);
    render(&mut m, 120, 12);
    m.note_off(0, 60);
    m.note_on(0, 62, 100);
    render(&mut m, 12, 12);
    assert!(!m.last_trigger, "one low block");
    render(&mut m, 12, 12);
    assert!(m.last_trigger, "then the new edge");
    assert_eq!(m.note, 62.0);
}

#[test]
fn the_sustain_pedal_holds_released_keys() {
    let mut m = inst();
    m.event(0, &PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 64, value: 127 } });
    m.note_on(0, 60, 100);
    m.note_off(1, 60);
    render(&mut m, 120, 12);
    assert!(m.last_trigger, "the pedal holds the released key");
    m.note_on(0, 64, 100);
    m.note_off(1, 64);
    render(&mut m, 120, 12);
    assert_eq!((m.note, m.last_trigger), (64.0, true), "a key under the pedal plays legato");
    m.midi(0, [0xB0, 64, 0]);
    render(&mut m, 24, 12);
    assert!(!m.last_trigger, "pedal up releases the voice");
    m.note_on(0, 60, 100);
    m.midi(0, [0xB0, 64, 127]);
    m.all_notes_off();
    render(&mut m, 24, 12);
    assert!(!m.last_trigger && !m.sustain, "all notes off clears keys and pedal");
}

#[test]
fn the_mod_wheel_reaches_its_destination() {
    let mut m = inst();
    sine_patch(&mut m);
    m.note_on(0, 57, 100);
    let plain = render(&mut m, 9600, 256);
    let mut w = inst();
    sine_patch(&mut w);
    w.note_on(0, 57, 100);
    w.event(0, &PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 1, value: 127 } });
    let wheel = render(&mut w, 9600, 256);
    assert!(plain.iter().zip(&wheel).any(|(a, b)| a != b), "the wheel on Timbre changes the sound");
    let mut off = inst();
    sine_patch(&mut off);
    off.set_param("mod_wheel", 3.0);
    off.note_on(0, 57, 100);
    off.event(0, &PluginEvent { channel: 0, note_id: -1, message: Message::Control14 { msb: 1, value: 16383 } });
    assert_eq!(render(&mut off, 9600, 256), plain, "Off ignores the wheel");
}

/// Any host block size gives the output of 12-frame host blocks bit for bit, with notes, bends and
/// parameter-free events at the same absolute frames.
#[test]
fn host_blocks_split_into_twelve_frame_renders_bit_for_bit() {
    let events: [(usize, Kind); 8] = [
        (5, Kind::On(60, 100)),
        (700, Kind::On(67, 90)),
        (1013, Kind::Bend(0.5)),
        (1500, Kind::Off(67)),
        (2049, Kind::Wheel(0.7)),
        (3001, Kind::Off(60)),
        (3002, Kind::On(48, 127)),
        (6000, Kind::Off(48)),
    ];
    let run = |block: usize| {
        let mut m = inst();
        m.set_param("model", 0.0);
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
    let reference = run(BLOCK);
    assert!(reference.iter().any(|b| *b != 0), "the reference sounds");
    for block in [1, 5, 7, 11, 13, 24, 64, 100, 256, 512, 1000] {
        assert!(run(block) == reference, "host blocks of {block} frames differ from 12-frame blocks");
    }
}

#[test]
fn every_model_sounds_and_aux_mix_selects_the_aux_output() {
    for (model, name) in MODELS.iter().enumerate() {
        let mut m = inst();
        m.set_param("model", model as f32);
        m.note_on(0, 48, 127);
        let out = render(&mut m, 9600, 256);
        let mut a = inst();
        a.set_param("model", model as f32);
        a.set_param("aux_mix", 100.0);
        a.note_on(0, 48, 127);
        let aux = render(&mut a, 9600, 256);
        assert!(peak(&out) > 1e-3 && peak(&aux) > 1e-3, "{name}: out {} aux {}", peak(&out), peak(&aux));
        assert!(peak(&out) <= db_to_gain(-6.0) * 32769.0 / 32768.0 + 1e-6, "{name} within full scale at the default level");
    }
}

#[test]
fn parameter_ids_are_stable_and_round_trip() {
    let ids: Vec<&str> = MACRO_OSC_INFO.params.iter().map(|p| p.id).collect();
    // Sessions and automation lanes store these ids: never rename one.
    assert_eq!(
        ids,
        ["model", "harmonics", "timbre", "morph", "decay", "lpg_color", "aux_mix", "level", "tune", "bend_range", "mod_wheel", "hardware_tuning"]
    );
    assert_eq!(MACRO_OSC_INFO.id, "macro_oscillator");
    assert_eq!(MACRO_OSC_INFO.name, "Macro Oscillator");
    let mut m = inst();
    for p in MACRO_OSC_INFO.params {
        let v = p.from_normalized(0.37);
        assert!(m.set_param(p.id, v), "{}", p.id);
        assert_eq!(m.param(p.id), Some(p.clamp(v)), "{}", p.id);
        assert!((p.from_normalized(p.to_normalized(v)) - p.clamp(v)).abs() < 1e-3, "{}", p.id);
    }
    assert_eq!(PARAMS[MODEL].choices.len(), 24);
    assert_eq!(PARAMS[MODEL].format(13.0), "Wavetable");
}

/// State is the parameters: a new instance given the same values plays the same bits.
#[test]
fn the_parameters_are_the_state() {
    let values: Vec<(&str, f32)> =
        MACRO_OSC_INFO.params.iter().enumerate().map(|(i, p)| (p.id, p.from_normalized((i as f32 * 0.29) % 1.0))).collect();
    let play = |m: &mut MacroOscillator| {
        m.note_on(0, 55, 110);
        render(m, 4800, 128)
    };
    let mut a = inst();
    for (id, v) in &values {
        a.set_param(id, *v);
    }
    let saved: Vec<(&str, f32)> = MACRO_OSC_INFO.params.iter().map(|p| (p.id, a.param(p.id).unwrap_or(f32::NAN))).collect();
    let mut b = inst();
    for (id, v) in &saved {
        b.set_param(id, *v);
    }
    assert!(play(&mut a) == play(&mut b));
    assert!(a.save_state().is_none(), "no state blob beyond the parameters");
}

#[test]
fn reset_returns_to_silence_and_a_fresh_voice() {
    let mut m = inst();
    m.note_on(0, 60, 100);
    let first = render(&mut m, 4800, 256);
    m.reset();
    assert_eq!(peak(&render(&mut m, 4800, 256)), 0.0);
    m.reset();
    m.note_on(0, 60, 100);
    assert!(render(&mut m, 4800, 256) == first, "after reset the voice plays as a new one");
}

#[test]
fn a_full_queue_never_refuses_a_release() {
    let mut m = inst();
    let ev = |message| PluginEvent { channel: 0, note_id: -1, message };
    for _ in 0..700 {
        m.event(0, &ev(Message::PitchBend { value: 9000 }));
    }
    let ons: Vec<bool> = (0..300u32).map(|i| m.event(1, &ev(Message::NoteOn { key: (i % 128) as u8, velocity: 100, id: NoteId::NONE }))).collect();
    let offs: Vec<bool> = (0..300u32).map(|i| m.event(2, &ev(Message::NoteOff { key: (i % 128) as u8, velocity: 0, id: NoteId::NONE }))).collect();
    assert_eq!(ons.iter().filter(|a| **a).count(), MAX_EVENTS - RELEASE_RESERVE - 700, "non-releases stop at the reserve");
    for (i, (a, b)) in ons.iter().zip(&offs).enumerate() {
        assert!(!*a || *b, "note {i} was admitted but its off was refused");
    }
    render(&mut m, 48, 12);
    assert!(m.notes.top().is_none(), "every admitted note was released");
    assert!(m.events.is_empty());
}

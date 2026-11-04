//! ADR 0041 Gate 1 at the core level: 48 kHz, a 128-frame constant-one mono source, level unity,
//! velocity 127, attack/decay/declick zero, sustain 1.

use super::{bind, core, note, render, unity};
use crate::event::{Control, Event, EventKind};
use crate::patch::{Pad, PlayMode, Zone};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{PADS, VOICES};

const ONES: [f32; 128] = [1.0; 128];
const ZEROS: [f32; 128] = [0.0; 128];

fn ramp() -> Vec<f32> {
    (0..128).map(|i| (i + 1) as f32).collect()
}

fn on(frame: u32, pad: usize, id: i32) -> Event {
    Event::new(frame, EventKind::NoteOn { channel: 0, key: note(pad), velocity: 127, id })
}

fn off(frame: u32, pad: usize, id: i32) -> Event {
    Event::new(frame, EventKind::NoteOff { channel: 0, key: note(pad), velocity: 64, id })
}

fn assert_frames(out: &[f32], expect: impl Fn(usize) -> f32) {
    for (n, y) in out.iter().enumerate() {
        assert_eq!(y.to_bits(), expect(n).to_bits(), "frame {n}: {y} != {}", expect(n));
    }
}

/// The choke factor of frame `n` for a fade that starts at `at`: (31 - j)/32 on frames at..at+32
/// (the ADR's figure written out, not the crate's constant, so a wrong length fails here).
fn choke_factor(n: usize, at: usize) -> f32 {
    if n < at {
        1.0
    } else if n < at + 32 {
        (31 - (n - at)) as f32 / 32.0
    } else {
        0.0
    }
}

#[test]
fn a_one_shot_plays_its_128_frames_and_ignores_the_note_off() {
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 0);
    let out = render(&mut c, &[SliceSource::mono(&ONES, 48_000.0)], &[on(0, 0, -1), off(16, 0, -1)], 200);
    assert_frames(&out, |n| if n < 128 { 1.0 } else { 0.0 });
    assert_eq!(c.active_voices(), 0);
}

#[test]
fn a_gate_release_of_8_frames_falls_by_eighths_from_the_note_off() {
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::Gate, 60, 8), 0);
    let out = render(&mut c, &[SliceSource::mono(&ONES, 48_000.0)], &[on(0, 0, -1), off(16, 0, -1)], 64);
    let tail = [0.875, 0.75, 0.625, 0.5, 0.375, 0.25, 0.125, 0.0];
    assert_frames(&out, |n| match n {
        0..16 => 1.0,
        16..24 => tail[n - 16],
        _ => 0.0,
    });
    assert_eq!(c.active_voices(), 0);
}

#[test]
fn a_retrigger_restarts_at_the_region_entry_and_the_old_note_off_does_not_release_it() {
    let mut c = core();
    let data = ramp();
    bind(&mut c, 0, 0, 128, unity(PlayMode::Gate, 60, 8), 0);
    let out = render(&mut c, &[SliceSource::mono(&data, 48_000.0)], &[on(0, 0, 1), on(20, 0, 2), off(21, 0, 1)], 60);
    // Frame 20 is source[0] (1), not source[20] (21) or a sum (22); the new note sounds on.
    assert_frames(&out, |n| if n < 20 { data[n] } else { data[n - 20] });
    assert_eq!(c.active_voices(), 1);
}

#[test]
fn a_choke_fades_the_other_pad_over_32_frames_from_the_trigger() {
    for (second_source, b) in [(1u16, 0.0), (0, 1.0)] {
        let mut c = core();
        bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 1);
        bind(&mut c, 1, second_source, 128, unity(PlayMode::OneShot, 61, 0), 1);
        let sources = [SliceSource::mono(&ONES, 48_000.0), SliceSource::mono(&ZEROS, 48_000.0)];
        let out = render(&mut c, &sources, &[on(0, 0, -1), on(16, 1, -1)], 100);
        // With the second pad silent the output is the first pad's contribution alone.
        assert_frames(&out, |n| choke_factor(n, 16) + if n >= 16 { b } else { 0.0 });
        assert_eq!(c.diagnostics().chokes, 1);
    }
}

#[test]
fn reverse_plays_the_region_from_its_end() {
    let mut c = core();
    let data = ramp();
    bind(&mut c, 0, 0, 128, crate::patch::Playback { reverse: true, ..unity(PlayMode::OneShot, 60, 0) }, 0);
    let out = render(&mut c, &[SliceSource::mono(&data, 48_000.0)], &[on(0, 0, -1)], 140);
    assert_frames(&out, |n| if n < 128 { data[127 - n] } else { 0.0 });
}

#[test]
fn sixty_four_pads_sound_at_once_and_a_65th_voice_steals_the_oldest() {
    let mut c = core();
    let mut data = vec![vec![1.0f32; 128]; PADS];
    data[0] = vec![1000.0; 128];
    let mut sources: Vec<SliceSource> = data.iter().map(|d| SliceSource::mono(d, 48_000.0)).collect();
    for p in 0..PADS {
        bind(&mut c, p, p as u16, 128, unity(PlayMode::OneShot, note(p), 0), 0);
    }
    // An unbound zone on keys 0..=59 (no pad's trigger note), its own source.
    let extra = [7.0f32; 128];
    sources.push(SliceSource::mono(&extra, 48_000.0));
    let zone = Zone { playback: unity(PlayMode::OneShot, 10, 0), keys: (0, 59), ..Zone::new(Region::whole(PADS as u16, 128)) };
    c.set_zone(PADS, Some(zone)).unwrap();
    let mut events: Vec<Event> = (0..PADS).map(|p| on(0, p, -1)).collect();
    events.push(Event::on(4, 10, 127));
    let out = render(&mut c, &sources, &events, 8);
    // Frames 0..4: pad 0's 1000 plus 63 ones; from 4 the oldest voice (pad 0, started first) is cut
    // and the zone's 7 sounds instead: no hidden extra tail voice.
    assert_frames(&out, |n| if n < 4 { 1063.0 } else { 63.0 + 7.0 });
    assert_eq!(c.active_voices(), VOICES);
    assert_eq!(c.diagnostics().steals, 1);
}

#[test]
fn simultaneous_triggers_inside_a_block_keep_their_order_and_the_last_of_a_group_wins() {
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 3);
    bind(&mut c, 1, 1, 128, unity(PlayMode::OneShot, 61, 0), 3);
    let twos = [2.0f32; 128];
    let sources = [SliceSource::mono(&ONES, 48_000.0), SliceSource::mono(&twos, 48_000.0)];
    let out = render(&mut c, &sources, &[on(37, 0, -1), on(37, 1, -1)], 100);
    assert_frames(&out, |n| if n < 37 { 0.0 } else { choke_factor(n, 37) + 2.0 });
    // The other order: the first pad wins.
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 3);
    bind(&mut c, 1, 1, 128, unity(PlayMode::OneShot, 61, 0), 3);
    let out = render(&mut c, &sources, &[on(37, 1, -1), on(37, 0, -1)], 100);
    assert_frames(&out, |n| if n < 37 { 0.0 } else { 2.0 * choke_factor(n, 37) + 1.0 });
}

#[test]
fn group_off_chokes_nothing() {
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 128, unity(PlayMode::OneShot, 61, 0), 0);
    let out = render(&mut c, &[SliceSource::mono(&ONES, 48_000.0)], &[on(0, 0, -1), on(16, 1, -1)], 64);
    assert_frames(&out, |n| if n < 16 { 1.0 } else { 2.0 });
    assert_eq!(c.diagnostics().chokes, 0);
}

#[test]
fn choke_groups_cross_banks() {
    let mut c = core();
    // Pad 3 is in bank A, pad 40 in bank C.
    bind(&mut c, 3, 0, 128, unity(PlayMode::OneShot, note(3), 0), 5);
    bind(&mut c, 40, 1, 128, unity(PlayMode::OneShot, note(40), 0), 5);
    let sources = [SliceSource::mono(&ONES, 48_000.0), SliceSource::mono(&ZEROS, 48_000.0)];
    let out = render(&mut c, &sources, &[on(0, 3, -1), on(16, 40, -1)], 64);
    assert_frames(&out, |n| choke_factor(n, 16));
}

#[test]
fn muted_and_solo_excluded_triggers_neither_sound_nor_choke() {
    let sources = [SliceSource::mono(&ONES, 48_000.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 2);
    bind(&mut c, 1, 0, 128, unity(PlayMode::OneShot, 61, 0), 2);
    c.set_pad(1, Pad { mute: true, ..*c.pad(1).unwrap() }).unwrap();
    let out = render(&mut c, &sources, &[on(0, 0, -1), on(16, 1, -1)], 64);
    assert_frames(&out, |_| 1.0);
    // Pad 2 is solo: pad 1 (unmuted again) is excluded and chokes nothing; pad 2 sounds.
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 2);
    bind(&mut c, 1, 0, 128, unity(PlayMode::OneShot, 61, 0), 2);
    bind(&mut c, 2, 0, 128, unity(PlayMode::OneShot, 62, 0), 0);
    c.set_pad(0, Pad { solo: true, ..*c.pad(0).unwrap() }).unwrap();
    c.set_pad(2, Pad { solo: true, ..*c.pad(2).unwrap() }).unwrap();
    let out = render(&mut c, &sources, &[on(0, 0, -1), on(16, 1, -1), on(16, 2, -1)], 64);
    assert_frames(&out, |n| if n < 16 { 1.0 } else { 2.0 });
    assert_eq!(c.diagnostics().chokes, 0);
}

#[test]
fn the_sustain_pedal_holds_a_gate_voice_until_it_lifts() {
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::Gate, 60, 8), 0);
    let events = [
        Event::new(0, EventKind::Control(Control::Sustain(true))),
        on(0, 0, -1),
        off(16, 0, -1),
        Event::new(40, EventKind::Control(Control::Sustain(false))),
    ];
    let out = render(&mut c, &[SliceSource::mono(&ONES, 48_000.0)], &events, 64);
    assert_frames(&out, |n| match n {
        0..40 => 1.0,
        40..48 => 1.0 - (n - 39) as f32 / 8.0,
        _ => 0.0,
    });
}

#[test]
fn all_sound_off_cuts_and_stop_fades_every_voice() {
    let sources = [SliceSource::mono(&ONES, 48_000.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 128, unity(PlayMode::Gate, 61, 8), 0);
    let out = render(&mut c, &sources, &[on(0, 0, -1), on(0, 1, -1), Event::new(30, EventKind::AllSoundOff)], 64);
    assert_frames(&out, |n| if n < 30 { 2.0 } else { 0.0 });
    assert_eq!(c.active_voices(), 0);
    let out = render(&mut c, &sources, &[on(0, 0, -1), on(0, 1, -1), Event::new(30, EventKind::Stop)], 100);
    assert_frames(&out, |n| 2.0 * choke_factor(n, 30));
    assert_eq!(c.active_voices(), 0);
}

#[test]
fn all_notes_off_releases_gate_voices_and_fades_one_shots() {
    let sources = [SliceSource::mono(&ONES, 48_000.0)];
    let mut c = core();
    bind(&mut c, 0, 0, 128, unity(PlayMode::OneShot, 60, 0), 0);
    bind(&mut c, 1, 0, 128, unity(PlayMode::Gate, 61, 8), 0);
    let out = render(&mut c, &sources, &[on(0, 0, -1), on(0, 1, -1), Event::new(30, EventKind::AllNotesOff)], 100);
    assert_frames(&out, |n| {
        let gate = match n {
            0..30 => 1.0,
            30..38 => 1.0 - (n - 29) as f32 / 8.0,
            _ => 0.0,
        };
        choke_factor(n, 30) + gate
    });
}

#[test]
fn a_pad_press_plays_until_its_release_and_a_missing_source_is_refused() {
    let mut c = core();
    bind(&mut c, 5, 0, 128, unity(PlayMode::Gate, 65, 8), 0);
    bind(&mut c, 6, 9, 128, unity(PlayMode::Gate, 66, 8), 0);
    let events = [
        Event::new(0, EventKind::PadPress { pad: 5, velocity: 127 }),
        Event::new(0, EventKind::PadPress { pad: 6, velocity: 127 }),
        Event::new(10, EventKind::PadRelease { pad: 5 }),
    ];
    let out = render(&mut c, &[SliceSource::mono(&ONES, 48_000.0)], &events, 32);
    assert_frames(&out, |n| match n {
        0..10 => 1.0,
        10..18 => 1.0 - (n - 9) as f32 / 8.0,
        _ => 0.0,
    });
    assert_eq!(c.diagnostics().refused_starts, 1);
}

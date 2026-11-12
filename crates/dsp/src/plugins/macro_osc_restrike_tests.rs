//! #292: a note-off pairs with the oldest open note-on of its key (Ableton's and Logic's
//! convention). A re-strike that arrives before the earlier note's off restarts the voice playing
//! that key; the earlier note's off, arriving after the re-strike, closes the earlier note (whose
//! voice the re-strike took over) and leaves the re-struck note sounding until its own off.
//! Exact event offsets, swept over host block sizes and both voice modes.

use super::*;

const SR: f32 = 48_000.0;
const KEY: u8 = 60;

fn instrument(voices: usize) -> MacroOscillator {
    let mut m = MacroOscillator::new();
    assert!(m.set_param("voices", (voices - 1) as f32));
    m.prepare(SR, 512, 2);
    m
}

fn render(m: &mut MacroOscillator, frames: usize, host: usize) {
    let mut io = [vec![0.0; host], vec![0.0; host]];
    for at in (0..frames).step_by(host) {
        m.process(&mut io, host.min(frames - at));
    }
}

fn sounding(m: &MacroOscillator) -> bool {
    m.slots.iter().any(|s| s.notes.any_held())
}

/// The orderings of the re-strike and the earlier note's off, as (on offset, off offset, on
/// first): one sample apart either way and the zero gap both ways round. In every one the
/// re-struck note holds until its own off.
const ORDERS: [(usize, usize, bool); 4] = [
    // The re-strike one sample before the earlier note's off (the report: 21065 then 21066).
    (5, 6, true),
    // The zero gap, the re-strike first.
    (6, 6, true),
    // The zero gap, the off first: the off ends the earlier note, the re-strike holds.
    (6, 6, false),
    // The off one sample before the re-strike.
    (6, 7, false),
];

#[test]
fn an_earlier_notes_off_after_a_restrike_leaves_the_restrike_sounding() {
    for voices in [1, 8] {
        for host in [1, 7, 12, 13, 256] {
            for (on, off, on_first) in ORDERS {
                let mut m = instrument(voices);
                m.note_on(0, KEY, 100);
                render(&mut m, 4800, host);
                if on_first {
                    m.note_on(on, KEY, 91);
                    m.note_off(off, KEY);
                } else {
                    m.note_off(off, KEY);
                    m.note_on(on.max(off), KEY, 91);
                }
                render(&mut m, 4800, host);
                assert!(sounding(&m), "voices={voices} host={host} on={on} off={off} on_first={on_first}: the re-struck note holds");
                // Its own off ends it.
                m.note_off(0, KEY);
                render(&mut m, 2 * BLOCK, host);
                assert!(!sounding(&m), "voices={voices} host={host} on={on} off={off} on_first={on_first}: the second off releases");
            }
        }
    }
}

/// The pedal keeps a released key; an earlier note's off after a re-strike changes nothing, and
/// lifting the pedal leaves the re-struck (still held) note sounding.
#[test]
fn an_earlier_notes_off_under_the_pedal_leaves_the_restrike_held() {
    for voices in [1, 8] {
        let mut m = instrument(voices);
        m.midi(0, [0xB0, CC_SUSTAIN, 127]);
        m.note_on(0, KEY, 100);
        render(&mut m, 480, 12);
        m.note_on(5, KEY, 91);
        m.note_off(6, KEY);
        m.midi(7, [0xB0, CC_SUSTAIN, 0]);
        render(&mut m, 480, 12);
        assert!(sounding(&m), "voices={voices}: the re-struck key is still down");
        m.note_off(0, KEY);
        render(&mut m, 2 * BLOCK, 12);
        assert!(!sounding(&m), "voices={voices}");
    }
}

/// Mono legato: a key struck again while another is held is the newest note again; the earlier
/// instance's off does not end it, and releasing the other key keeps it.
#[test]
fn mono_legato_keeps_a_restruck_key_through_its_earlier_off() {
    let mut m = instrument(1);
    m.note_on(0, KEY, 100);
    m.note_on(24, 64, 100);
    m.note_on(48, KEY, 100);
    m.note_off(60, KEY);
    render(&mut m, 480, 7);
    assert_eq!(m.slots[0].notes.top(), Some(KEY));
    m.note_off(0, 64);
    render(&mut m, 48, 7);
    assert_eq!(m.slots[0].notes.top(), Some(KEY));
    assert!(m.slots[0].notes.any_held());
    m.note_off(0, KEY);
    render(&mut m, 48, 7);
    assert!(!sounding(&m));
}

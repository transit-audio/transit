use super::*;
use MiddleC::{C3, C4};

#[test]
fn names() {
    assert_eq!(gm_drum_name(36), Some("Bass Drum 1"));
    assert_eq!(gm_drum_name(81), Some("Open Triangle"));
    assert_eq!(gm_drum_name(34), None);
    assert_eq!(gm_drum_name(82), None);
    assert_eq!(gm_program_name(0), "Acoustic Grand Piano");
    assert_eq!(gm_program_name(127), "Gunshot");
    assert_eq!(gm_program_name(200), "Unknown");
}

#[test]
fn the_landmarks_in_both_conventions() {
    for (pitch, c3, c4) in [(0, "C-2", "C-1"), (60, "C3", "C4"), (61, "C#3", "C#4"), (69, "A3", "A4"), (127, "G8", "G9")] {
        assert_eq!(note_name(pitch, C3), c3);
        assert_eq!(note_name(pitch, C4), c4);
        assert_eq!(parse_note(c3, C3, None), Some(pitch));
        assert_eq!(parse_note(c4, C4, None), Some(pitch));
    }
    assert_eq!((octave(60, C3), octave(60, C4)), (3, 4));
    assert_eq!((octave(0, C3), octave(0, C4)), (-2, -1));
    assert_eq!((octave(48, C3), octave(48, C4)), (2, 3));
}

#[test]
fn name_and_parse_round_trip_over_every_pitch_in_both_conventions() {
    for convention in MiddleC::ALL {
        for pitch in 0..=127u8 {
            let name = note_name(pitch, convention);
            assert_eq!(parse_note(&name, convention, None), Some(pitch), "{name} under {convention:?}");
            assert!(!name.contains('b') && !name.starts_with("B#") && !name.starts_with("Cb"), "sharps only: {name}");
        }
    }
}

#[test]
fn negative_control_c3_text_under_the_c4_argument_is_the_other_pitch() {
    // A caller that passes the wrong convention gets a different key, an octave away.
    for pitch in 12..=127u8 {
        let c3 = note_name(pitch, C3);
        assert_eq!(parse_note(&c3, C4, None), Some(pitch - 12), "{c3}");
    }
    assert_ne!(note_name(60, C3), note_name(60, C4));
}

/// Every grammar rule of ADR 0051 decision 1, each with what it accepts and what it refuses.
#[test]
fn the_parser_table() {
    let accepted: [(&str, Option<i32>, u8); 22] = [
        ("C3", None, 60),
        ("c3", None, 60),
        ("  C3\t", None, 60),
        ("C#3", None, 61),
        ("C\u{266F}3", None, 61),
        ("Db3", None, 61),
        ("D\u{266D}3", None, 61),
        ("Bb3", None, 70),
        ("bb3", None, 70),
        ("b3", None, 71),
        ("Cb3", None, 59),
        ("B#2", None, 60),
        ("C03", None, 60),
        ("C-2", None, 0),
        ("C-02", None, 0),
        ("G8", None, 127),
        ("C-0", None, 24),
        ("G", Some(3), 67),
        ("Bb", Some(3), 70),
        ("bb", Some(3), 70),
        ("F#", Some(3), 66),
        ("c", Some(1), 36),
    ];
    for (text, bare, pitch) in accepted {
        assert_eq!(parse_note(text, C3, bare), Some(pitch), "{text:?} is accepted");
    }
    let refused: [(&str, Option<i32>); 22] = [
        ("", Some(3)),
        ("   ", Some(3)),
        ("H3", None),
        ("BB3", None),
        ("C+3", None),
        ("C 3", None),
        ("C# 3", None),
        ("C3 4", None),
        ("C--1", None),
        ("C-", None),
        ("C3x", None),
        ("CB3", None),
        ("C##3", None),
        ("#C3", None),
        ("60", None),
        ("-1", None),
        ("c", None),
        ("G", None),
        ("B#8", None),
        ("Cb-2", None),
        ("C9", None),
        ("C99999999999999999999999", None),
    ];
    for (text, bare) in refused {
        assert_eq!(parse_note(text, C3, bare), None, "{text:?} is refused");
    }
    // Range: G8 is the last name under C3; G9 under C4; C-1 is 0 under C4 and C-2 is refused there.
    assert_eq!(parse_note("G#8", C3, None), None);
    assert_eq!(parse_note("G9", C4, None), Some(127));
    assert_eq!(parse_note("C-1", C4, None), Some(0));
    assert_eq!(parse_note("C-2", C4, None), None);
    // No range syntax: a dash between two names is not a note.
    assert_eq!(parse_note("C3-C4", C3, None), None);
}

#[test]
fn a_bare_unsigned_number_is_a_midi_number_only_where_the_field_takes_numbers() {
    assert_eq!(parse_note_or_number("60", C3, None), Some(60));
    assert_eq!(parse_note_or_number(" 0 ", C4, None), Some(0));
    assert_eq!(parse_note_or_number("127", C3, None), Some(127));
    assert_eq!(parse_note_or_number("007", C3, None), Some(7));
    assert_eq!(parse_note_or_number("128", C3, None), None);
    assert_eq!(parse_note_or_number("-1", C3, None), None, "signed digits are refused");
    assert_eq!(parse_note_or_number("+1", C3, None), None);
    assert_eq!(parse_note_or_number("C1", C3, None), Some(36), "a name still reads");
    assert_eq!(parse_note_or_number("C1", C4, None), Some(24));
    assert_eq!(parse_note_or_number("c", C3, None), None, "a bare letter is refused with no bare octave");
    // Negative control: the plain parser takes no numbers.
    assert_eq!(parse_note("60", C3, None), None);
}

#[test]
fn labels() {
    assert_eq!(MiddleC::default(), C3);
    assert_eq!(MiddleC::from_label("C4"), Some(C4));
    assert_eq!(MiddleC::from_label("c4"), None);
    assert_eq!(MiddleC::ALL.map(MiddleC::label), ["C3", "C4"]);
    assert_eq!(serde_json::to_value(C4).unwrap(), serde_json::json!("C4"));
    assert_eq!(serde_json::from_value::<MiddleC>(serde_json::json!("C3")).unwrap(), C3);
}

//! Note names (ADR 0051): one namer and one parser, with the octave convention passed by every
//! caller. There is no global, `static` or thread-local convention here: the UI holds the Middle C
//! preference and passes it, commands pass the fixed [`MiddleC::C3`], and a file format passes its
//! own convention as a literal.

const PITCH_CLASSES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The name given to MIDI 60 (middle C), which fixes every octave number. `C3` is what Logic
/// shows by default ("Display Middle C as": C3) and what Ableton Live shows; `C4` is scientific
/// pitch (a tuner, MusicXML, SFZ). `Default` is the application's default, C3 (ADR 0051
/// decision 2); the functions below never assume it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum MiddleC {
    #[default]
    C3,
    C4,
}

impl MiddleC {
    /// Both conventions, in menu order.
    pub const ALL: [MiddleC; 2] = [MiddleC::C3, MiddleC::C4];

    /// `"C3"` or `"C4"`: the name of MIDI 60, as the menu and `options.middle_c` spell it.
    pub fn label(self) -> &'static str {
        match self {
            MiddleC::C3 => "C3",
            MiddleC::C4 => "C4",
        }
    }

    /// The convention a label names (exactly `"C3"` or `"C4"`).
    pub fn from_label(text: &str) -> Option<MiddleC> {
        MiddleC::ALL.into_iter().find(|m| m.label() == text)
    }

    /// What is added to `pitch / 12` to get the octave number: -2 under C3, -1 under C4.
    fn offset(self) -> i32 {
        match self {
            MiddleC::C3 => -2,
            MiddleC::C4 => -1,
        }
    }
}

/// The octave number of a MIDI pitch: 60 is octave 3 under [`MiddleC::C3`] and 4 under
/// [`MiddleC::C4`]; 0 is -2 or -1.
pub fn octave(pitch: u8, convention: MiddleC) -> i32 {
    i32::from(pitch / 12) + convention.offset()
}

/// A MIDI pitch's name: sharps only, the octave after the letter, negative as `-1`. Under C3, 0
/// is `C-2`, 60 is `C3` and 127 is `G8`; under C4, 0 is `C-1`, 60 is `C4` and 127 is `G9`.
pub fn note_name(pitch: u8, convention: MiddleC) -> String {
    let class = PITCH_CLASSES.get(usize::from(pitch % 12)).copied().unwrap_or("?");
    format!("{class}{}", octave(pitch, convention))
}

/// A note name as a MIDI pitch, or `None`. After surrounding whitespace is trimmed: a letter `A`
/// to `G` in either case, always first; an optional accidental (`#`, U+266F, a lowercase `b` or
/// U+266D), so a `b` after the letter is the flat (`bb3` is B flat 3, `b3` is B3, `BB3` is
/// refused); an optional octave of ASCII digits with at most one leading `-` and no `+` (leading
/// zeros read: `C03` is `C3`); nothing else. The octave belongs to the letter and the accidental
/// moves the pitch after it: under C3, `Cb3` is 59 and `B#2` is 60. A spelling outside 0 to 127
/// is `None`. A letter with no octave means `bare_octave`, or is refused when that is `None`.
/// Nothing here reads a range, and a bare number is not a note (see [`parse_note_or_number`]).
pub fn parse_note(text: &str, convention: MiddleC, bare_octave: Option<i32>) -> Option<u8> {
    let mut chars = text.trim().chars();
    let class: i64 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (accidental, rest) = match rest.chars().next() {
        Some(c @ ('#' | '\u{266F}')) => (1, &rest[c.len_utf8()..]),
        Some(c @ ('b' | '\u{266D}')) => (-1, &rest[c.len_utf8()..]),
        _ => (0, rest),
    };
    let octave = if rest.is_empty() {
        i64::from(bare_octave?)
    } else {
        let digits = rest.strip_prefix('-').unwrap_or(rest);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // Leading zeros read; a value too long for an i64 is refused (it is far out of range).
        let magnitude = digits.parse::<i64>().ok()?;
        if rest.starts_with('-') { -magnitude } else { magnitude }
    };
    let pitch = octave.checked_sub(i64::from(convention.offset()))?.checked_mul(12)?.checked_add(class + accidental)?;
    u8::try_from(pitch).ok().filter(|p| *p <= 127)
}

/// A typed note field that also takes numbers: unsigned ASCII digits (after trimming) are a MIDI
/// number 0 to 127 (`-1` alone is refused), anything else goes to [`parse_note`]. The field then
/// applies its own range.
pub fn parse_note_or_number(text: &str, convention: MiddleC, bare_octave: Option<i32>) -> Option<u8> {
    let t = text.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse::<u8>().ok().filter(|n| *n <= 127);
    }
    parse_note(t, convention, bare_octave)
}

const GM_DRUMS: [&str; 47] = [
    "Acoustic Bass Drum",
    "Bass Drum 1",
    "Side Stick",
    "Acoustic Snare",
    "Hand Clap",
    "Electric Snare",
    "Low Floor Tom",
    "Closed Hi-Hat",
    "High Floor Tom",
    "Pedal Hi-Hat",
    "Low Tom",
    "Open Hi-Hat",
    "Low-Mid Tom",
    "Hi-Mid Tom",
    "Crash Cymbal 1",
    "High Tom",
    "Ride Cymbal 1",
    "Chinese Cymbal",
    "Ride Bell",
    "Tambourine",
    "Splash Cymbal",
    "Cowbell",
    "Crash Cymbal 2",
    "Vibraslap",
    "Ride Cymbal 2",
    "Hi Bongo",
    "Low Bongo",
    "Mute Hi Conga",
    "Open Hi Conga",
    "Low Conga",
    "High Timbale",
    "Low Timbale",
    "High Agogo",
    "Low Agogo",
    "Cabasa",
    "Maracas",
    "Short Whistle",
    "Long Whistle",
    "Short Guiro",
    "Long Guiro",
    "Claves",
    "Hi Wood Block",
    "Low Wood Block",
    "Mute Cuica",
    "Open Cuica",
    "Mute Triangle",
    "Open Triangle",
];

/// General MIDI (level 1) percussion key-map name for a pitch on channel 10 (keys 35..=81).
pub fn gm_drum_name(pitch: u8) -> Option<&'static str> {
    let idx = usize::from(pitch).checked_sub(35)?;
    GM_DRUMS.get(idx).copied()
}

const GM_PROGRAMS: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavi",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "SynthStrings 1",
    "SynthStrings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "SynthBrass 1",
    "SynthBrass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bag pipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// General MIDI (level 1) instrument name for a 0-based program number. Values above 127
/// return `"Unknown"`.
pub fn gm_program_name(program: u8) -> &'static str {
    GM_PROGRAMS.get(usize::from(program)).copied().unwrap_or("Unknown")
}

#[cfg(test)]
mod tests;

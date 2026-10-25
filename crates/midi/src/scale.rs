//! Musical key and scale shared by note editing and piano-roll views.

macro_rules! scales {
    ($($variant:ident, $id:literal, $name:literal, [$($step:literal),+];)+) => {
        /// A named scale: a set of pitch classes above the tonic, stored as a 12-bit mask
        /// (bit `n` set means the tone `n` semitones above the root belongs to the scale).
        /// The list and order follow Ableton Live 12 and Logic Pro; Minor is natural minor.
        /// Sessions store the kebab-case id, so `major` and `minor` from older sessions load as is.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub enum Scale {
            #[default]
            $(#[serde(rename = $id)] $variant,)+
        }

        impl Scale {
            /// Every scale in menu order.
            pub const ALL: &'static [Scale] = &[$(Scale::$variant),+];

            /// Display name, such as "Minor Pentatonic".
            pub const fn name(self) -> &'static str {
                match self { $(Scale::$variant => $name,)+ }
            }

            /// Stable session and MCP id, such as "minor-pentatonic".
            pub const fn id(self) -> &'static str {
                match self { $(Scale::$variant => $id,)+ }
            }

            /// Interval mask, bit 0 being the root.
            pub const fn mask(self) -> u16 {
                match self { $(Scale::$variant => 0 $(| 1u16 << $step)+,)+ }
            }
        }
    };
}

scales! {
    Major, "major", "Major", [0, 2, 4, 5, 7, 9, 11];
    Minor, "minor", "Minor", [0, 2, 3, 5, 7, 8, 10];
    Dorian, "dorian", "Dorian", [0, 2, 3, 5, 7, 9, 10];
    Mixolydian, "mixolydian", "Mixolydian", [0, 2, 4, 5, 7, 9, 10];
    Lydian, "lydian", "Lydian", [0, 2, 4, 6, 7, 9, 11];
    Phrygian, "phrygian", "Phrygian", [0, 1, 3, 5, 7, 8, 10];
    Locrian, "locrian", "Locrian", [0, 1, 3, 5, 6, 8, 10];
    WholeTone, "whole-tone", "Whole Tone", [0, 2, 4, 6, 8, 10];
    HalfWholeDim, "half-whole-dim", "Half-whole Dim.", [0, 1, 3, 4, 6, 7, 9, 10];
    WholeHalfDim, "whole-half-dim", "Whole-half Dim.", [0, 2, 3, 5, 6, 8, 9, 11];
    MinorBlues, "minor-blues", "Minor Blues", [0, 3, 5, 6, 7, 10];
    MinorPentatonic, "minor-pentatonic", "Minor Pentatonic", [0, 3, 5, 7, 10];
    MajorPentatonic, "major-pentatonic", "Major Pentatonic", [0, 2, 4, 7, 9];
    HarmonicMinor, "harmonic-minor", "Harmonic Minor", [0, 2, 3, 5, 7, 8, 11];
    HarmonicMajor, "harmonic-major", "Harmonic Major", [0, 2, 4, 5, 7, 8, 11];
    DorianSharp4, "dorian-sharp4", "Dorian #4", [0, 2, 3, 6, 7, 9, 10];
    PhrygianDominant, "phrygian-dominant", "Phrygian Dominant", [0, 1, 4, 5, 7, 8, 10];
    MelodicMinor, "melodic-minor", "Melodic Minor", [0, 2, 3, 5, 7, 9, 11];
    LydianAugmented, "lydian-augmented", "Lydian Augmented", [0, 2, 4, 6, 8, 9, 11];
    LydianDominant, "lydian-dominant", "Lydian Dominant", [0, 2, 4, 6, 7, 9, 10];
    SuperLocrian, "super-locrian", "Super Locrian", [0, 1, 3, 4, 6, 8, 10];
    EightToneSpanish, "eight-tone-spanish", "8-Tone Spanish", [0, 1, 3, 4, 5, 6, 8, 10];
    Bhairav, "bhairav", "Bhairav", [0, 1, 4, 5, 7, 8, 11];
    HungarianMinor, "hungarian-minor", "Hungarian Minor", [0, 2, 3, 6, 7, 8, 11];
    Hirajoshi, "hirajoshi", "Hirajoshi", [0, 2, 3, 7, 8];
    InSen, "in-sen", "In-Sen", [0, 1, 5, 7, 10];
    Iwato, "iwato", "Iwato", [0, 1, 5, 6, 10];
    Kumoi, "kumoi", "Kumoi", [0, 2, 3, 7, 9];
    PelogSelisir, "pelog-selisir", "Pelog Selisir", [0, 1, 3, 7, 8];
    PelogTembung, "pelog-tembung", "Pelog Tembung", [0, 1, 5, 7, 8];
    Messiaen1, "messiaen-1", "Messiaen 1", [0, 2, 4, 6, 8, 10];
    Messiaen2, "messiaen-2", "Messiaen 2", [0, 1, 3, 4, 6, 7, 9, 10];
    Messiaen3, "messiaen-3", "Messiaen 3", [0, 2, 3, 4, 6, 7, 8, 10, 11];
    Messiaen4, "messiaen-4", "Messiaen 4", [0, 1, 2, 5, 6, 7, 8, 11];
    Messiaen5, "messiaen-5", "Messiaen 5", [0, 1, 5, 6, 7, 11];
    Messiaen6, "messiaen-6", "Messiaen 6", [0, 2, 4, 5, 6, 8, 10, 11];
    Messiaen7, "messiaen-7", "Messiaen 7", [0, 1, 2, 3, 5, 6, 7, 8, 9, 11];
}

impl Scale {
    /// Number of tones per octave.
    pub const fn tone_count(self) -> u32 {
        self.mask().count_ones()
    }

    /// Whether `semitones` above the root (modulo 12) is a scale tone.
    pub const fn has(self, semitones: u8) -> bool {
        (self.mask() >> (semitones % 12)) & 1 == 1
    }

    /// Look up by display name or id, ignoring case.
    pub fn parse(text: &str) -> Option<Scale> {
        let text = text.trim();
        Scale::ALL.iter().copied().find(|s| s.id().eq_ignore_ascii_case(text) || s.name().eq_ignore_ascii_case(text))
    }
}

/// Root names for key text; accidentals are written as sharps.
pub const ROOT_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// A tonic pitch class (C = 0) and scale. Defaults to C major.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct KeyScale {
    pub root: u8,
    pub scale: Scale,
}

/// Per-clip editing aid. Never applied to MIDI playback or live input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ScaleMode {
    pub enabled: bool,
    pub fold: bool,
    pub key: KeyScale,
}

impl KeyScale {
    /// Move one pitch by scale degrees; a one-note group (see `transpose_group`).
    pub fn transpose(self, pitch: u8, degrees: i64) -> u8 {
        self.transpose_group(&[pitch], pitch, degrees)[0]
    }

    /// Move a chord by scale degrees as one group, so no two notes fold onto one pitch.
    ///
    /// Each note is a scale tone (the highest tone at or below it) plus a chromatic offset;
    /// every note moves the same number of tones and keeps its offset, so a C# in C minor
    /// moved up a degree becomes D#. If that would collide two notes (an accidental landing
    /// on the next tone because the new gap is one semitone, such as D# and E in C major) the
    /// whole group instead shifts by the semitone step of `anchor` (the grabbed note, or the
    /// lowest one), which preserves every interval exactly but leaves the scale. The degree
    /// count shrinks toward zero until the whole group fits 0..=127, like `clamp_move`; when
    /// no step fits, the group is returned unchanged (refused as a group).
    pub fn transpose_group(self, pitches: &[u8], anchor: u8, degrees: i64) -> Vec<u8> {
        let tones: Vec<u8> = (0..=127).filter(|&p| self.contains(p)).collect();
        let Some(&last) = tones.last() else { return pitches.to_vec() };
        let split = |pitch: u8| {
            let index = tones.partition_point(|&t| t <= pitch).saturating_sub(1);
            (index as i64, i64::from(pitch) - i64::from(tones[index]))
        };
        let in_range = |p: i64| (0..=127).contains(&p);
        let parts: Vec<(i64, i64)> = pitches.iter().map(|&p| split(p)).collect();
        let (anchor_index, _) = split(anchor);
        let mut d = degrees.clamp(-(last as i64) - 128, last as i64 + 128);
        loop {
            if d == 0 {
                return pitches.to_vec();
            }
            let at = |i: i64| usize::try_from(i).ok().and_then(|i| tones.get(i)).map(|&t| i64::from(t));
            let diatonic: Option<Vec<i64>> = parts.iter().map(|&(i, o)| at(i + d).map(|t| t + o)).collect();
            if let Some(out) = diatonic.filter(|out| out.iter().all(|&p| in_range(p))) {
                let mut sorted = out.clone();
                sorted.sort_unstable();
                if sorted.windows(2).all(|w| w[0] != w[1]) {
                    return out.into_iter().map(|p| p as u8).collect();
                }
                if let Some(target) = at(anchor_index + d) {
                    let step = target - i64::from(tones[anchor_index as usize]);
                    if pitches.iter().all(|&p| in_range(i64::from(p) + step)) {
                        return pitches.iter().map(|&p| (i64::from(p) + step) as u8).collect();
                    }
                }
            }
            d -= d.signum();
        }
    }

    /// Whether a MIDI pitch belongs to this key. Tonic values normalize modulo 12.
    pub fn contains(self, pitch: u8) -> bool {
        let degree = (pitch % 12 + 12 - self.root % 12) % 12;
        self.scale.has(degree)
    }

    /// Key text such as "C Minor Pentatonic".
    pub fn label(self) -> String {
        format!("{} {}", ROOT_NAMES[usize::from(self.root % 12)], self.scale.name())
    }

    /// Closest scale tone in MIDI range; equal distances choose the lower pitch.
    pub fn nearest(self, pitch: u8) -> u8 {
        (0..=127).filter(|&candidate| self.contains(candidate)).min_by_key(|&candidate| candidate.abs_diff(pitch)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_minor_nearest_ties_and_boundaries() {
        let c = KeyScale::default();
        assert_eq!((0..12).filter(|&p| c.contains(p)).collect::<Vec<_>>(), [0, 2, 4, 5, 7, 9, 11]);
        assert_eq!([61, 63, 66, 68, 70].map(|p| c.nearest(p)), [60, 62, 65, 67, 69]);
        let a = KeyScale { root: 9, scale: Scale::Minor };
        assert_eq!((0..12).filter(|&p| a.contains(p)).collect::<Vec<_>>(), [0, 2, 4, 5, 7, 9, 11]);
        let cm = KeyScale { root: 0, scale: Scale::Minor };
        assert_eq!([61, 64, 66, 69, 71].map(|p| cm.nearest(p)), [60, 63, 65, 68, 70]);
        let d = KeyScale { root: 1, scale: Scale::Major };
        assert_eq!(d.nearest(127), 126);
        assert_eq!(KeyScale { root: 2, scale: Scale::Major }.nearest(0), 1);
        for root in 0..=255 {
            for scale in [Scale::Major, Scale::Minor, Scale::MinorPentatonic, Scale::WholeTone, Scale::HalfWholeDim] {
                let key = KeyScale { root, scale };
                for pitch in 0..=127 {
                    let nearest = key.nearest(pitch);
                    assert!(nearest <= 127 && key.contains(nearest));
                    assert!((0..=127).filter(|&p| key.contains(p)).all(|p| p.abs_diff(pitch) >= nearest.abs_diff(pitch)));
                }
            }
        }
    }

    #[test]
    fn group_transpose_keeps_chords_distinct_and_in_range() {
        let cm = KeyScale { root: 0, scale: Scale::Minor };
        assert_eq!(cm.transpose_group(&[60, 61], 60, 1), [62, 63]);
        assert_eq!(cm.transpose(61, 1), 63);
        let c = KeyScale::default();
        assert_eq!(c.transpose_group(&[124, 125, 127], 124, 1), [124, 125, 127]);
        assert_eq!(c.transpose_group(&[60, 64, 67], 60, 1), [62, 65, 69]);
        assert_eq!(c.transpose_group(&[60, 64, 67], 60, -1), [59, 62, 65]);
        // D# and E: the accidental would land on F with E; the group shifts by the anchor step.
        assert_eq!(c.transpose_group(&[63, 64], 63, 1), [65, 66]);
        // Top of range: the degree count shrinks until the whole group fits.
        assert_eq!(c.transpose_group(&[120, 127], 120, 3), [120, 127]);
        assert_eq!(c.transpose_group(&[117, 119], 117, 3), [122, 124]);
    }

    #[test]
    fn group_transpose_never_equates_two_pitches() {
        for root in 0..12 {
            for scale in [Scale::Major, Scale::Minor, Scale::MinorPentatonic, Scale::WholeTone, Scale::HalfWholeDim] {
                let key = KeyScale { root, scale };
                for a in 0..=127u8 {
                    for b in (a + 1)..=127u8 {
                        for degrees in [-1, 1] {
                            for anchor in [a, b] {
                                let out = key.transpose_group(&[a, b], anchor, degrees);
                                assert_ne!(out[0], out[1], "{root} {scale:?} {a} {b} {degrees}");
                                assert!(out.iter().all(|&p| p <= 127));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Reference table written as semitone gaps (the way theory books list scales), checked
    /// against the interval masks the model stores.
    const GAPS: [(Scale, &str, &str); 37] = [
        (Scale::Major, "Major", "2212221"),
        (Scale::Minor, "Minor", "2122122"),
        (Scale::Dorian, "Dorian", "2122212"),
        (Scale::Mixolydian, "Mixolydian", "2212212"),
        (Scale::Lydian, "Lydian", "2221221"),
        (Scale::Phrygian, "Phrygian", "1222122"),
        (Scale::Locrian, "Locrian", "1221222"),
        (Scale::WholeTone, "Whole Tone", "222222"),
        (Scale::HalfWholeDim, "Half-whole Dim.", "12121212"),
        (Scale::WholeHalfDim, "Whole-half Dim.", "21212121"),
        (Scale::MinorBlues, "Minor Blues", "321132"),
        (Scale::MinorPentatonic, "Minor Pentatonic", "32232"),
        (Scale::MajorPentatonic, "Major Pentatonic", "22323"),
        (Scale::HarmonicMinor, "Harmonic Minor", "2122131"),
        (Scale::HarmonicMajor, "Harmonic Major", "2212131"),
        (Scale::DorianSharp4, "Dorian #4", "2131212"),
        (Scale::PhrygianDominant, "Phrygian Dominant", "1312122"),
        (Scale::MelodicMinor, "Melodic Minor", "2122221"),
        (Scale::LydianAugmented, "Lydian Augmented", "2222121"),
        (Scale::LydianDominant, "Lydian Dominant", "2221212"),
        (Scale::SuperLocrian, "Super Locrian", "1212222"),
        (Scale::EightToneSpanish, "8-Tone Spanish", "12111222"),
        (Scale::Bhairav, "Bhairav", "1312131"),
        (Scale::HungarianMinor, "Hungarian Minor", "2131131"),
        (Scale::Hirajoshi, "Hirajoshi", "21414"),
        (Scale::InSen, "In-Sen", "14232"),
        (Scale::Iwato, "Iwato", "14142"),
        (Scale::Kumoi, "Kumoi", "21423"),
        (Scale::PelogSelisir, "Pelog Selisir", "12414"),
        (Scale::PelogTembung, "Pelog Tembung", "14214"),
        (Scale::Messiaen1, "Messiaen 1", "222222"),
        (Scale::Messiaen2, "Messiaen 2", "12121212"),
        (Scale::Messiaen3, "Messiaen 3", "211211211"),
        (Scale::Messiaen4, "Messiaen 4", "11311131"),
        (Scale::Messiaen5, "Messiaen 5", "141141"),
        (Scale::Messiaen6, "Messiaen 6", "22112211"),
        (Scale::Messiaen7, "Messiaen 7", "1112111121"),
    ];

    fn tones(scale: Scale) -> Vec<u8> {
        (0..12).filter(|&n| scale.has(n)).collect()
    }

    #[test]
    fn scale_interval_sets_match_reference_table_for_every_scale() {
        assert_eq!(Scale::ALL.len(), GAPS.len());
        for (i, (scale, name, gaps)) in GAPS.iter().enumerate() {
            assert_eq!(Scale::ALL[i], *scale, "menu order at {i}");
            assert_eq!(scale.name(), *name);
            let mut from_gaps = vec![0u8];
            for g in gaps.chars() {
                let next = from_gaps.last().unwrap() + g.to_digit(10).unwrap() as u8;
                from_gaps.push(next);
            }
            assert_eq!(from_gaps.pop(), Some(12), "{name} gaps close the octave");
            assert_eq!(tones(*scale), from_gaps, "{name}");
            assert_eq!(scale.tone_count() as usize, from_gaps.len());
            assert_eq!(scale.mask() & !0xFFF, 0);
        }
        assert_eq!(Scale::MinorPentatonic.tone_count(), 5);
        assert_eq!(Scale::WholeTone.tone_count(), 6);
        assert_eq!(Scale::HalfWholeDim.tone_count(), 8);
    }

    #[test]
    fn scale_ids_are_unique_kebab_case_and_round_trip_through_serde() {
        for scale in Scale::ALL {
            let json = serde_json::to_string(scale).unwrap();
            assert_eq!(json, format!("\"{}\"", scale.id()));
            assert_eq!(serde_json::from_str::<Scale>(&json).unwrap(), *scale);
            assert_eq!(Scale::parse(scale.name()), Some(*scale));
            assert_eq!(Scale::ALL.iter().filter(|s| s.id() == scale.id()).count(), 1);
        }
        assert_eq!(serde_json::from_str::<Scale>("\"major\"").unwrap(), Scale::Major);
        assert_eq!(serde_json::from_str::<Scale>("\"minor\"").unwrap(), Scale::Minor);
        assert!(serde_json::from_str::<Scale>("\"nope\"").is_err());
        assert_eq!(KeyScale { root: 0, scale: Scale::MinorPentatonic }.label(), "C Minor Pentatonic");
    }

    #[test]
    fn constrain_and_transpose_on_five_six_and_eight_note_scales() {
        // C minor pentatonic: C Eb F G Bb; ties go down.
        let pent = KeyScale { root: 0, scale: Scale::MinorPentatonic };
        assert_eq!([61, 62, 64, 66, 69, 71].map(|p| pent.nearest(p)), [60, 63, 63, 65, 70, 70]);
        // A degree is one scale tone; five tones per octave, so five degrees is an octave.
        assert_eq!(pent.transpose(60, 1), 63);
        assert_eq!(pent.transpose(70, 1), 72);
        assert_eq!(pent.transpose(60, 5), 72);
        assert_eq!(pent.transpose(60, -1), 58);
        assert_eq!(pent.transpose_group(&[60, 67], 60, 1), [63, 70]);
        // C whole tone: six tones, a degree is a whole step.
        let whole = KeyScale { root: 0, scale: Scale::WholeTone };
        assert_eq!([61, 63, 65].map(|p| whole.nearest(p)), [60, 62, 64]);
        assert_eq!(whole.transpose(60, 1), 62);
        assert_eq!(whole.transpose(60, 6), 72);
        assert_eq!(whole.transpose(70, 1), 72);
        assert_eq!(whole.transpose_group(&[60, 64], 60, 2), [64, 68]);
        // C half-whole diminished: eight tones, alternating semitone and whole step.
        let dim = KeyScale { root: 0, scale: Scale::HalfWholeDim };
        assert_eq!([62, 65, 68, 71].map(|p| dim.nearest(p)), [61, 64, 67, 70]);
        assert_eq!(dim.transpose(60, 1), 61);
        assert_eq!(dim.transpose(60, 2), 63);
        assert_eq!(dim.transpose(60, 8), 72);
        assert_eq!(dim.transpose(60, -1), 58);
        assert_eq!(dim.transpose_group(&[60, 63], 60, 1), [61, 64]);
        // The root shifts the whole mask.
        let a_pent = KeyScale { root: 9, scale: Scale::MinorPentatonic };
        assert_eq!((0..12).filter(|&p| a_pent.contains(p)).collect::<Vec<_>>(), [0, 2, 4, 7, 9]);
    }
}

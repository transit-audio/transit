//! Musical key and scale shared by note editing and piano-roll views.

/// Seven-note scales. Minor means natural minor (Aeolian).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    #[default]
    Major,
    Minor,
}

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
        match self.scale {
            Scale::Major => [0, 2, 4, 5, 7, 9, 11].contains(&degree),
            Scale::Minor => [0, 2, 3, 5, 7, 8, 10].contains(&degree),
        }
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
            for scale in [Scale::Major, Scale::Minor] {
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
            for scale in [Scale::Major, Scale::Minor] {
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
}

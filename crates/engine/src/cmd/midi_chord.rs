//! Chord spelling for the MIDI command channel (control thread only).

use super::*;

pub(super) fn pitches(p: &Value) -> Result<Vec<u8>> {
    let cmd = "midi.chord_add";
    let root = p.get("root").ok_or_else(|| bad(cmd, "root is required (MIDI number or note name, e.g. G3)"))?;
    let root = match (root.as_i64(), root.as_str()) {
        (Some(n), _) if (0..=127).contains(&n) => n,
        (_, Some(name)) => note_number(name).ok_or_else(|| {
            bad(cmd, "invalid root; use MIDI 0..127 or a note name (C3 = 60 regardless of the Middle C setting, bare names use octave 3)")
        })?,
        _ => return Err(bad(cmd, "root must be a MIDI number 0..127 or note name")),
    };
    let mut intervals: Vec<i64> = match (p.get("quality"), p.get("notes")) {
        (Some(quality), None) => match quality.as_str() {
            Some("maj") => vec![0, 4, 7],
            Some("min") => vec![0, 3, 7],
            Some("dim") => vec![0, 3, 6],
            Some("aug") => vec![0, 4, 8],
            Some("maj7") => vec![0, 4, 7, 11],
            Some("min7") => vec![0, 3, 7, 10],
            Some("dom7") => vec![0, 4, 7, 10],
            Some("sus2") => vec![0, 2, 7],
            Some("sus4") => vec![0, 5, 7],
            _ => return Err(bad(cmd, "quality must be maj|min|dim|aug|maj7|min7|dom7|sus2|sus4")),
        },
        (None, Some(notes)) => {
            let notes =
                notes.as_array().filter(|a| !a.is_empty() && a.len() <= 32).ok_or_else(|| bad(cmd, "notes must contain 1..32 semitone intervals"))?;
            notes
                .iter()
                .map(|v| v.as_i64().filter(|n| (-127..=127).contains(n)).ok_or_else(|| bad(cmd, "intervals must be integers in -127..127")))
                .collect::<Result<_>>()?
        }
        _ => return Err(bad(cmd, "provide exactly one of quality or notes (semitone intervals from root)")),
    };
    intervals.sort_unstable();
    intervals.dedup();
    let inversion = match p.get("inversion") {
        None => 0,
        Some(v) => v
            .as_u64()
            .filter(|n| *n < intervals.len() as u64)
            .ok_or_else(|| bad(cmd, "inversion must be 0 through the number of distinct notes minus 1"))? as usize,
    };
    // Raise the lowest N chord tones by one octave, then sort the resulting voicing.
    for interval in intervals.iter_mut().take(inversion) {
        *interval += 12;
    }
    let mut pitches = intervals
        .into_iter()
        .map(|interval| {
            u8::try_from(root + interval).ok().filter(|n| *n <= 127).ok_or_else(|| bad(cmd, "chord voicing falls outside MIDI pitches 0..127"))
        })
        .collect::<Result<Vec<_>>>()?;
    pitches.sort_unstable();
    pitches.dedup();
    Ok(pitches)
}

/// A root's note name through the shared parser (ADR 0051 decision 5): the fixed convention
/// C3 = 60, whatever the Middle C setting, and a bare letter is octave 3.
fn note_number(name: &str) -> Option<i64> {
    transit_midi::parse_note(name, transit_midi::MiddleC::C3, Some(3)).map(i64::from)
}

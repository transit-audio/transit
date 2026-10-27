//! Onset of a sound on the audio return, by sample position, for the MIDI-to-audio onset latency
//! (`--midi-roundtrip`, `--placement`).
//!
//! The detector lives in `transit_playback::onset`, which the app's External Instrument
//! Measure (ADR 0020 decision 3) uses too, so the bench and the app time a synth one way. This
//! module re-exports it and says its refusals in the bench's words (its command-line options).

pub use transit_playback::onset::*;

/// The level an onset's envelope must exceed over the `reference` floor, or why that floor is too
/// loud to measure over.
pub fn threshold(reference: Floor) -> Result<f32, String> {
    transit_playback::onset::threshold(reference).map_err(|loud| {
        format!(
            "the return is not quiet with no note playing (envelope peak {}, above {}): a drone, a held note or the wrong input; check --in",
            dbfs(loud.peak),
            dbfs(MAX_FLOOR)
        )
    })
}

/// Whether the window before a send (`pre`) is quiet enough to measure over: a window louder
/// than [`MAX_FLOOR`] is a release still sounding (or something else playing). A window merely
/// above the floor is measured with a raised threshold and a warning (`crate::midi_roundtrip`).
pub fn not_sounding(pre: Floor) -> Result<(), String> {
    transit_playback::onset::not_sounding(pre).map_err(|loud| {
        format!(
            "the return before the note is above the noise floor (envelope peak {}, above {}): the previous note's release is still sounding or something else plays; use the synth's shortest release, raise --gap-ms",
            dbfs(loud.peak),
            dbfs(MAX_FLOOR)
        )
    })
}

/// The threshold and the refinement's guard of a send whose pre-send window peaked at `pre`, over
/// the noise floor `floor` ([`transit_playback::onset::levels`]), with the bench's refusal text.
pub fn levels(floor: f32, pre: f32) -> Result<(f32, f32), String> {
    threshold(Floor { peak: floor, rms: 0.0 })?;
    transit_playback::onset::levels(floor, pre).map_err(|_| "the noise floor is too loud".to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_refusals_name_the_bench_options() {
        let loud = threshold(Floor { peak: 0.3, rms: 0.0 }).unwrap_err();
        assert!(loud.contains("not quiet") && loud.contains("--in"), "{loud}");
        let ringing = not_sounding(Floor { peak: 0.4, rms: 0.0 }).unwrap_err();
        assert!(ringing.contains("above the noise floor") && ringing.contains("--gap-ms"), "{ringing}");
        assert!(levels(0.3, 0.0).unwrap_err().contains("not quiet"));
        assert_eq!(levels(0.0005, 0.004).unwrap(), (0.016, 0.008), "the shared detector's levels");
        assert_eq!(threshold(Floor { peak: 0.0, rms: 0.0 }).unwrap(), MIN_THRESHOLD);
    }
}

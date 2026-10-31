//! The Random Sequencer MIDI effect's controls in the MIDI effects card (#202): the help entry of
//! each control and the credit line under them (ADR 0033: credit in the device's help and panel).
//! The card draws its parameters in sections by their group (Rhythm, Pitch, Loop, Velocity, Seed),
//! the first open, as the Macro Oscillator's matrix opens from its line.

use crate::help::Id;

/// The Random Sequencer's id (#202).
pub const RANDOM_SEQUENCER: &str = transit_dsp::midi_fx::random_sequencer::ID;

/// The credit line under the controls.
pub const CREDIT: &str = transit_dsp::midi_fx::random_sequencer::CREDIT;

/// The help entry of parameter `id` (the credits' entry for an unknown id).
#[must_use]
pub fn param_help(id: &str) -> Id {
    match id {
        "rate" => Id::RandomSequencerRate,
        "model" => Id::RandomSequencerModel,
        "gate_bias" => Id::RandomSequencerGateBias,
        "jitter" => Id::RandomSequencerJitter,
        "gate_length" => Id::RandomSequencerGateLength,
        "length_random" => Id::RandomSequencerLengthRandom,
        "play" => Id::RandomSequencerPlay,
        "spread" => Id::RandomSequencerSpread,
        "bias" => Id::RandomSequencerBias,
        "steps" => Id::RandomSequencerSteps,
        "scale" => Id::RandomSequencerScale,
        "root" => Id::RandomSequencerRoot,
        "octave" => Id::RandomSequencerOctave,
        "range" => Id::RandomSequencerRange,
        "pitch_mode" => Id::RandomSequencerPitchMode,
        "deja_vu" => Id::RandomSequencerDejaVu,
        "loop_length" => Id::RandomSequencerLoopLength,
        "freeze" => Id::RandomSequencerFreeze,
        "deja_vu_on" => Id::RandomSequencerDejaVuOn,
        "velocity" => Id::RandomSequencerVelocity,
        "dynamics" => Id::RandomSequencerDynamics,
        "seed" => Id::RandomSequencerSeed,
        "seed_latch" => Id::RandomSequencerSeedLatch,
        "thru" => Id::RandomSequencerThru,
        _ => Id::RandomSequencerCredits,
    }
}

/// The credit line's help entry.
pub const CREDIT_HELP: Id = Id::RandomSequencerCredits;

/// The credit line of MIDI effect `plugin`, if it has one.
#[must_use]
pub fn credit_of(plugin: &str) -> Option<&'static str> {
    (plugin == RANDOM_SEQUENCER).then_some(CREDIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_has_its_own_help_entry() {
        let info = transit_dsp::midi_fx::info(RANDOM_SEQUENCER).unwrap();
        let mut seen = std::collections::HashSet::new();
        for p in info.params {
            let id = param_help(p.id);
            assert_ne!(id, Id::RandomSequencerCredits, "{}", p.id);
            assert!(seen.insert(id), "{} shares an entry", p.id);
            assert_eq!(id.key(), format!("random_sequencer.{}", p.id));
        }
        assert!(credit_of(RANDOM_SEQUENCER).is_some_and(|c| c.contains("Émilie Gillet") && c.contains("MIT")));
        assert!(credit_of("midi_transpose").is_none());
    }
}

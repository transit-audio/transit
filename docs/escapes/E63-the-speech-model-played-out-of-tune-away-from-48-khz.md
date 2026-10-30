# E63: the Speech model played out of tune away from 48 kHz

1. **Failure.** Found in review of #195 slice 3 (claude-review-195-3, finding 1), before any build
   shipped the instrument. The macro oscillator's LPC speech synth (the Speech model when Harmonics
   is above 1/3, blended with SAM between 1/6 and 1/3) played 1.47 semitones flat at 44.1 kHz,
   10.5 semitones sharp at 88.2 kHz and an octave sharp at 96 kHz, formants moved by the same factor.
   48 kHz was right. The defect reached main with slice 2 (chain BW, `Voice::new(SampleRate)`).
2. **Mechanism.** `LpcSpeechSynthController::render`
   (`crates/macro-osc/src/speech/lpc_speech_synth_controller.rs`) advanced the synth's clock by
   `rate = rate_ratio / 6.0` per output sample, the firmware's step for 48 kHz (an 8 kHz clock), at
   every rate. Slice 2 scaled the voice's `a0` and the corrected rate by the sample rate, so the
   pitch shift per LPC sample stayed the 48 kHz one while the LPC clock ran at `rate / 6`: the
   sounding pitch scaled by `rate / 48000`. Word timing was right (`remaining_frame_samples` uses the
   rate), which made the error sound like a transposition, not a bug.
3. **Why checks missed it.** Slice 2's `six_op_and_speech_keep_their_48_khz_pitch_at_96_khz`
   (`tests/voice_sample_rates.rs`) searched the autocorrelation peak within 1.19x of one 48 kHz
   period. A signal an octave up has a peak at that lag too (two of its periods), so the test passed
   at 96 kHz and reported -0.86 cents. 44.1 kHz was never checked for LPC. The null tests run at
   48 kHz only (the oracle's rate), where the step is correct.
4. **Regression test.** The clock step is scaled by `48000 / rate` (exactly 1 at 48 kHz, so the
   null tests are unchanged). `tests/voice_pitch.rs` measures the absolute pitch of every pitch path
   at 44.1, 48, 88.2 and 96 kHz under both tunings: the common path (eight engines that read `a0`),
   the six-op voices' own reference and the LPC synth's own clock, each within 1 cent. The
   measurement refuses aliases: the autocorrelation peak must be a real periodicity within 3% of the
   expected period and must not repeat at a half or a third of it, and the frequency is read from
   the spectrum (a DFT peak, no lag grid). `the_measurement_refuses_an_octave_alias` proves the check
   fails a 440 Hz signal tested as 220 Hz. Run against the unfixed step, the new test fails LPC
   speech at 44.1 kHz, the first rate it measures (no periodicity at the expected period:
   normalized correlation 0.18 against the 0.3 floor, 2026-10-30); with the fix, LPC speech measures within 0.001 cent of
   440 Hz at every rate.
5. **Gate class.** A pitch test compares against the expected absolute frequency with a measurement
   that rejects octave and lag-multiple aliases, and covers every engine with its own pitch path, at
   every supported rate, not a ratio to another rate.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`
   (Oracle null tests). Owner: the coordinator. Blind spot: a pitch path that only engages under a
   trigger or a parameter range the free-running renders do not reach (the speech word banks, the
   chord and string-machine voicings) is covered by reading the code, not by a measurement.

See also: [E46](E46-reopen-state-test-never-ran.md), another test that passed without testing its claim.

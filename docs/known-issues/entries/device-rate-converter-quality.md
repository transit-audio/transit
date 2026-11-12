<!-- transit-index
status: Open, measured
follow-up: Run the audio device at the session's rate (the converter is then bit-exact pass-through). A band-limited converter replacing linear interpolation is a later decision; the figures are pinned by `the_linear_converter_quality_is_measured_and_pinned` in `crates/playback`.
-->

Device-rate conversion is linear interpolation with no anti-alias filter. When the device runs at a rate other than the session's (the device does not offer the session's rate), or playback runs at a speed other than 1, the high treble droops and images and aliases are only weakly rejected. Measured with -6 dBFS tones: 44.1 kHz on a 48 kHz device, -0.015 dB at 1 kHz with images 62 dB down, -1.5 dB at 10 kHz (images 20 dB down), -6.3 dB at 20 kHz; 48 kHz on a 44.1 kHz device, a 23 kHz tone folds to 21.1 kHz only 7 dB down; 96 kHz on a 48 kHz device picks every other frame, so content above 24 kHz folds back at full level. The full table is in the [#525 escape record](../../escapes/issue-525.md). The defect that held one sample per block (#525) is fixed; these figures are the converter's structural limit.

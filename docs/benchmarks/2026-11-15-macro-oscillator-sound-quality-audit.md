# Macro Oscillator sound-quality audit (#716, first slice)

- **Date:** 2026-11-15.
- **Base:** `6ba9d1e62739f0bd9de845f8e9edae46d2e8f489`, branch `audit/716-macro-osc-quality`.
- **Question:** What signal-quality limits are observable through the played instrument before any rendering changes, including the modulation changes planned in #715?
- **Scope:** Offline measurements through `Plugin::note_on` and `Plugin::process`. No audio or MIDI device, application, listening test, product change, or decision.
- **Program:** [`macro_osc_audit.rs`](../../crates/dsp/examples/macro_osc_audit.rs).

## Method

The program output below specifies the settings, capture windows, normalization, exclusions, and methods. Each capture constructs a fresh instrument with the same seeds. Calls contain 128 frames of stereo floating-point audio; analysis uses the left channel. Default output is dual mono, and the pan experiment explicitly measures the left channel. Notes use velocity 127, the default level is -6 dB, and eight voices are available. Middle C is MIDI 60, C4. The keyboard sweep is MIDI 24, 48, 72, 96, and 127.

The analyzer uses `transit_math::fft` and portable double-precision functions, following the power convention of the converter measurements in `crates/playback/src/player/audio/tests/converter.rs` and the explicit measurement/refinement approach of `crates/resample/tests/acceptance.rs`. Unlike a synthetic converter input, the instrument cannot generally be made coherent with an FFT window. A periodic four-term Blackman-Harris window limits leakage. Frequency-bin spacing is not resolving power: its main lobe spans about eight bins. Synthetic calibration and capture/mask sensitivity measurements accompany the instrument results.

1. **Residual and quantization:** Middle C after one second, 131072 frames, all models; repeat with the level knob 40 dB lower. Integrate power outside guarded harmonics, including ultrasonic bins through Nyquist, excluding DC and its guard. This is total non-harmonic residual, not isolated quantization noise. Without a meaningful harmonic fundamental the reference is explicitly a frequency-band proxy. Weak references use the ideal note frequency for the harmonic mask. The output level knob follows integer conversion, so it scales existing quantization along with the signal. The theoretical white rounding-noise figure is a comparison, not a claim that the original module's truncation produces white noise.
2. **Keyboard:** Same settling and capture, five keys, defaults and maximum Harmonics/Timbre/Morph. Find the strongest local spectral maximum outside guarded harmonics below 20 kHz. Its integral excludes the guarded bins too, preventing a sidelobe from integrating back into the fundamental. Maximum knobs are a reproducible stress setting, not a perceptually optimized brightest patch; some controls select ratios, waveforms, chords, or patches. Intentional inharmonic/noise settings and missing/weak fundamentals have no alias figure. Even eligible rows measure non-harmonic components, without proving their cause. Search centers follow the source's pitch limits; the frequency itself is measured, and cents always compare it with the requested MIDI note. The output states both clamps and the weak-reference thresholds.
3. **Modulation:** One matrix row at a time, seven destinations, sine at 1/10/40 Hz and square at 40 Hz, 524288 frames after one second. A controlled FM patch supplies a near-1000-Hz carrier. Harmonics and Morph use nonzero FM index so the destination actually changes the sound. Search around the first four multiples of the 4000-Hz control rate on either side of the carrier; include an unmodulated baseline. Also search off the carrier-plus-integer-LFO-frequency grid for square modulation. Control images coincide with that grid at the maximum rate, so this cannot isolate square-shape aliases. Intended synthesis sidebands and harmonic distortion can occupy the image bands.
4. **Shaper:** Filter envelope and maximum-rate sine LFO, with a static filter baseline; capture from note-on so the envelope sweep is included. Drive at 0 and 24 dB on the sine-like FM patch at C5 and C7. These are complete instrument outputs, without a private pre-quantization or pre-filter signal.
5. **Rates:** Repeat residual and keyboard measurements for Virtual analog, FM, and Inharmonic string at 44100 and 96000 Hz; print frequency and cents relative to equal temperament. A frequency-band candidate for an inharmonic signal is not a tuning estimate. These rates and 48000 Hz are native voice rates, so the non-native-rate pitch-offset branch is not exercised.
6. **Headroom:** First 131072 frames after note-on, single middle C and the specified eight-note chord; sample peak, unwindowed RMS, and number of samples reaching or exceeding unity. Floating output can exceed unity; this count does not establish clipping in a subsequent mixer or device.

## Reproduction and validation

Run from the repository root:

```powershell
$env:CARGO_TARGET_DIR = 'E:/transit-targets/codex-716'
$env:CARGO_BUILD_JOBS = '8'
cargo run --release --locked -p transit-dsp --example macro_osc_audit -j 8
cargo clippy --locked -p transit-dsp --all-targets -j 8 -- -D warnings
cargo fmt --all -- --check
cargo xtask layers
```

The example takes no arguments. Execution stayed within the requested runtime budget without shortening captures. Wall-clock time is measured outside the program so it does not alter the output. A second execution of the release executable produced byte-identical output. The release run, clippy, formatting check, and layer check passed. No test suites or ignored tests ran. The layer check also passed its file-size and temporary-directory checks.

`transit-math` was added only as a workspace development dependency of `transit-dsp`; the lockfile changed only for that dependency edge. There is no product-code change. All tables below were copied directly from the completed program's standard output. An exploratory analyzer run exposed integration across a harmonic guard and undefined silent-signal ratios; those analyzer errors were corrected before collecting these tables. Calibration now includes the strongest outside-harmonic component as well as integrated residual.

## What this shows

This orders measured magnitudes, not listening thresholds. There was no measurement of playback level or masking, so a distance from universal inaudibility cannot be assigned. Model content, transient decay, and a weak fundamental must not be ranked as defects. No choice of future implementation follows from this record.

- **Headroom is the largest absolute-level result.** Particle noise's chord peaks at +12.06 dBFS, with 74 captured samples reaching full scale. Its chord RMS is -19.59 dBFS: this is a transient peak, not a sustained level. Inharmonic string is the only chord row with no full-scale samples; every held single-note row has none. Summing voices without normalization explains the possibility of overs, but that explanation is source-derived, not proof of downstream clipping.
- **Keyboard components differ substantially by model.** Among eligible rows, the largest outside-harmonic component is Phase distortion at maximum knobs, C7: +6.14 dBc at 2371.21 Hz. The per-model maxima then descend through Virtual analog (-13.33 dBc), Wave terrain (-24.17 dBc), Wavetable (-25.96 dBc), Waveshaper (-28.46 dBc), Filtered analog (-30.05 dBc), and Grain (-31.41 dBc). The full rows identify their keys and settings. These relative figures are sensitive to how much fundamental the patch contains; they are not a ranking of total loudness or proven alias power. Additive's weak or absent fundamental prevents a useful ratio in this sweep. The explicitly listed inharmonic/noise settings are unranked.
- **Shaping and modulation add observable high-frequency content.** Maximum drive on the FM sine patch at C7 gives -36.25 dBc at 16604.93 Hz, versus -92.41 dBc for the strongest component with drive off. The deep filter LFO sweep has a first-image-band maximum of -48.06 dBc; the envelope sweep reads -98.88 dBc and the static filter -115.66 dBc. In the shallower matrix experiment, sine modulation at maximum rate reads -63.03 dBc for Cutoff, -65.84 dBc for Level, and -71.58 dBc for Pan. The raw largest sine image-band result is Morph at -31.22 dBc, but its baseline already reads -30.96 dBc, so that is not demonstrated stepping. The largest square off-grid result is Harmonics at -17.85 dBc, with intentional ratio modulation; it is not an isolated square-LFO alias measurement.
- **Default tonal residuals exceed the ideal rounding-noise prediction, but this does not identify quantization as their cause.** Virtual analog has the largest meaningful default tonal residual: -54.62 dBc, or -69.46 dBFS. Wave terrain, Wavetable, and Phase distortion follow at -55.65, -55.99, and -56.85 dBc; Filtered analog reads -63.96 dBc, Waveshaper -78.27 dBc, and Grain -84.38 dBc. Nonzero residuals scale by -40.000 dB with the level knob. That is consistent with a post-conversion gain and cannot distinguish quantization from other residual content. The ideal rounding model predicts -107.10 dBFS after the default gain; that figure is theoretical. Synthetic calibration shows the analysis limit: the unquantized sine already reads -87.460 dBc residual, whereas the rounded low sine reads -101.526 dBFS residual. The quietest instrument residuals are therefore upper bounds containing leakage, not resolved quantization floors.
- **Rate changes affect the measured spectrum, while the top MIDI note hits a separate pitch limit.** The largest eligible rate-repeat component is Virtual analog at maximum knobs and C7 at 44100 Hz: -15.60 dBc at 10611.92 Hz. At 96000 Hz its corresponding result is -22.49 dBc at 8650.22 Hz; at 48000 Hz it is -13.33 dBc at 12418.92 Hz. Default middle-C residual is -54.08, -54.62, and -71.30 dBc respectively. At the highest accepted MIDI key, Virtual analog measures 8372.018 Hz at 44100 Hz and 8372.017 Hz at 96000 Hz, each -700.000 cents from the requested note. The source's note clamp is the inferred explanation; these native-rate observations do not test pitch-offset compensation.

**Sensitivity:** Doubling capture length leaves the Phase distortion, Virtual analog, and Wave terrain maximum-knob C7 components unchanged at the printed precision. Waveshaper's middle-C residual moves from -78.27 to -79.05 dBc; Grain's from -84.38 to -85.24 dBc, while Grain's C7 component moves from -74.64 to -74.60 dBc. The per-model guard-width column supplies another sensitivity check on the same captures. It is especially consequential for evolving or almost-decayed sounds: Six-op FM 3 reads -9.836 dB in that column. No alternate window was measured; these are capture-length and mask sensitivities, not an uncertainty bound over every analysis choice.

**Not isolated:** Quantization before conversion; aliases distinguished from intended model content; square-LFO aliases that coincide with legitimate sidebands; envelope stepping separated from the sweep transient; non-native-rate correction; listening audibility; and downstream clipping. Speech, Bass drum, Snare, and Hi-hat have zero power in the settled default capture, even though their onset headroom rows contain signal. A held key does not make every model sustain. Their silent ratios/frequencies remain undefined rather than becoming reassuring noise-floor numbers. Voice stealing and releases are outside this measurement.

**Source observations, unchanged:** The public wrapper accepts MIDI through key 127, but the original voice clamps its synthesis note to 120 in [`voice.rs`](../../crates/macro-osc/src/voice.rs); Grain additionally caps its carrier at one eighth of the sample rate in [`grainlet_oscillator.rs`](../../crates/macro-osc/src/oscillator/grainlet_oscillator.rs). This limits the keyboard/tuning interpretation and is not a newly established product defect. [`macro_osc.rs`](../../crates/dsp/src/plugins/macro_osc.rs) applies output level after integer conversion and sums voices without a limiter. [`macro_osc_voice.rs`](../../crates/dsp/src/plugins/macro_osc_voice.rs) reads modulation sources once per voice block. No product defect was found that invalidated the usable measurements, and no product code was changed.

## Program output: Macro Oscillator sound-quality audit

No arguments; fresh instrument per capture, voice seed=0x21+voice index; Plugin::note_on then Plugin::process, stereo, 128-frame calls, left output analyzed. No audio device. Velocity 127; eight voices; main output only; level -6 dB; macros 50%; shaper off unless specified. Scientific note names: C1=24, C3=48, C4/middle C=60, C5=72, C7=96; highest accepted MIDI key=127. A4=440 Hz.

FFT and all analysis transcendental math: transit_math f64. Periodic four-term Blackman-Harris window (0.35875, 0.48829, 0.14128, 0.01168), approximately -92 dB peak sidelobes. N=131072: bin spacing 0.366211 Hz at 48k, 0.336456 Hz at 44.1k, 0.732422 Hz at 96k. Modulation N=524288: 0.091553 Hz at 48k (10.923 s). Main-lobe width is about eight bins, not one bin. Fixed captures, no adaptive shortening.

Power bins = one-sided |FFT|^2/(N*sum(window^2)); RMS dBFS=10 log10(power), so a unit-peak sine is -3.0103 dBFS. Fundamental candidate = strongest bin within +/-3% of the search center; log-parabolic frequency interpolation; its power integrates +/-4 bins. Center is ideal note frequency, except keyboard rows use ideal(min(key,120)) and Grain additionally min(center,sr/8), following source clamps; cents always compare the measured candidate to the requested note. These centers are source-derived, never substituted for a measured frequency. A missing/inharmonic fundamental makes this only a nominal-band proxy, NOT a tuning measurement. A reference at or below -120 dBFS or at least 80 dB below total windowed power is labeled weak: no keyboard ratio; section1 uses the ideal frequency for its mask and labels its ratio a proxy. No denominator floor: -inf means zero power, NaN means undefined ratio/frequency, n/a means excluded. Non-harmonic residual integrates all bins above DC+5 through Nyquist except +/-max(5 bins,0.1% of harmonic Hz) of candidate harmonics. Strongest components use local maxima in 20 Hz..20 kHz, integrated +/-4 bins WITH the same selection/exclusion applied to every bin; frequency is interpolated. This prevents a sidelobe's integral from including the excluded main lobe. Components closer than a main lobe are unresolved. Non-harmonic does NOT establish alias causation.

Section 1: 1 s settling then N samples; level -6 versus -46 dB (after int16 conversion). Low-level comparison cannot isolate quantization. Ideal rounded undithered 16-bit step 1/32768 predicts q^2/12=-101.10 dBFS noise, 98.09 dB SNR for a full-scale sine; after -6 dB gain -107.10 dBFS. Actual truncation is signal-correlated, not white rounding noise; measured residual also includes intended model content and leakage. Guard 5->8 is a same-capture sensitivity check, with the 0.1% floor retained.

Section 2: same settling/capture, defaults versus all three macros at 100% (a reproducible maximum-control stress, not a proven maximum of perceptual brightness; patch/chord/ratio selectors are not monotonic brightness controls). Ineligible settings below have no alias number. Candidate Hz/cents remain a reference-band observation for these settings.

| Model | Harmonic exclusion not valid when |
|---|---|
| Filtered analog | maximum only: wave/sub mixture; subharmonics |
| Six-op FM 1 | both: patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | both: patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | both: patch-dependent six-operator ratios, detuning and envelopes |
| String machine | both: multiple chord pitches / arpeggiation |
| Chiptune | both: multiple chord pitches / arpeggiation |
| FM | both: noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| Chord | both: multiple chord pitches / arpeggiation |
| Speech | both: speech/formants with possible unvoiced content |
| Swarm | both: detuned stochastic grains |
| Filtered noise | both: noise or inharmonic percussion |
| Particle noise | both: noise or inharmonic percussion |
| Inharmonic string | both: inharmonic physical modes |
| Modal | both: inharmonic physical modes |
| Bass drum | both: pitch envelope and nonlinear percussion |
| Snare | both: noise or inharmonic percussion |
| Hi-hat | both: noise or inharmonic percussion |

Section 3: FM model, ratio=1 (Harmonics=23.4375%), Timbre=0, Morph=50%, note72 tuned to 1000 Hz; shaper on, cutoff4000 Hz, default filter/envelopes. Harmonics and Morph destinations use Timbre=20% so ratio/feedback modulation has an effect. Matrix row1=LFO1, via Off, linear; pitch depth0.1% (+/-2.4 cents), other depths10%; retriggered sine1/10/40 Hz and square40 Hz. 1 s settling, MOD_N capture. Carrier integrates +/-4 bins around baseline measured frequency. Image k searches local maxima within +/-(2*LFO rate+5 bins) of |carrier +/- k*4000|, k=1..4; baseline maximum over the same bands is printed, no spectral subtraction. Harmonic distortion can coincide with image bands. Off-comb square excludes +/-5 bins of carrier+n*40 (integer n). At 4000/40=100, control images overlap legitimate LFO harmonics: this cannot separate square-shape aliasing. Timbre/ratio/pitch modulation also creates intended sidebands.

Section 4: same FM sine, cutoff1000 Hz; static baseline, envelope +3 octaves attack2000/decay2000 ms/sustain0, or sine40 Hz +/-3 octaves. Capture starts at note-on, N samples; image bands use +/-80 Hz plus 5 bins. Transient/envelope spectra are not isolated stepping. Drive0/24 dB on sine-like FM at MIDI72/96, default cutoff20000, shaper on, 1 s settling, N samples; harmonic exclusion as section2.

Section 5: Virtual analog, FM, Inharmonic string repeat sections1/2 at 44100/96000 Hz. All three host rates are directly supported: code gives zero rate_offset, so this does not exercise the non-native-rate pitch-offset branch. Section6: first N samples from note-on, default settings/full velocity; single60 and chord48,52,55,59,60,64,67,71; sample peak, unwindowed RMS, count abs(sample)>=1 (floating output, not a claim of clipping). No releases/voice stealing measured.

### Residual and level scaling, 48000 Hz

| Model | Reference | f0 Hz | Fundamental dBFS | Residual dBc | Residual dBFS | Low fundamental dBFS | Low residual dBc | Low residual dBFS | Residual change dB | Guard 5->8 change dB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Filtered analog | tonal candidate | 261.627 | -15.23 | -63.96 | -79.19 | -55.23 | -63.96 | -119.19 | -40.000 | -0.011 |
| Phase distortion | tonal candidate | 261.625 | -23.19 | -56.85 | -80.04 | -63.19 | -56.85 | -120.04 | -40.000 | -0.008 |
| Six-op FM 1 | nominal-band proxy | 260.872 | -19.33 | 1.27 | -18.06 | -59.33 | 1.27 | -58.06 | -40.000 | -1.404 |
| Six-op FM 2 | missing/weak fundamental proxy | 253.601 | -149.74 | 47.28 | -102.46 | -189.74 | 47.28 | -142.46 | -40.000 | -0.000 |
| Six-op FM 3 | nominal-band proxy | 261.642 | -28.03 | -57.76 | -85.78 | -68.03 | -57.76 | -125.78 | -40.000 | -9.836 |
| Wave terrain | tonal candidate | 261.627 | -23.07 | -55.65 | -78.72 | -63.07 | -55.65 | -118.72 | -40.000 | -0.004 |
| String machine | nominal-band proxy | 261.103 | -44.50 | 18.70 | -25.80 | -84.50 | 18.70 | -65.80 | -40.000 | -0.000 |
| Chiptune | missing/weak fundamental proxy | 261.869 | -148.09 | 135.39 | -12.70 | -188.09 | 135.39 | -52.70 | -40.000 | -0.000 |
| Virtual analog | tonal candidate | 261.627 | -14.85 | -54.62 | -69.46 | -54.85 | -54.62 | -109.46 | -40.000 | -0.001 |
| Waveshaper | tonal candidate | 261.627 | -20.12 | -78.27 | -98.39 | -60.12 | -78.27 | -138.39 | -40.000 | -0.602 |
| FM | nominal-band proxy | 261.626 | -23.79 | 9.83 | -13.95 | -63.79 | 9.83 | -53.95 | -40.000 | -0.000 |
| Grain | tonal candidate | 261.627 | -18.48 | -84.38 | -102.87 | -58.48 | -84.38 | -142.87 | -40.000 | -1.712 |
| Additive | missing/weak fundamental proxy | 261.596 | -133.59 | 40.61 | -92.98 | -173.59 | 40.60 | -132.98 | -40.000 | -0.001 |
| Wavetable | tonal candidate | 261.627 | -23.69 | -55.99 | -79.68 | -63.69 | -55.99 | -119.68 | -40.000 | -0.003 |
| Chord | missing/weak fundamental proxy | 257.017 | -111.72 | 96.32 | -15.40 | -151.72 | 96.32 | -55.40 | -40.000 | -0.016 |
| Speech | missing/weak fundamental proxy | NaN | -inf | NaN | -inf | -inf | NaN | -inf | NaN | NaN |
| Swarm | nominal-band proxy | 261.669 | -31.29 | 13.17 | -18.12 | -71.29 | 13.17 | -58.12 | -40.000 | -0.080 |
| Filtered noise | nominal-band proxy | 257.801 | -26.59 | 8.96 | -17.64 | -66.59 | 8.96 | -57.64 | -40.000 | -0.517 |
| Particle noise | nominal-band proxy | 259.121 | -52.25 | 19.04 | -33.21 | -92.25 | 19.04 | -73.21 | -40.000 | -0.021 |
| Inharmonic string | nominal-band proxy | 261.222 | -59.80 | -47.30 | -107.10 | -99.80 | -47.30 | -147.10 | -40.000 | -0.008 |
| Modal | missing/weak fundamental proxy | 261.198 | -125.57 | 69.15 | -56.42 | -165.57 | 69.15 | -96.42 | -40.000 | -0.000 |
| Bass drum | missing/weak fundamental proxy | NaN | -inf | NaN | -inf | -inf | NaN | -inf | NaN | NaN |
| Snare | missing/weak fundamental proxy | NaN | -inf | NaN | -inf | -inf | NaN | -inf | NaN | NaN |
| Hi-hat | missing/weak fundamental proxy | NaN | -inf | NaN | -inf | -inf | NaN | -inf | NaN | NaN |

### Keyboard / tuning, 48000 Hz

| Model | Knobs | MIDI key | Ideal Hz | Candidate Hz | Cents | Reference dBFS | Strongest outside harmonics dBc | Hz | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Filtered analog | default | 24 | 32.703 | 32.705 | 0.087 | -15.23 | -88.39 | 23.71 | candidate, not proven alias |
| Filtered analog | default | 48 | 130.813 | 130.813 | 0.008 | -15.23 | -76.98 | 253.49 | candidate, not proven alias |
| Filtered analog | default | 72 | 523.251 | 523.251 | -0.001 | -15.23 | -63.94 | 384.09 | candidate, not proven alias |
| Filtered analog | default | 96 | 2093.005 | 2093.006 | 0.001 | -15.52 | -49.88 | 4046.90 | candidate, not proven alias |
| Filtered analog | default | 127 | 12543.854 | 8372.021 | -699.999 | -23.79 | -30.05 | 6139.90 | candidate, not proven alias |
| Filtered analog | maximum | 24 | 32.703 | 32.704 | 0.057 | -15.27 | n/a | n/a | wave/sub mixture; subharmonics |
| Filtered analog | maximum | 48 | 130.813 | 130.814 | 0.020 | -15.37 | n/a | n/a | wave/sub mixture; subharmonics |
| Filtered analog | maximum | 72 | 523.251 | 523.251 | -0.001 | -16.69 | n/a | n/a | wave/sub mixture; subharmonics |
| Filtered analog | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -15.68 | n/a | n/a | wave/sub mixture; subharmonics |
| Filtered analog | maximum | 127 | 12543.854 | 8372.021 | -699.999 | -17.56 | n/a | n/a | wave/sub mixture; subharmonics |
| Phase distortion | default | 24 | 32.703 | 32.704 | 0.030 | -23.52 | -82.87 | 160.96 | candidate, not proven alias |
| Phase distortion | default | 48 | 130.813 | 130.813 | 0.004 | -23.37 | -86.48 | 7055.84 | candidate, not proven alias |
| Phase distortion | default | 72 | 523.251 | 523.250 | -0.002 | -22.91 | -58.69 | 12942.16 | candidate, not proven alias |
| Phase distortion | default | 96 | 2093.005 | 2093.006 | 0.001 | -22.98 | -26.03 | 14511.93 | candidate, not proven alias |
| Phase distortion | default | 127 | 12543.854 | 8372.020 | -700.000 | -19.44 | -25.92 | 6139.91 | candidate, not proven alias |
| Phase distortion | maximum | 24 | 32.703 | 32.704 | 0.030 | -45.88 | -15.04 | 781.75 | candidate, not proven alias |
| Phase distortion | maximum | 48 | 130.813 | 130.813 | 0.004 | -40.63 | -12.60 | 1193.34 | candidate, not proven alias |
| Phase distortion | maximum | 72 | 523.251 | 523.250 | -0.002 | -29.02 | -11.15 | 18174.67 | candidate, not proven alias |
| Phase distortion | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -34.50 | 6.14 | 2371.21 | candidate, not proven alias |
| Phase distortion | maximum | 127 | 12543.854 | 8372.020 | -700.000 | -14.91 | -25.57 | 14511.93 | candidate, not proven alias |
| Six-op FM 1 | default | 24 | 32.703 | 32.645 | -3.084 | -21.27 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | default | 48 | 130.813 | 130.320 | -6.535 | -21.23 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | default | 72 | 523.251 | 521.888 | -4.515 | -21.72 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | default | 96 | 2093.005 | 2092.981 | -0.020 | -26.52 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | default | 127 | 12543.854 | 8325.237 | -709.701 | -32.71 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | maximum | 24 | 32.703 | 32.669 | -1.834 | -25.83 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | maximum | 48 | 130.813 | 130.921 | 1.435 | -26.80 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | maximum | 72 | 523.251 | 523.500 | 0.822 | -27.33 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | maximum | 96 | 2093.005 | 2094.782 | 1.470 | -26.65 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 1 | maximum | 127 | 12543.854 | 8379.565 | -698.440 | -34.44 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | default | 24 | 32.703 | 32.482 | -11.761 | -163.65 | n/a | n/a | missing/weak fundamental; no ratio |
| Six-op FM 2 | default | 48 | 130.813 | 130.856 | 0.569 | -173.67 | n/a | n/a | missing/weak fundamental; no ratio |
| Six-op FM 2 | default | 72 | 523.251 | 539.246 | 52.127 | -149.24 | n/a | n/a | missing/weak fundamental; no ratio |
| Six-op FM 2 | default | 96 | 2093.005 | 2152.939 | 48.879 | -157.97 | n/a | n/a | missing/weak fundamental; no ratio |
| Six-op FM 2 | default | 127 | 12543.854 | 8125.927 | -751.652 | -158.78 | n/a | n/a | missing/weak fundamental; no ratio |
| Six-op FM 2 | maximum | 24 | 32.703 | 33.875 | 60.922 | -49.36 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | maximum | 48 | 130.813 | 128.672 | -28.568 | -55.31 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | maximum | 72 | 523.251 | 521.134 | -7.019 | -85.47 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | maximum | 96 | 2093.005 | 2088.770 | -3.506 | -94.30 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 2 | maximum | 127 | 12543.854 | 8212.824 | -733.237 | -101.38 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | default | 24 | 32.703 | 32.729 | 1.355 | -28.02 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | default | 48 | 130.813 | 130.839 | 0.352 | -28.02 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | default | 72 | 523.251 | 523.235 | -0.054 | -28.05 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | default | 96 | 2093.005 | 2093.026 | 0.018 | -28.52 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | default | 127 | 12543.854 | 8372.075 | -699.988 | -26.83 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | maximum | 24 | 32.703 | 31.948 | -40.424 | -21.25 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | maximum | 48 | 130.813 | 130.861 | 0.642 | -36.73 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | maximum | 72 | 523.251 | 523.272 | 0.069 | -32.77 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | maximum | 96 | 2093.005 | 2092.756 | -0.206 | -23.70 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Six-op FM 3 | maximum | 127 | 12543.854 | 8382.550 | -697.823 | -33.00 | n/a | n/a | patch-dependent six-operator ratios, detuning and envelopes |
| Wave terrain | default | 24 | 32.703 | 32.704 | 0.062 | -22.61 | -87.76 | 67.97 | candidate, not proven alias |
| Wave terrain | default | 48 | 130.813 | 130.814 | 0.014 | -22.79 | -84.17 | 375.86 | candidate, not proven alias |
| Wave terrain | default | 72 | 523.251 | 523.251 | 0.000 | -23.78 | -59.36 | 13988.61 | candidate, not proven alias |
| Wave terrain | default | 96 | 2093.005 | 2093.071 | 0.055 | -25.85 | -32.98 | 16603.95 | candidate, not proven alias |
| Wave terrain | default | 127 | 12543.854 | 8375.965 | -699.184 | -31.19 | -64.44 | 14496.14 | candidate, not proven alias |
| Wave terrain | maximum | 24 | 32.703 | 32.704 | 0.062 | -22.43 | -90.23 | 62.70 | candidate, not proven alias |
| Wave terrain | maximum | 48 | 130.813 | 130.814 | 0.014 | -22.00 | -67.76 | 3940.97 | candidate, not proven alias |
| Wave terrain | maximum | 72 | 523.251 | 523.251 | 0.000 | -20.09 | -60.37 | 4675.73 | candidate, not proven alias |
| Wave terrain | maximum | 96 | 2093.005 | 2093.071 | 0.055 | -23.37 | -24.17 | 10324.74 | candidate, not proven alias |
| Wave terrain | maximum | 127 | 12543.854 | 8375.965 | -699.184 | -23.12 | -30.15 | 14496.14 | candidate, not proven alias |
| String machine | default | 24 | 32.703 | 32.639 | -3.417 | -44.50 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | default | 48 | 130.813 | 130.551 | -3.463 | -44.50 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | default | 72 | 523.251 | 522.205 | -3.465 | -44.51 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | default | 96 | 2093.005 | 2088.818 | -3.466 | -44.73 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | default | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| String machine | maximum | 24 | 32.703 | 32.620 | -4.431 | -23.36 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | maximum | 48 | 130.813 | 130.593 | -2.915 | -28.40 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | maximum | 72 | 523.251 | 522.233 | -3.372 | -31.50 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | maximum | 96 | 2093.005 | 2088.840 | -3.448 | -30.35 | n/a | n/a | multiple chord pitches / arpeggiation |
| String machine | maximum | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | default | 24 | 32.703 | 31.968 | -39.357 | -157.55 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | default | 48 | 130.813 | 128.548 | -30.242 | -151.45 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | default | 72 | 523.251 | 525.235 | 6.552 | -143.80 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | default | 96 | 2093.005 | 2075.917 | -14.192 | -151.15 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | default | 127 | 12543.854 | 8590.480 | -655.404 | -144.08 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | maximum | 24 | 32.703 | 33.327 | 32.701 | -158.57 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | maximum | 48 | 130.813 | 129.896 | -12.178 | -155.55 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | maximum | 72 | 523.251 | 515.699 | -25.168 | -147.84 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | maximum | 96 | 2093.005 | 2091.792 | -1.003 | -151.25 | n/a | n/a | missing/weak fundamental; no ratio |
| Chiptune | maximum | 127 | 12543.854 | 8580.324 | -657.452 | -146.20 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | default | 24 | 32.703 | 32.705 | 0.087 | -14.85 | -90.39 | 8788.29 | candidate, not proven alias |
| Virtual analog | default | 48 | 130.813 | 130.813 | 0.008 | -14.85 | -80.66 | 7840.61 | candidate, not proven alias |
| Virtual analog | default | 72 | 523.251 | 523.251 | -0.001 | -14.84 | -63.20 | 18174.65 | candidate, not proven alias |
| Virtual analog | default | 96 | 2093.005 | 2093.006 | 0.001 | -14.85 | -49.26 | 12418.92 | candidate, not proven alias |
| Virtual analog | default | 127 | 12543.854 | 8372.021 | -699.999 | -16.33 | -38.76 | 14511.92 | candidate, not proven alias |
| Virtual analog | maximum | 24 | 32.703 | 32.705 | 0.075 | -112.73 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 48 | 130.813 | 130.828 | 0.197 | -125.62 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 72 | 523.251 | 523.233 | -0.061 | -131.43 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -41.97 | -13.33 | 12418.92 | candidate, not proven alias |
| Virtual analog | maximum | 127 | 12543.854 | 8372.021 | -699.999 | -23.97 | -33.62 | 14511.92 | candidate, not proven alias |
| Waveshaper | default | 24 | 32.703 | 32.705 | 0.087 | -19.47 | -92.04 | 30.17 | candidate, not proven alias |
| Waveshaper | default | 48 | 130.813 | 130.813 | 0.008 | -19.48 | -94.83 | 128.23 | candidate, not proven alias |
| Waveshaper | default | 72 | 523.251 | 523.251 | -0.001 | -23.47 | -81.31 | 3523.59 | candidate, not proven alias |
| Waveshaper | default | 96 | 2093.005 | 2093.006 | 0.001 | -14.48 | -74.32 | 14511.93 | candidate, not proven alias |
| Waveshaper | default | 127 | 12543.854 | 8372.021 | -699.999 | -17.81 | -38.69 | 14511.92 | candidate, not proven alias |
| Waveshaper | maximum | 24 | 32.703 | 32.705 | 0.087 | -18.61 | -85.71 | 67.99 | candidate, not proven alias |
| Waveshaper | maximum | 48 | 130.813 | 130.813 | 0.008 | -21.49 | -28.46 | 2624.39 | candidate, not proven alias |
| Waveshaper | maximum | 72 | 523.251 | 523.251 | -0.001 | -17.39 | -46.38 | 8232.86 | candidate, not proven alias |
| Waveshaper | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -15.24 | -49.30 | 1953.90 | candidate, not proven alias |
| Waveshaper | maximum | 127 | 12543.854 | 8372.021 | -699.999 | -17.81 | -38.69 | 14511.92 | candidate, not proven alias |
| FM | default | 24 | 32.703 | 32.982 | 14.688 | -19.54 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 48 | 130.813 | 130.814 | 0.014 | -23.78 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 72 | 523.251 | 523.250 | -0.003 | -23.80 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 96 | 2093.005 | 2130.955 | 31.110 | -18.71 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 127 | 12543.854 | 8372.021 | -699.999 | -17.08 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 24 | 32.703 | 32.704 | 0.062 | -42.09 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 48 | 130.813 | 130.814 | 0.014 | -39.91 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 72 | 523.251 | 523.250 | -0.003 | -29.83 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -13.72 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 127 | 12543.854 | 8372.021 | -699.999 | -17.08 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| Grain | default | 24 | 32.703 | 32.705 | 0.087 | -51.10 | -59.30 | 357.15 | candidate, not proven alias |
| Grain | default | 48 | 130.813 | 130.813 | 0.008 | -32.04 | -77.08 | 394.99 | candidate, not proven alias |
| Grain | default | 72 | 523.251 | 523.251 | -0.001 | -18.84 | -96.20 | 520.66 | candidate, not proven alias |
| Grain | default | 96 | 2093.005 | 2093.006 | 0.001 | -23.51 | -84.64 | 6139.91 | candidate, not proven alias |
| Grain | default | 127 | 12543.854 | 6000.000 | -1276.736 | -33.68 | -323.39 | 1772.46 | candidate, not proven alias |
| Grain | maximum | 24 | 32.703 | 32.705 | 0.087 | -67.97 | -31.41 | 5681.44 | candidate, not proven alias |
| Grain | maximum | 48 | 130.813 | 130.813 | 0.008 | -55.87 | -31.50 | 4831.92 | candidate, not proven alias |
| Grain | maximum | 72 | 523.251 | 523.251 | -0.001 | -43.58 | -31.72 | 5616.60 | candidate, not proven alias |
| Grain | maximum | 96 | 2093.005 | 2093.006 | 0.001 | -30.74 | -74.64 | 12418.92 | candidate, not proven alias |
| Grain | maximum | 127 | 12543.854 | 6000.000 | -1276.736 | -27.83 | -323.85 | 1772.46 | candidate, not proven alias |
| Additive | default | 24 | 32.703 | 32.724 | 1.082 | -131.22 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | default | 48 | 130.813 | 130.819 | 0.087 | -131.26 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | default | 72 | 523.251 | 523.162 | -0.296 | -138.74 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | default | 96 | 2093.005 | 2047.792 | -37.808 | -139.66 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | default | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | maximum | 24 | 32.703 | 32.875 | 9.081 | -141.98 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | maximum | 48 | 130.813 | 134.275 | 45.225 | -141.17 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | maximum | 72 | 523.251 | 523.194 | -0.188 | -138.16 | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | maximum | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Additive | maximum | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Wavetable | default | 24 | 32.703 | 32.705 | 0.087 | -23.65 | -92.16 | 35.29 | candidate, not proven alias |
| Wavetable | default | 48 | 130.813 | 130.813 | 0.008 | -23.66 | -88.33 | 3000.55 | candidate, not proven alias |
| Wavetable | default | 72 | 523.251 | 523.251 | -0.001 | -23.74 | -44.66 | 10325.87 | candidate, not proven alias |
| Wavetable | default | 96 | 2093.005 | 2093.006 | 0.001 | -24.25 | -31.96 | 14511.93 | candidate, not proven alias |
| Wavetable | default | 127 | 12543.854 | 8372.021 | -699.999 | -28.60 | -25.96 | 14511.92 | candidate, not proven alias |
| Wavetable | maximum | 24 | 32.703 | 32.729 | 1.348 | -123.28 | n/a | n/a | missing/weak fundamental; no ratio |
| Wavetable | maximum | 48 | 130.813 | 130.762 | -0.673 | -130.87 | n/a | n/a | missing/weak fundamental; no ratio |
| Wavetable | maximum | 72 | 523.251 | 523.301 | 0.166 | -136.24 | n/a | n/a | missing/weak fundamental; no ratio |
| Wavetable | maximum | 96 | 2093.005 | 2124.045 | 25.487 | -143.60 | n/a | n/a | missing/weak fundamental; no ratio |
| Wavetable | maximum | 127 | 12543.854 | 8297.889 | -715.397 | -141.22 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | default | 24 | 32.703 | 32.043 | -35.282 | -129.18 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | default | 48 | 130.813 | 126.526 | -57.685 | -134.21 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | default | 72 | 523.251 | 513.935 | -31.103 | -112.14 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | default | 96 | 2093.005 | 2045.990 | -39.332 | -102.00 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | default | 127 | 12543.854 | 8330.169 | -708.676 | -158.04 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | maximum | 24 | 32.703 | 33.875 | 60.922 | -143.20 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | maximum | 48 | 130.813 | 132.169 | 17.856 | -144.57 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | maximum | 72 | 523.251 | 514.271 | -29.968 | -142.67 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | maximum | 96 | 2093.005 | 2049.329 | -36.508 | -154.15 | n/a | n/a | missing/weak fundamental; no ratio |
| Chord | maximum | 127 | 12543.854 | 8352.751 | -703.989 | -158.01 | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | default | 24 | 32.703 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | default | 48 | 130.813 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | default | 72 | 523.251 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | default | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | default | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | maximum | 24 | 32.703 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | maximum | 48 | 130.813 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | maximum | 72 | 523.251 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | maximum | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Speech | maximum | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Swarm | default | 24 | 32.703 | 32.723 | 1.039 | -26.37 | n/a | n/a | detuned stochastic grains |
| Swarm | default | 48 | 130.813 | 130.837 | 0.314 | -29.24 | n/a | n/a | detuned stochastic grains |
| Swarm | default | 72 | 523.251 | 523.143 | -0.357 | -31.60 | n/a | n/a | detuned stochastic grains |
| Swarm | default | 96 | 2093.005 | 2092.933 | -0.059 | -30.70 | n/a | n/a | detuned stochastic grains |
| Swarm | default | 127 | 12543.854 | 8372.061 | -699.991 | -35.46 | n/a | n/a | detuned stochastic grains |
| Swarm | maximum | 24 | 32.703 | 32.460 | -12.914 | -24.84 | n/a | n/a | detuned stochastic grains |
| Swarm | maximum | 48 | 130.813 | 130.839 | 0.351 | -27.86 | n/a | n/a | detuned stochastic grains |
| Swarm | maximum | 72 | 523.251 | 523.234 | -0.056 | -28.17 | n/a | n/a | detuned stochastic grains |
| Swarm | maximum | 96 | 2093.005 | 2093.004 | -0.001 | -27.31 | n/a | n/a | detuned stochastic grains |
| Swarm | maximum | 127 | 12543.854 | 8372.031 | -699.997 | -29.91 | n/a | n/a | detuned stochastic grains |
| Filtered noise | default | 24 | 32.703 | 33.062 | 18.877 | -16.14 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | default | 48 | 130.813 | 128.533 | -30.431 | -22.36 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | default | 72 | 523.251 | 530.721 | 24.539 | -34.23 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | default | 96 | 2093.005 | 2064.988 | -23.331 | -47.82 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | default | 127 | 12543.854 | 8327.927 | -709.142 | -68.76 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | maximum | 24 | 32.703 | 32.735 | 1.705 | -21.43 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | maximum | 48 | 130.813 | 130.813 | -0.003 | -24.59 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | maximum | 72 | 523.251 | 523.762 | 1.689 | -24.41 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | maximum | 96 | 2093.005 | 2093.841 | 0.692 | -24.30 | n/a | n/a | noise or inharmonic percussion |
| Filtered noise | maximum | 127 | 12543.854 | 8374.360 | -699.516 | -35.88 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | default | 24 | 32.703 | 32.656 | -2.475 | -44.58 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | default | 48 | 130.813 | 130.239 | -7.614 | -49.01 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | default | 72 | 523.251 | 509.450 | -46.276 | -55.56 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | default | 96 | 2093.005 | 2031.696 | -51.469 | -63.86 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | default | 127 | 12543.854 | 8199.445 | -736.059 | -74.89 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | maximum | 24 | 32.703 | 33.644 | 49.127 | -31.18 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | maximum | 48 | 130.813 | 129.388 | -18.955 | -37.26 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | maximum | 72 | 523.251 | 514.228 | -30.116 | -44.24 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | maximum | 96 | 2093.005 | 2064.288 | -23.918 | -50.70 | n/a | n/a | noise or inharmonic percussion |
| Particle noise | maximum | 127 | 12543.854 | 8456.549 | -682.608 | -62.94 | n/a | n/a | noise or inharmonic percussion |
| Inharmonic string | default | 24 | 32.703 | 33.875 | 60.922 | -53.55 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 48 | 130.813 | 131.948 | 14.955 | -52.39 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 72 | 523.251 | 523.916 | 2.199 | -75.51 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 96 | 2093.005 | 2120.549 | 22.635 | -58.04 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 127 | 12543.854 | 8553.102 | -662.953 | -169.19 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 24 | 32.703 | 33.770 | 55.555 | -67.01 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 48 | 130.813 | 133.031 | 29.112 | -74.10 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 72 | 523.251 | 521.798 | -4.813 | -148.00 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 96 | 2093.005 | 2030.090 | -52.838 | -161.62 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 127 | 12543.854 | 8220.069 | -731.710 | -166.40 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | default | 24 | 32.703 | 32.043 | -35.282 | -59.56 | n/a | n/a | inharmonic physical modes |
| Modal | default | 48 | 130.813 | 130.741 | -0.946 | -124.28 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | default | 72 | 523.251 | 522.473 | -2.575 | -124.65 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | default | 96 | 2093.005 | 2091.286 | -1.422 | -126.78 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | default | 127 | 12543.854 | 8520.378 | -669.590 | -118.89 | n/a | n/a | inharmonic physical modes |
| Modal | maximum | 24 | 32.703 | 32.397 | -16.269 | -137.72 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | maximum | 48 | 130.813 | 130.351 | -6.129 | -139.39 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | maximum | 72 | 523.251 | 523.702 | 1.490 | -131.89 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | maximum | 96 | 2093.005 | 2059.283 | -28.120 | -138.63 | n/a | n/a | missing/weak fundamental; no ratio |
| Modal | maximum | 127 | 12543.854 | 8448.810 | -684.193 | -133.15 | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | default | 24 | 32.703 | 33.287 | 30.652 | -189.88 | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | default | 48 | 130.813 | 133.684 | 37.590 | -210.70 | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | default | 72 | 523.251 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | default | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | default | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Bass drum | maximum | 24 | 32.703 | 33.424 | 37.765 | -10.61 | n/a | n/a | pitch envelope and nonlinear percussion |
| Bass drum | maximum | 48 | 130.813 | 133.752 | 38.463 | -10.57 | n/a | n/a | pitch envelope and nonlinear percussion |
| Bass drum | maximum | 72 | 523.251 | 533.695 | 34.215 | -15.77 | n/a | n/a | pitch envelope and nonlinear percussion |
| Bass drum | maximum | 96 | 2093.005 | 2134.057 | 33.628 | -40.74 | n/a | n/a | pitch envelope and nonlinear percussion |
| Bass drum | maximum | 127 | 12543.854 | 8514.613 | -670.761 | -53.01 | n/a | n/a | pitch envelope and nonlinear percussion |
| Snare | default | 24 | 32.703 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Snare | default | 48 | 130.813 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Snare | default | 72 | 523.251 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Snare | default | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Snare | default | 127 | 12543.854 | 8375.372 | -699.307 | -178.75 | n/a | n/a | missing/weak fundamental; no ratio |
| Snare | maximum | 24 | 32.703 | 31.806 | -48.151 | -119.78 | n/a | n/a | noise or inharmonic percussion |
| Snare | maximum | 48 | 130.813 | 133.474 | 34.867 | -115.76 | n/a | n/a | noise or inharmonic percussion |
| Snare | maximum | 72 | 523.251 | 526.068 | 9.294 | -115.26 | n/a | n/a | noise or inharmonic percussion |
| Snare | maximum | 96 | 2093.005 | 2113.859 | 17.164 | -119.64 | n/a | n/a | noise or inharmonic percussion |
| Snare | maximum | 127 | 12543.854 | 8612.372 | -650.998 | -106.22 | n/a | n/a | noise or inharmonic percussion |
| Hi-hat | default | 24 | 32.703 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | default | 48 | 130.813 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | default | 72 | 523.251 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | default | 96 | 2093.005 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | default | 127 | 12543.854 | NaN | NaN | -inf | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | maximum | 24 | 32.703 | 33.875 | 60.922 | -155.42 | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | maximum | 48 | 130.813 | 133.075 | 29.687 | -140.26 | n/a | n/a | missing/weak fundamental; no ratio |
| Hi-hat | maximum | 72 | 523.251 | 512.721 | -35.194 | -119.65 | n/a | n/a | noise or inharmonic percussion |
| Hi-hat | maximum | 96 | 2093.005 | 2134.679 | 34.132 | -99.45 | n/a | n/a | noise or inharmonic percussion |
| Hi-hat | maximum | 127 | 12543.854 | 8359.326 | -702.627 | -79.61 | n/a | n/a | noise or inharmonic percussion |

### Modulation stepping, 48000 Hz

| Destination | Shape | Hz | Amount % | Carrier dBFS | Image 1 dBc | Image 2 dBc | Image 3 dBc | Image 4 dBc | Baseline max image dBc | Off-comb dBc | Off-comb Hz |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Pitch | sine | 1 | 0.1 | -18.34 | -104.00 | -124.73 | -140.83 | -152.47 | -100.52 | n/a | n/a |
| Pitch | sine | 10 | 0.1 | -13.56 | -100.81 | -119.73 | -136.24 | -147.64 | -100.52 | n/a | n/a |
| Pitch | sine | 40 | 0.1 | -13.52 | -100.39 | -117.61 | -131.78 | -143.64 | -100.52 | n/a | n/a |
| Pitch | square | 40 | 0.1 | -13.52 | -100.36 | -117.63 | -131.63 | -143.94 | -100.52 | -92.08 | 999.23 |
| Harmonics | sine | 1 | 10.0 | -14.07 | -52.91 | -125.74 | -149.33 | -152.32 | -30.96 | n/a | n/a |
| Harmonics | sine | 10 | 10.0 | -14.07 | -46.50 | -125.21 | -148.47 | -154.82 | -30.96 | n/a | n/a |
| Harmonics | sine | 40 | 10.0 | -14.07 | -44.37 | -125.57 | -147.63 | -153.67 | -30.96 | n/a | n/a |
| Harmonics | square | 40 | 10.0 | -14.07 | -51.68 | -107.55 | -135.39 | -152.55 | -30.96 | -17.85 | 238.33 |
| Timbre | sine | 1 | 10.0 | -13.53 | -70.47 | -117.76 | -131.31 | -143.41 | -100.52 | n/a | n/a |
| Timbre | sine | 10 | 10.0 | -13.53 | -70.47 | -117.77 | -131.35 | -143.00 | -100.52 | n/a | n/a |
| Timbre | sine | 40 | 10.0 | -13.53 | -70.45 | -117.63 | -131.39 | -143.40 | -100.52 | n/a | n/a |
| Timbre | square | 40 | 10.0 | -13.54 | -59.97 | -104.06 | -127.31 | -143.35 | -100.52 | -81.38 | 40.77 |
| Morph | sine | 1 | 10.0 | -14.37 | -31.22 | -115.53 | -130.42 | -141.84 | -30.96 | n/a | n/a |
| Morph | sine | 10 | 10.0 | -14.37 | -31.22 | -115.60 | -130.32 | -142.26 | -30.96 | n/a | n/a |
| Morph | sine | 40 | 10.0 | -14.37 | -31.22 | -115.50 | -130.40 | -142.33 | -30.96 | n/a | n/a |
| Morph | square | 40 | 10.0 | -14.38 | -31.51 | -108.43 | -131.19 | -143.92 | -30.96 | -82.39 | 4959.38 |
| Level | sine | 1 | 10.0 | -13.52 | -97.93 | -103.00 | -106.69 | -108.44 | -100.52 | n/a | n/a |
| Level | sine | 10 | 10.0 | -13.52 | -77.94 | -83.67 | -86.69 | -88.45 | -100.52 | n/a | n/a |
| Level | sine | 40 | 10.0 | -13.52 | -65.84 | -71.60 | -74.63 | -76.40 | -100.52 | n/a | n/a |
| Level | square | 40 | 10.0 | -13.52 | -56.22 | -64.55 | -68.19 | -70.25 | -100.52 | -78.12 | 320.71 |
| Pan (left) | sine | 1 | 10.0 | -13.80 | -100.37 | -109.41 | -112.42 | -114.19 | -100.52 | n/a | n/a |
| Pan (left) | sine | 10 | 10.0 | -13.80 | -83.68 | -89.41 | -92.42 | -94.19 | -100.52 | n/a | n/a |
| Pan (left) | sine | 40 | 10.0 | -13.80 | -71.58 | -77.34 | -80.37 | -82.14 | -100.52 | n/a | n/a |
| Pan (left) | square | 40 | 10.0 | -13.96 | -61.79 | -70.12 | -73.77 | -75.83 | -100.52 | -84.70 | 720.57 |
| Cutoff | sine | 1 | 10.0 | -13.59 | -95.14 | -107.69 | -116.14 | -122.83 | -100.52 | n/a | n/a |
| Cutoff | sine | 10 | 10.0 | -13.59 | -75.17 | -87.40 | -96.12 | -102.82 | -100.52 | n/a | n/a |
| Cutoff | sine | 40 | 10.0 | -13.59 | -63.03 | -75.29 | -84.03 | -90.73 | -100.52 | n/a | n/a |
| Cutoff | square | 40 | 10.0 | -13.67 | -53.89 | -67.29 | -76.81 | -84.36 | -100.52 | -75.22 | 720.59 |

### Shaper filter and drive, 48000 Hz

| Filter excitation | Carrier dBFS | Image 1 dBc | Image 2 dBc | Image 3 dBc | Image 4 dBc |
|---|---:|---:|---:|---:|---:|
| static | -16.51 | -115.66 | -139.03 | -152.63 | -163.37 |
| envelope | -13.53 | -98.88 | -107.48 | -117.14 | -124.43 |
| LFO sine 40 Hz | -20.43 | -48.06 | -61.73 | -70.25 | -76.78 |

| FM sine, MIDI key | Drive dB | Fundamental dBFS | Non-harmonic dBc | Hz |
|---:|---:|---:|---:|---:|
| 72 | 0 | -13.46 | -96.11 | 525.86 |
| 72 | 24 | -3.43 | -74.62 | 10849.17 |
| 96 | 0 | -13.68 | -92.41 | 2090.44 |
| 96 | 24 | -3.43 | -36.25 | 16604.93 |

### Residual and level scaling, 44100 Hz

| Model | Reference | f0 Hz | Fundamental dBFS | Residual dBc | Residual dBFS | Low fundamental dBFS | Low residual dBc | Low residual dBFS | Residual change dB | Guard 5->8 change dB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Virtual analog | tonal candidate | 261.625 | -14.84 | -54.08 | -68.92 | -54.84 | -54.08 | -108.92 | -40.000 | -0.001 |
| FM | nominal-band proxy | 261.625 | -23.79 | 9.82 | -13.97 | -63.79 | 9.82 | -53.97 | -40.000 | -0.000 |
| Inharmonic string | nominal-band proxy | 260.750 | -61.64 | -45.38 | -107.02 | -101.64 | -45.38 | -147.02 | -40.000 | -0.004 |

### Keyboard / tuning, 44100 Hz

| Model | Knobs | MIDI key | Ideal Hz | Candidate Hz | Cents | Reference dBFS | Strongest outside harmonics dBc | Hz | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Virtual analog | default | 24 | 32.703 | 32.704 | 0.019 | -14.85 | -83.94 | 13784.67 | candidate, not proven alias |
| Virtual analog | default | 48 | 130.813 | 130.812 | -0.009 | -14.85 | -71.91 | 13097.31 | candidate, not proven alias |
| Virtual analog | default | 72 | 523.251 | 523.252 | 0.001 | -14.84 | -60.32 | 15321.21 | candidate, not proven alias |
| Virtual analog | default | 96 | 2093.005 | 2093.004 | -0.001 | -14.86 | -48.77 | 12704.93 | candidate, not proven alias |
| Virtual analog | default | 127 | 12543.854 | 8372.018 | -700.000 | -16.81 | -38.56 | 10611.92 | candidate, not proven alias |
| Virtual analog | maximum | 24 | 32.703 | 32.703 | -0.022 | -110.97 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 48 | 130.813 | 130.799 | -0.182 | -131.23 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 72 | 523.251 | 523.249 | -0.008 | -132.11 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 96 | 2093.005 | 2093.004 | -0.001 | -41.19 | -15.60 | 10611.92 | candidate, not proven alias |
| Virtual analog | maximum | 127 | 12543.854 | 8372.018 | -700.000 | -24.25 | -33.90 | 11025.00 | candidate, not proven alias |
| FM | default | 24 | 32.703 | 32.982 | 14.689 | -19.30 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 48 | 130.813 | 130.812 | -0.012 | -23.78 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 72 | 523.251 | 523.252 | 0.003 | -23.80 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 96 | 2093.005 | 2130.956 | 31.111 | -18.76 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 127 | 12543.854 | 8372.018 | -700.000 | -17.74 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 24 | 32.703 | 32.704 | 0.048 | -42.05 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 48 | 130.813 | 130.812 | -0.013 | -39.61 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 72 | 523.251 | 523.252 | 0.003 | -29.52 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 96 | 2093.005 | 2093.004 | -0.001 | -13.76 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 127 | 12543.854 | 8372.018 | -700.000 | -17.74 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| Inharmonic string | default | 24 | 32.703 | 33.814 | 57.820 | -49.34 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 48 | 130.813 | 131.861 | 13.814 | -77.68 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 72 | 523.251 | 521.828 | -4.715 | -75.56 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 96 | 2093.005 | 2118.574 | 21.021 | -64.12 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 127 | 12543.854 | 8121.480 | -752.599 | -184.90 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 24 | 32.703 | 33.178 | 24.938 | -93.75 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 48 | 130.813 | 133.021 | 28.984 | -76.81 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 72 | 523.251 | 521.821 | -4.738 | -145.56 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 96 | 2093.005 | 2101.047 | 6.640 | -163.95 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 127 | 12543.854 | 8435.144 | -686.995 | -179.48 | n/a | n/a | missing/weak fundamental; no ratio |

### Residual and level scaling, 96000 Hz

| Model | Reference | f0 Hz | Fundamental dBFS | Residual dBc | Residual dBFS | Low fundamental dBFS | Low residual dBc | Low residual dBFS | Residual change dB | Guard 5->8 change dB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Virtual analog | tonal candidate | 261.627 | -14.85 | -71.30 | -86.15 | -54.85 | -71.30 | -126.15 | -40.000 | -0.031 |
| FM | nominal-band proxy | 261.628 | -23.78 | 9.86 | -13.92 | -63.78 | 9.86 | -53.92 | -40.000 | -0.816 |
| Inharmonic string | nominal-band proxy | 260.819 | -63.04 | -42.19 | -105.23 | -103.04 | -42.19 | -145.23 | -40.000 | -2.549 |

### Keyboard / tuning, 96000 Hz

| Model | Knobs | MIDI key | Ideal Hz | Candidate Hz | Cents | Reference dBFS | Strongest outside harmonics dBc | Hz | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| Virtual analog | default | 24 | 32.703 | 32.700 | -0.147 | -14.85 | -91.29 | 27.53 | candidate, not proven alias |
| Virtual analog | default | 48 | 130.813 | 130.810 | -0.033 | -14.85 | -87.17 | 15288.97 | candidate, not proven alias |
| Virtual analog | default | 72 | 523.251 | 523.254 | 0.009 | -14.85 | -71.35 | 19081.94 | candidate, not proven alias |
| Virtual analog | default | 96 | 2093.005 | 2093.002 | -0.002 | -14.84 | -59.54 | 18558.83 | candidate, not proven alias |
| Virtual analog | default | 127 | 12543.854 | 8372.017 | -700.000 | -15.00 | -60.15 | 12836.24 | candidate, not proven alias |
| Virtual analog | maximum | 24 | 32.703 | 32.696 | -0.377 | -109.11 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 48 | 130.813 | 130.828 | 0.207 | -120.19 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 72 | 523.251 | 523.263 | 0.038 | -128.93 | n/a | n/a | missing/weak fundamental; no ratio |
| Virtual analog | maximum | 96 | 2093.005 | 2093.002 | -0.002 | -57.42 | -22.49 | 8650.22 | candidate, not proven alias |
| Virtual analog | maximum | 127 | 12543.854 | 8372.017 | -700.000 | -29.43 | -45.35 | 12836.24 | candidate, not proven alias |
| FM | default | 24 | 32.703 | 32.071 | -33.785 | -29.90 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 48 | 130.813 | 133.253 | 31.999 | -21.72 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 72 | 523.251 | 523.253 | 0.005 | -23.79 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 96 | 2093.005 | 2130.957 | 31.111 | -18.53 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | default | 127 | 12543.854 | 8372.018 | -700.000 | -14.38 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 24 | 32.703 | 32.701 | -0.116 | -42.33 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 48 | 130.813 | 130.811 | -0.024 | -41.32 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 72 | 523.251 | 523.253 | 0.005 | -33.66 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 96 | 2093.005 | 2093.003 | -0.002 | -13.54 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| FM | maximum | 127 | 12543.854 | 8372.018 | -700.000 | -14.38 | n/a | n/a | noninteger FM ratio (default 12.16 semitones); feedback at maximum |
| Inharmonic string | default | 24 | 32.703 | 32.361 | -18.234 | -65.65 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 48 | 130.813 | 131.854 | 13.730 | -57.44 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 72 | 523.251 | 521.937 | -4.352 | -89.24 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | default | 96 | 2093.005 | 2105.350 | 10.181 | -137.19 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | default | 127 | 12543.854 | 8240.257 | -727.463 | -192.45 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 24 | 32.703 | 34.058 | 70.255 | -53.25 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 48 | 130.813 | 133.049 | 29.345 | -70.87 | n/a | n/a | inharmonic physical modes |
| Inharmonic string | maximum | 72 | 523.251 | 521.827 | -4.718 | -129.58 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 96 | 2093.005 | 2074.203 | -15.622 | -150.02 | n/a | n/a | missing/weak fundamental; no ratio |
| Inharmonic string | maximum | 127 | 12543.854 | 8510.517 | -671.594 | -169.55 | n/a | n/a | missing/weak fundamental; no ratio |

### Level and headroom, 48000 Hz

| Model | Held peak dBFS | Held RMS dBFS | Held full-scale samples | Chord peak dBFS | Chord RMS dBFS | Chord full-scale samples |
|---|---:|---:|---:|---:|---:|---:|
| Filtered analog | -14.06 | -14.88 | 0 | 4.00 | -5.82 | 6747 |
| Phase distortion | -9.10 | -11.76 | 0 | 8.69 | -2.14 | 27263 |
| Six-op FM 1 | -8.86 | -14.55 | 0 | 7.42 | -1.26 | 37677 |
| Six-op FM 2 | -8.98 | -28.54 | 0 | 5.47 | -19.96 | 161 |
| Six-op FM 3 | -13.56 | -19.55 | 0 | 2.98 | -10.51 | 387 |
| Wave terrain | -9.68 | -14.37 | 0 | 6.89 | -1.64 | 33384 |
| String machine | -14.19 | -23.47 | 0 | 0.63 | -11.08 | 22 |
| Chiptune | -12.02 | -12.70 | 0 | 6.04 | -3.71 | 17504 |
| Virtual analog | -9.63 | -14.07 | 0 | 6.48 | -4.42 | 12446 |
| Waveshaper | -11.35 | -15.40 | 0 | 4.37 | -6.84 | 3448 |
| FM | -10.44 | -13.52 | 0 | 6.91 | -4.53 | 12306 |
| Grain | -10.71 | -16.32 | 0 | 7.06 | -5.25 | 8629 |
| Additive | -9.66 | -18.64 | 0 | 3.12 | -9.70 | 316 |
| Wavetable | -7.34 | -20.92 | 0 | 10.52 | -10.88 | 83 |
| Chord | -8.00 | -14.86 | 0 | 8.99 | -5.46 | 7720 |
| Speech | -9.88 | -27.74 | 0 | 5.48 | -14.46 | 828 |
| Swarm | -6.00 | -17.94 | 0 | 9.06 | -8.68 | 859 |
| Filtered noise | -6.14 | -16.71 | 0 | 3.24 | -9.28 | 471 |
| Particle noise | -6.00 | -32.22 | 0 | 12.06 | -19.59 | 74 |
| Inharmonic string | -17.34 | -41.30 | 0 | -9.93 | -31.08 | 0 |
| Modal | -6.00 | -30.84 | 0 | 10.62 | -21.84 | 25 |
| Bass drum | -13.53 | -36.68 | 0 | 4.20 | -23.52 | 16 |
| Snare | -7.32 | -35.94 | 0 | 4.58 | -26.14 | 26 |
| Hi-hat | -8.21 | -36.65 | 0 | 1.42 | -27.55 | 2 |

### Capture sensitivity (same start, default C4; maximum knobs C7)

| Model | N | C4 residual dBc | C7 maximum non-harmonic dBc | Hz |
|---|---:|---:|---:|---:|
| Phase distortion | 131072 | -56.85 | 6.14 | 2371.21 |
| Phase distortion | 262144 | -56.86 | 6.14 | 2371.21 |
| Wave terrain | 131072 | -55.65 | -24.17 | 10324.74 |
| Wave terrain | 262144 | -55.65 | -24.17 | 10324.74 |
| Virtual analog | 131072 | -54.62 | -13.33 | 12418.92 |
| Virtual analog | 262144 | -54.62 | -13.33 | 12418.92 |
| Waveshaper | 131072 | -78.27 | -49.30 | 1953.90 |
| Waveshaper | 262144 | -79.05 | -49.30 | 1953.90 |
| Grain | 131072 | -84.38 | -74.64 | 12418.92 |
| Grain | 262144 | -85.24 | -74.60 | 12418.92 |
| Additive | 131072 | 40.61 | NaN | NaN |
| Additive | 262144 | 40.09 | NaN | NaN |

### Analysis calibration (synthetic signals, not instrument measurements)

Half-peak sine at ideal middle C; then the same sine with rounding at step 1/32768, before a unity output gain. The low row reduces the sine by 40 dB BEFORE rounding. Same FFT and harmonic mask as section1; this contrasts with the instrument's post-conversion level knob.

| Signal | Fundamental dBFS | Residual dBc | Residual dBFS | Frequency error cents | Strongest non-harmonic dBc | Hz |
|---|---:|---:|---:|---:|---:|---:|
| sine | -9.031 | -87.460 | -96.491 | 0.0053 | -90.875 | 259.067 |
| rounded sine | -9.031 | -86.275 | -95.306 | 0.0053 | -90.872 | 259.066 |
| rounded low sine | -49.030 | -52.496 | -101.526 | 0.0053 | -72.338 | 19270.937 |

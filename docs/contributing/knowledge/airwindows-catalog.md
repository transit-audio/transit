# Airwindows catalog survey

Evidence for [ADR 0042](../../adr/0042-stock-effects-airwindows-ports.md), surveyed 2026-11-02.
This is a source survey and selection, not a report of completed ports or measured nulls.
Chris Johnson's quoted source names below are his; transit device names are in the ADR.

## Source and version convention

The supplied original [Airwindows repository](https://github.com/airwindows/airwindows/tree/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a)
is pinned at **`7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a`**, dated 2026-10-31, commit subject
`PunchyDeluxe`. Its `plugins/LinuxVST/src/` has **532 effect directories**. Read the original
[LICENSE](https://github.com/airwindows/airwindows/blob/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a/LICENSE):
MIT, `Copyright (c) 2018 Chris Johnson`; source-file notices also remain intact.

For every row, the source is
[`plugins/LinuxVST/src/`](https://github.com/airwindows/airwindows/tree/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a/plugins/LinuxVST/src)
plus `<Name>/<Name>.cpp`, `<Name>/<Name>Proc.cpp` and `<Name>/<Name>.h`.
The first supplies constructor defaults, parameter names/displays and chunk handling; `Proc.cpp`
contains separate float and double processing functions; the header declares counts and state.
The tables transcribe `getParameterName` in source order, not names inferred from publicity.
The numeric suffix is the revision when it denotes one. **U** means unnumbered at this exact pin,
not an invented version 1. Model numbers 140 and 240 are plate types, not version comparisons.
The wrappers commonly return vendor version 1000; that is not useful revision provenance.

[Airwindopedia.txt at the same pin](https://github.com/airwindows/airwindows/blob/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a/Airwindopedia.txt)
is Johnson's compilation of his own release notes. Sections named after the effects support the
selection. They sometimes retain historical descriptions before later updates; the pinned source
controls parameter counts and arithmetic. The "TPDFDither" section, for example, first describes
no controls and later its Redux controls; the source has two. The "NotJustAnotherDither" followup
says the latest variant is inside "Monitoring", so it is not presented here as the latest
noise-shaping algorithm available anywhere in the catalog.

## Common mechanics

- Controls are normally normalized **f32 0–1**, held in `A`, `B`, etc. Meaning and nonlinear
  mapping are in the processing function; a label like Ratio is not proof of a calibrated ratio.
  Discrete controls use integer casts or thresholds. Transit retains normalized values and maps
  its display separately; the ADR specifies the units, lists and exceptions.
- A processing call derives coefficients and then executes `while (--sampleFrames >= 0)` over
  stereo samples. Most arithmetic is double even in `processReplacing(float**)`; output rounds
  to float. Its companion double-output path often advances the RNG but comments out output
  dither. They are different reference paths. transit's f32 insert follows the float path.
- **F** below means per-instance `uint32_t fpdL/fpdR`, initialized by repeated constructor
  `rand()*UINT32_MAX` until at least 16386. The per-sample xorshift uses shifts 13, 17, 5.
  Near-zero input is commonly replaced with `fpd * 1.18e-17` below `1.18e-23`; float output
  usually gets exponent-scaled dither using `frexpf`, `5.5e-36l` and `pow(2, expon+62)`.
  F is a common convention, not a promise of identical noise code: "Density3" uses a smaller
  dither constant, and "ADClip9" disables the float-output additions; both can advance the right
  RNG an extra time as their rows detail. Preserve each source's actual operations. Seeding must occur
  before the first sample, and reset must reproduce it. F does not mean stateless, transparent
  at silence or equivalent to a word-length dither plug-in. The suffix `l` needs a compiler-specific
  rounding audit, especially GCC x86-64 extended precision.
- **T** is the selected output dither: the same instance seed words but three RNG advances per
  channel/sample, two uniform additions, floor quantization and no extra float-output dither.
  **N** is the deferred noise-shaped quantizer: F-style seed words advance once per channel/sample,
  with denormal injection, bin statistics and noise shaping instead of ordinary output dither.
- **Yes** in the rate column means the rate influences processing. **No** means no effective
  rate compensation, not equal sound in seconds: a fixed sample delay becomes shorter and a
  fixed phase increment becomes faster as the rate rises. `r` means `sample_rate / 44100`.
  `cycle` means `clamp(floor(r), 1, 4)`, not unlimited high-rate compensation.
- All selected processors' mutable processing histories are instance members; no mutable
  process-function statics were found. Constructors' global `rand()` is the shared-state
  exception replaced by deterministic seed patches. Immutable table/size constants are harmless
  shared data. The deferred "DeEss" is an explicit mutable-static exception below.

## First set

These are the 12 selected inserts, in slice order. The 2026-11-02 amendment admits stability
patches: Tape Color and Wave Saturator join slice 1, and Large Plate joins Small Hall in slice 3.
Sibilance Control is deferred because its overflow occurs inside normal use; see the DeBess
row and counterexample below. The de-essing gap remains open.
When the replacements pass their gates and land, slice 1 retires Saturator and slice 3 retires
Room Reverb and Plate Reverb under ADR 0040. No retirement happens in this documentation change.
Each row gives the state that the port's reset,
floor and scheduling fingerprint must cover, in addition to its parameters and rate.

| Source / version | Count and exact parameter names | Sample-rate dependence | State and seed notes | Gap and selection evidence |
|---|---|---|---|---|
| "Chorus" / U | 3: Speed, Range, Dry/Wet | **Yes:** phase increment `A^4 * .001 * r`; range is a fixed sample-buffer fraction `B^4 * loopLimit * .499`. Speed in Hz therefore also changes with rate; treble processing is sample-based. | F; two float delay arrays of 16386, shared sweep/count, separate L/R treble histories and flip flag. Same modulation on both sides; no invented stereo spread. | Single moving-tap modulation; notes call it mono chorus that can serve as vibrato. |
| "ChorusEnsemble" / U | 3: Speed, Range, Dry/Wet | **Yes:** phase increment `A^3 * .001 * r`; range `B^3 * loopLimit * .12` in samples; four related tap offsets. | F; two 16386-float buffers, sweep/count, treble histories and flip flag. | Four-tap ensemble for pads, not a second single-tap flavor. The author's section explicitly distinguishes the bank of taps. |
| "Pressure6" / 6 | 2: Compres, Ratio | **Yes:** detector speed target `((A*92)+92)*r`; threshold `1-(.9*A)^3`. | F; shared stereo-linked `muComp` and `muSpd`, initialized 1 and 100. Ratio maps to `wet=.9*B`, used inside a nonlinear blend, not an ordinary ratio in x:1. | Character bus compression; latest Pressure line and explicitly aimed at that job in its notes. |
| "ToTape9" / 9 | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | **Yes:** encoder/decoder coefficients `(1-C)/r` and `C/r`, now capped at 1 by the stability patch; bias, head-bump and flutter formulas use `r`; depth capped at 498 samples; clip spacing `clamp(floor(r),1,16)`, slew span `clamp(floor(2*r),2,32)`. Head frequency `(H*H*175+25)/rate`. | F also drives flutter and clipping noise; encoder/decoder envelopes, IIR/biquads, two 1002-double flutter buffers, phases/targets, bias stages, hysteresis, averaging histories and clip/lookahead state. Preserve the descending intermediate/slew copies; establish latency from output, not the spacing comment. | Slice 1, Tape Color in `transit-tape-color`. Latest tape revision; notes describe the replaced core, brightness compression and new output clipper. Both companding coefficients are bounded to close the low-rate runaway; patch/proof table below. |
| "Density3" / 3 | 4: Density, Highpas, Output, Dry/Wet | **Yes:** high-pass amount `B^3/r`, now capped at 1 by the stability patch; zero high-pass still clears its histories at block entry. | F **exception:** float dither uses `3.553e-44l`, not `5.5e-36l`; right RNG advances a second time when either unsigned L-minus-R or R-minus-L is below 1073741824. L/R IIR memories; internal `density=A*5`, display `A*5-1`. | Slice 1, Wave Saturator in `transit-wave-saturation`. Preferred saturation revision in the author's notes; coefficient patch closes the low-rate recurrence. Its separate extreme-input polynomial/output-conversion audit remains an admission gate; see below. |
| "Console9Channel" / 9 | 2: Pan, Fader | **No** `getSampleRate`; **block-dependent** pan/fader interpolation using remaining frames / original call length, including first call. | F; `panA/B`, `inTrimA/B`. Encode curves use `log1p`/`expm1`; raw constructor endpoints stay 0.5. The Plugin adapter snaps both pan endpoints to `A*1.57079633` and both trim endpoints to `B*2.0` on initialization/reset, separately nulled with the same harness operation. | Latest numbered standalone summing encoder. Notes distinguish the summing core from full processing strips. |
| "Console9Buss" / 9 | 2: Pan, Fader | **No** rate query; same block-based control ramps. | F; pan/trim endpoint memories; complementary decode curve. Same raw constructor and explicit adapter snap/reset rule as the channel. | Must pair with the selected channel algorithm after the sum; no cross-generation pair. |
| "ADClip9" / 9 | 5: Boost, Match, Noise, Ceiling, Mode | **Yes:** sample spacing `clamp(floor(r),1,16)` enters clip processing and slew tests; it is not the measured latency. | F **exception:** float-output dither additions are commented out; both RNGs still advance, with a second right advance when either unsigned L-minus-R or R-minus-L is below 1073741824. RNG also supplies clipping noise and denormal injection. Last output/dry sample, 18-element intermediate and 34-element slew arrays per side, clip flags. Preserve the descending intermediate/slew copies; establish latency from output, not the spacing comment. **Constructor omits `lastDryL/R` initialization; deterministic patch required.** | Latest clipper revision. Source has three modes, not the predecessor's eight. Noise/Ceiling are nonlinear thresholds, not dB labels. |
| "TPDFDither" / U, Redux | 2: Quant, DeRez | **No:** fixed word-length scaling, independent of sample rate. | T; only parameters and RNG words persist. Quant selects scale 32768 or 8388608; DeRez multiplies by `(1-B)^6`, scale floor `.0001`, output divisor floor 8. | One general output dither. Notes recommend it when no special shaping is needed and acknowledge its half-LSB offset; preserve the actual floor/addition sequence. |
| "Slew4" / 4 | 1: Compres | **Yes:** `compresity=(.95*A)^2*8*r`; filter banks and lookahead remain in samples. | F; L/R 24-by-12 averaging storage, slew sustain, second/third samples, shared count; explicit two-sample lookahead. The final fractional-depth blend uses the right channel's `depthTrim` for both channels; preserve this coupling. | Latest brightness compression without saturation; distinct standalone use even though tape includes related processing. |
| "kGuitarHall2" / 2 | 6: Regen, Derez, Filter, EarlyRF, Positin, Dry/Wet | **Yes:** revised Bezier undersampling: `B*2` folds about center, divides by `r`, clamps, then left half locks to reciprocal integer subdivisions while right half stays continuous with rate correction. | F; 3-by-3 early and 6-by-6 main delay networks, counts, feedback, Pear-filter and Bezier histories. Immutable `early[36]`; `int(E*27)` chooses 28 starting positions over nine entries. `c6ZAR` remains at its initialized index, while `c6ZBR` advances twice per internal update; preserve the source's counter behavior. | A small hall with source positioning; author's notes describe its hall/arena goal and the new Position control. |
| "kPlate140" / U, model 140 | 5: Input, Regen, DeRez, Predlay, Wetness | **Yes:** `derez=C/r`, clamped `.0005..1`, then reciprocal-integer locked; predelay `int(16000*D*derez)` at the internal update rate. Filter/size behavior follows that clock. | F; active state includes 5-by-5 matrix delays/counts, feedback, low-frequency damping, vibrato, predelay/subdelay and Bezier histories. Six SubTight histories now bounded to [-1, 1] by the stability patch. `gainIn`, `gainOutL/R` and `pearA`–`pearF` are declared and initialized but unused in both processing paths, not active gain/Pear stages. | Slice 3, Large Plate in `transit-space-reverb` beside Small Hall. Preferred next-generation large plate in the author's notes; history clamp closes the hot-input SubTight recurrence. At Wetness .5 both dry and wet gains are 1; below .5 wet gain is squared. |

## Deferred candidates and predecessors

Deferred means no additional first-set insert. A predecessor is kept in this survey for the choice's
audit trail, not queued for a duplicate port. The original issue's example versions were investigated
even where the selection moved to a later version. F/N meanings are defined above.

| Source / version | Count and exact parameter names | Sample-rate dependence | State and seed notes | Gap / why deferred |
|---|---|---|---|---|
| "DeBess" / U | 5: Intense, Sharp, Depth, Filter, Sense | **Yes:** intensity `A^5*8192/r`; sharpness `max(2,40*B)` samples, speed `.1/sharpness`; Filter itself is not rate-scaled. | F; 41-element sample/comparison arrays per side, alternating ratios/IIRs and flip. No legacy static noise counters. On the B phase, the condition tests `ratioAL/AR` but divides by `ratioBL/BR` in both precision paths. Sense above `.49999` outputs input minus processed. | Sibilance Control deferred: valid one-frame control changes with +/-1 input at 44.1 kHz overflow f32 at frame 3459 in the source recurrence below. A bound would activate inside normal use, contradicting this amendment's transparency contract. Admission needs a later decision on bounded B-phase division and an explicit transparency exception. The author's recommendation over "DeEss" and longer detection window do not resolve this defect; high-rate, bandwidth-limited material can also trigger it less. |
| "Console7Channel" / 7 | 1: Fader | **Yes:** 20 kHz biquad normalized by rate; gain chase capped by `sampleFrames`. | F; gain chase/speed and one stereo biquad. Block-length dependent. | Earlier console encoder; selected 9 gives the latest standalone pair. |
| "Console7Buss" / 7 | 1: Master | **Yes:** two 20 kHz biquads; chase capped by call length. | F; gain chase/speed, two stereo biquads. | Earlier matching decoder, not an alternative for the selected encoder. |
| "ToTape8" / 8 | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | **Yes:** `r` scales flutter, bias and filters; clip spacing capped at 16; depth capped at 498. | F; 1002-sample flutter buffers, companding/bias/head-bump filter state, 16-element clip intermediates. Boundary access needs audit if ever ported. | Tape predecessor; "ToTape9" notes explicitly describe the improved replacement core. |
| "Density2" / 2 | 4: Density, Highpass, Output, Dry/Wet | **Yes:** high-pass `B^3/r`; reconstructed halfway samples and alternating correction histories are sample-based. | F; three previous samples, correction accumulators and two IIR histories per side. | Saturation predecessor; "Density3" is the author's revision, not an arbitrary same-sounding substitute. |
| "PurestDrive" / U | 1: Drive | **No:** sine saturation blended using current/previous sample behavior. | F; previous sample per side. Header's unused B–E do not make this a five-parameter effect. | Simpler saturation candidate for a later decision; not an alternative to selected "Density3" without separate review. It needs its own full admission evidence before replacing the inherited Saturator. |
| "DeEss" / U | 3: Intense, Max DS, Freq | **Yes:** intensity and IIR amount divide by `r`. | F plus **function-static** `noisesourceL=0`, `noisesourceR=850010` in each precision path. Residue computation squares signed ints beyond their range; static counters leak across instances. Detector histories, ratios, IIRs and flip are instance state. | De-esser predecessor; author recommends "DeBess". Any later port must resolve static ownership and overflow explicitly rather than seed only `fpd`. |
| "StereoChorus" / U | 2: Speed, Depth | **Yes:** processing every `cycle` frames with reconstruction between; different control laws from "Chorus". | F; stereo delay/phase and air-compensation histories, cycle and output references. | Stereo widening variant; first set chooses a simpler independent speed/range/mix design, not a claim that this variant is obsolete. |
| "StereoEnsemble" / U | 2: Depth, FXlevel | **Yes:** `cycle` division and reconstructed output; delay factor `.66+A/3` changes fixed sample lengths. | F; many paired delay banks, maxima/counts, feedback and reconstruction state. | Another ensemble topology; avoid a second ensemble in slice 1. |
| "Vibrato" / U | 5: Speed, Depth, FMSpeed, FMDepth, Inv/Wet | **No:** increments `(0.1+A)^6` and `(0.1+C)^6` per sample; depths derive from their square roots; fixed sample buffers. | F; two 16386-double buffers, two sweeps, shared position, treble histories and flip. Wet spans -1..1. | Dedicated FM vibrato later; the first chorus already has a wet pitch-modulation use. |
| "Flutter2" / 2 | 3: Flutter, Speed, Dry/Wet | **Yes:** depth `A^4*r*90`, capped 498; rate `.02*B^3/r`. | F also updates random flutter targets; two 1002-double buffers, L/R phases/targets and shared count. | Standalone tape pitch variation, not a complete tape coloration stage. Remains a later modulation candidate alongside selected Tape Color; not a substitute for the full tape processor. |
| "PhaseNudge" / U | 2: Phase Nudge, Dry/Wet | **No:** target allpass delay `int(A^3*1501)` samples; per-sample chase. | F; two 1503-double allpass buffers, position and current maximum delay. | Phase rotation/diffusion, not a self-modulating phaser. Keep the dedicated phaser gap explicit. |
| "Capacitor2" / 2 | 4: Lowpass, Highpass, NonLin, Dry/Wet | **No:** source explicitly avoids scaling so coefficient 1 can bypass; cutoff and chase times therefore vary with rate. | F; six high-pass and six low-pass histories per side, cycle count, chase targets/current values and prior controls. | Nonlinear tone filter; existing Filter and planned EQ make it lower priority. |
| "PurestGain" / U | 2: Gain, Slow Fade | **No effective dependence:** computes `overallscale` but never uses it. Gain chase and 4000/4001 fade smoothing are sample-based. | F; gain/setting/fade chase and speed. At exactly unity gain copies original input and does not advance dither state. | Gain utility later; retain existing Gain and its test role until separately replaced. Source Gain display is -40..40 dB. |
| "BitShiftGain" / U | 1: BitShift | **No:** `int(A*32-16)` selects exact power-of-two gain. | No RNG, dither or persistent DSP history; parameter only. | Stepped gain utility, -16..16 shifts. Too narrow to replace the general gain device alone. |
| "ADClip8" / 8 | 3: Boost, Ceiling, Mode | **Yes:** spacing `clamp(floor(r),1,16)`; mode selects stage count. | F; eight stages of last samples, 16-by-8 intermediate arrays and clip flags per side. | Predecessor with eight modes; superseded by selected 9 for this job. Audit index 16 against 16-element buffers if revived. |
| "NotJustAnotherDither" / U | 2: Quant, DeRez | **No:** same 16/24-bit and DeRez scale concept, no rate query. | N; 13-bin distributions and noise-shaping error per channel. | Noise-shaped/Benford word-length reducer. Notes say it is not conventional added-noise dither and point to a newer embedded variant; postpone instead of shipping two output dithers. |
| "Slew3" / 3 | 1: Clampng | **Yes:** threshold `(1-A)^4/r`. | F; three sample-history values per side. | Earlier slew limiter; source inspected, newest line chosen. |
| "Galactic" / U | 5: Replace, Brightness, Detune, Bigness, Dry/Wet | **Yes:** `cycle` division, low-pass `/sqrt(r)`; fixed-delay size scaling. Above the cycle cap, time behavior changes. | F also affects modulation; stereo delay matrices, feedback, modulation buffers/phase, filters and reconstruction histories. | Ambient space predecessor. Author's "Galactic3" notes recommend 3 as the update to this topology. |
| "Galactic2" / 2 | 4: Drive, Sustain, Darken, Wetness | **Yes:** `cycle` division; block-entry input/wet endpoints interpolate across each call. | F; stereo cross-oriented delay matrix/counts, feedback, filter and interpolation histories. Author's later notes report four previously uninitialized variables were fixed. | An ambient variant, not the hall default. Notes warn high Sustain can grow indefinitely; 3 is a return to the original topology rather than an exact replacement for 2. |
| "Galactic3" / 3 | 6: Replace, Brightns, Detune, Derez, Bigness, Dry/Wet | **Yes:** Derez `D/r`, clamped and reciprocal-integer locked; Bezier reconstruction, low-pass `/sqrt(r)`, sample-based detune. | F plus modulation's `oldfpd` history; matrices, indices, feedback, IIRs, variable delay lengths and Bezier state. | Preferred later ambient space. Notes recommend 3 over the original for undersampling; room-size motion can make abrupt sounds. It is not selected as a natural hall. |
| "Verbity" / U | 4: Bigness, Longness, Darkness, Wetness | **Yes:** `cycle` division; low-pass `/sqrt(r)`, fixed buffer lengths times size. | F; dual-mono delay networks, feedback, IIR/thunder and reconstruction history. | Earlier compact room/ambience candidate; survey successor before a later room slice. |
| "Verbity2" / 2 | 4: RmSize, Sustain, Mulch, Wetness | **Yes:** `cycle` division; high/low-pass `/sqrt(r)` and fixed-delay size control. | F; 25-delay network per side, counters, feedback crossfeed, IIR/interpolation histories. | Larger ambience topology, dual-mono at no feedback and crossfeeding as sustain rises per notes; not a second first-set hall. |
| "kPlateA" / U, model A | 5: InputPad, Damping, Low Cut, Predelay, Wetness | **Yes:** `cycle` division; `downRate=rate/cycle` sets filters and `int(downRate*D^2*.5)` predelay. Cycle cap makes very high rates require a predelay bounds audit. | F; early reflections, 5-by-5 delays/counters, feedback, gain/IIR/biquad and reconstruction histories. | Earlier plate candidate. Not selected alongside "kPlate140"; needs its own admission evidence before any later port. |
| "kPlate240" / U, model 240 | 5: Input, Regen, DeRez, Predlay, Wetness | **Yes:** rate-divided reciprocal-integer DeRez clock and scaled predelay, like model 140, with different feedback/blur coefficients. | F; its own 5-by-5 delays and feedback, damping/gain, Pear filters, modulation, predelay/subdelay and Bezier state. | Author describes a smaller, cloudier plate. Different model, not newer because 240 exceeds 140; remains deferred and needs its own admission evidence, not an automatic substitute for model 140. |

## Stability patches admitted 2026-11-02

The [ADR amendment](../../adr/0042-stock-effects-airwindows-ports.md#amendment-2026-11-02-stability-patches)
defines one admitted rate/input domain for the entire set, the per-effect normal domains and
fixtures N1–N3. The table below is a specification, not a report of implemented patches or nulls.
Each patch must name the variable, bound and reason in its own comment and the oracle's `VENDOR.md`,
and the build applies it only to copies under the build directory. The patched C++ becomes the
Rust port's reference across admitted-domain fixtures; the reference without stability patches
remains a second reference for every normal-domain sweep, long floor/null and lifecycle scenario.
Both variants share deterministic seeds and necessary initialization/UB definitions.

| Effect | Variable | Bound and placement | Counterexample closed / reason | Normal-domain proof |
|---|---|---|---|---|
| "ToTape9" / Tape Color | `iirEncFreq`, `iirDecFreq` | Cap each at 1.0 immediately after its original coefficient expression, before IIR/envelope use; preserve values at or below 1 and zero. | At 8 kHz and Shape = 1, `C/r = 5.5125` gives the decoder history multiplier -4.5125 and the sample-472 infinity below. The cap makes the multiplier zero and all four IIR/envelope recurrences convex. Include Shape = 0 for the mirror encoder case. | Normal domain: the six normal rates, peak input <= 1, all controls. Both coefficients are already in [0, 1], so the patch is inactive. Require N1 `normal_full_sweep`, N2 `normal_long_floor_and_null`, N3 `normal_lifecycle_and_schedules` against the unpatched reference, unchanged coefficient bits and zero activations. |
| "Density3" / Wave Saturator | `iirAmount` | Cap at 1.0 after `B^3/r`, before the existing zero reset and both history updates. | At 8 kHz, Highpas = 1, the old residual grows as `4.5125^n`, overflowing f32 by sample 60. At coefficient 1 each history equals input, residual zero. The cap removes that runaway without changing the zero-reset rule. | Normal domain: the six normal rates, peak input <= 1, all controls. `B^3/r <= 1`, so no activation. Require N1–N3 against the unpatched reference, including both sides of internal density 1 and 2. The extreme-input polynomial/output-conversion issue below is separate. |
| "kPlate140" / Large Plate | `subAL`, `subAR`, `subBL`, `subBR`, `subCL`, `subCR` | Clamp each new `outSample*scale` history to [-1.0, 1.0] at its write. Keep the outgoing temporary and downstream output clamps unchanged. | Constant `2000.0f` at 44.1 kHz previously drove `subCL` infinite at sample 51 and output NaN at sample 53. Each next update now starts with bounded history; `abs(v) <= 1+s` bounds the three successive stage outputs and their binary64 products for finite f32 input. | Normal domain: the six normal rates, peak through +12 dBFS, all controls. With conservative input bound 4, the ADR derives history invariants .01, .022, .06, all below 1. Require N1–N3, zero activations, all six history bits, maximum Regen, slowest DeRez, longest predelay, control jumps and fully exercised tails. |

"The six normal rates" means the ADR's normal-domain rate list; +12 dBFS is approximately
3.981072 in sample magnitude, not an admitted-input ceiling. N1 includes full parameter sweeps
and signed silence/subnormal, impulse, DC, alternating, unequal-channel, tone and noise fixtures.
N2 requires at least 60 seconds of sample time with 10 seconds of silence, extended until delay
paths fill and wrap twice. N3 covers raw/adapter reset, prepare, bypass, isolation and matched
block/event schedules. Each runs at all normal rates with zero differing output/state/seed bits;
both C++ variants also self-floor and compare directly. No measured result exists yet.

The three recorded failures remain mandatory unstrided negative checks with each patch removed.
Whole-domain finite-state/output analysis and extreme fixtures remain required for every effect;
these three local recurrence fixes alone are not a certificate for every finite input. Additional
necessary bounds must be recorded here with the same proof before admission, within the ADR's
coefficient/history/divisor scope. In particular, Density3's below-unity-density polynomial can
produce an out-of-range f32 result even with a stable high-pass history: controls `[0, 0, 1, 1]`
and `1.0e10f` input give about `2.7557e43` before dither (a source calculation, not an oracle run);
maximum-f32 dry output plus dither also needs range analysis, including in the plate. Neither
shrinking the fixtures nor muting a nonfinite result in the adapter is an acceptable repair.

Each patched help entry says: "transit bounds `<variable>` so the effect stays finite at extreme
settings; within normal use it is sample-identical to Chris Johnson's original." Substitute the
actual bounded variables, disclose the effect's normal rates and input/headroom limit, and publish
that claim only after the dual nulls pass. Taste, retuning, oversampling, latency and CPU patches
remain forbidden. A different revision needs separate provenance and admission analysis.

## What the survey changes about the implementation

**Seeded does not mean zero residual is optional.** Test fixtures set both seed words and all
initialization state before processing. Self-floors run in one process and a child process;
instance interleaving tests catch hidden shared state. Nulls compare all f32 output bits and final
seed words against the same compiler's float path with its recorded patch set. Stability-patched
effects additionally null against the unpatched float path on N1–N3. Both patch sets and build
variants are recorded in `VENDOR.md`; no full null result exists yet for these proposed ports.

**Initialization must be audited.** "ADClip9" is a specific finding, not a hypothetical warning:
its header declares `lastDryL/R`, its constructor initializes the clip history but not those two
members, and each processing path first assigns `overshootL/R = lastDryL/R` before storing new
input. Define their initial values as zero in a documented build-overlay patch and port that
definition. Several constructors initialize only the used prefix of larger arrays; an untouched
spare element is not itself a defect. Prove index bounds and read-before-write behavior across
parameter/rate extremes before adding patches. The old "DeEss" residue's signed multiplication
overflow is a separate problem from F's unsigned xorshift arithmetic.

**Block shape matters.** Both selected console processors use
`temp = (double)sampleFrames / inFramesToProcess` after decrementing the frame count. They move
the prior target into the A endpoint at call entry, even on a fresh instance. Preserve that exact
sequence. "Console7" caps chase speed by call length, and "Galactic2" has block ramps too.
Per-sample processing does not authorize block restructuring. Raw constructor nulls keep the
original 0.5 endpoints. At the Plugin boundary, stored parameters are initial state, as required
by `Plugin::snap_initial_params` in `crates/dsp/src/lib.rs` and its caller in
`crates/mix/src/instance.rs`. Both console adapters set both pan endpoints to `A*1.57079633`
and both trim endpoints to `B*2.0` before the first sample; the reference harness exposes this
explicit operation separately from raw construction. Prepare and reset preserve current controls,
restore default seeds and snap endpoints; the post-restoration hook only snaps endpoints. This
prevents reset from reintroducing the constructor ramp after restoration. Subsequent automation
retains the original ramps. Adapter nulls replay identical initialization, parameter timing and
call sizes; scheduling equivalence uses the same schedule. Test a restored Fader = 0 at 64 frames:
without the snap, the first interpolated trim is 0.4921875 instead of 0 (output dither still stays).

**Physical units must be earned.** "Pressure6" Ratio is a nonlinear blend amount, not a measured
compression ratio. For any later admission of deferred "DeBess", Sense's display labels disagree
with its delta-monitoring branch: `E > .49999` emits input minus processed, so label Listen as
Processed / Removed (0 / 1). Sharp is not purely stepped because fractional sharpness also affects
speed. These are survey notes, not a scheduled implementation. Hall Position really selects 28
positions. The selected plate's Wetness has unity dry and wet at the midpoint. Predelay's exact seconds
depend on integer truncation and the internal clock. Percentage travel is an honest display for uncalibrated
controls; the ADR's specified dB/list mappings are exceptions backed by source formulas.

**Defined quirks stay.** In `Slew4/Slew4Proc.cpp`, the left channel computes `depthTrim` at
line 37, the right overwrites it at line 191, and both final blends use that right-derived value
at lines 345–346. The double path repeats this at lines 394, 548 and 702–703. Separate variables
would change stereo coupling and fail fidelity; unequal-channel null scenarios must exercise it.
In `kGuitarHall2/kGuitarHall2.cpp:76`, `c6ZAR` and `c6ZBR` both start at 1. In its `Proc.cpp`,
`a6ZAR` is written/read at the unchanged `c6ZAR` index (lines 376 and 390), while `c6ZBR`
advances twice (lines 339 and 383). The double path repeats this (lines 832, 869, 876 and 883).
These are initialized, bounded accesses; preserve them and exercise the affected wet paths in
the nulls. They are not permission for an oracle cleanup patch.

In `ADClip9/ADClip9Proc.cpp` and `ToTape9/ToTape9Proc.cpp`, both precision paths write
`intermediate[spacing]` and copy downward in descending index order. Each iteration propagates
the just-written value instead of shifting an ordinary delay line. The slew arrays use the same
descending propagation. On the unclipped clipper-stage path, `lastSampleL/R` becomes the current
sample and is emitted on the next frame, even when `spacing > 1`. Preserve the copies; establish
latency from output, not the spacing comment. Gate scenarios compare the intermediate/slew state
at `spacing > 1` to distinguish these operations from a conventional shift. The tape's complete
latency still includes analysis of its other stages, separately from this clipper-stage behavior.

**Normal-domain overflow blocks DeBess.** `DeBess/DeBess.cpp` initializes all four ratios to 1,
the IIR/detector histories to zero and `flip=false`. In both `DeBess/DeBessProc.cpp` paths, the
B branch tests the A ratios but divides by the B ratios. Round 4 recorded this asymmetry as a
preservation note; round 5 demonstrates that it prevents admission under the stability amendment.
At 44,100 Hz, set B=C=D=E=0 and use one frame per call: odd frames A=0, input +1; even frames
A=1, input -1, on both channels. Sharpness=2, speed=.05, depth=10000, IIR amount=0. Sense is 0
on odd frames and 8192 on even frames. The A ratios exceed 1 from frame 2 onward; on odd frame
`2k-1` the B ratios are `.95^k`, so after the first frame pre-dither output is `1/(.95^k)`.
At frame 3459, a binary64 evaluation yields `ratioBL=2.8962573551544587e-39` and
`3.452731844496842e38` output, exceeding finite f32 before the cast passed to `frexpf`. The
right channel is identical. Denormal injection never applies to these inputs and output dither
does not enter the detector. This is a source-derived result, not a compiled oracle measurement.

Sibilance Control remains deferred pending a decision defining bounded B-phase division and
the corresponding exception to normal-domain transparency. Keep this exact schedule for any
future admission regression on both channels; no silent branch correction, restriction on valid
automation, adapter mute or substitution of "DeEss" is authorized. Its prospective transit id
`sibilance_control` is not scheduled for registration, and the inherited de-esser remains retired
under ADR 0040. The three selected stability patches remain subject to the unchanged dual-null
contract and may only change results outside normal use.

**High rates are in scope now.** The ADR's single admitted-domain definition traces the engine
interval, common session rates and playback device choices, and requires implementer confirmation
of every render entry point. The selected high-rate floors/nulls include 705.6/768 kHz and the
low-rate/boundary probes specified there, retaining 1 kHz as extra robustness coverage rather
than assuming import or DSP-sanitizer limits are session limits.
Array bounds, integer-divider transitions, long delay warmup and sample-based tone changes need
evidence. Neither running only 48 kHz nor silently rate-converting the source satisfies the decision.

**Low-rate stability requires a reference patch.** "Density3" is a concrete counterexample to
admitting every selected source unchanged. Its high-pass update is
`history = history*(1-iirAmount) + input*iirAmount`. At 8 kHz and Highpas = 1,
`iirAmount=5.5125`; with constant input 1 and zero history, the high-pass residual's magnitude is
`4.5125^n` after n samples. Density = 0.25 makes internal density 1.25, retaining 75% of that
residual alongside bounded saturation. With Output = Dry/Wet = 1, by sample 60 the unbounded
contribution is about `1.38e39`, beyond finite f32 output. This analytical source counterexample
requires no wall-clock assertion or compiled oracle claim. A coefficient clamp would alter
defined DSP, not repair an unwritten read. The amendment now authorizes that change explicitly:
cap `iirAmount` at 1, so the recorded corner's residual is zero. Wave Saturator returns to slice 1;
the inherited Saturator retires only after its replacement passes the whole-domain and dual-null gates.

"ToTape9" provides a second required patch. In `ToTape9/ToTape9Proc.cpp`, the encoder and decoder
coefficients are `(1-C)/r` and `C/r`. At 8 kHz and Shape = 1, the decoder's
`iirDecL/R` and `compDecL/R` updates multiply their old histories by -4.5125. The clamp on
`highPart` bounds the input to the companding envelope, not its accumulated history. The
constructor in `ToTape9/ToTape9.cpp` initializes IIR/average histories to zero and companding
histories to one, so this is not an unwritten-state defect.

The judging round 2 source-derived binary64 trace used fresh state, constant stereo `0.1f`
input, 8,000 Hz and source-order controls `[.5, 0, 1, 0, .5, .5, 0, .5, 0]` (Input through
Output). Flutter, bias processing and head bump are disabled by their source branches, and the
encoder coefficient is zero. On sample 472, `iirDecL = -7.625997560091027e307`, `highPart = 1`
and `compDecL = +Infinity`, producing negative infinite decoder output. Output = 0 multiplies
that by zero, producing NaN; the clipper comparisons do not reject it and its delayed history
emits it on the next sample. The right channel follows the same recurrence. This is a numerical
source counterexample, not a compiled upstream-oracle test result. A stabilizing coefficient
change alters defined DSP, and an adapter-only NaN mute would leave the raw-reference gate
unsatisfied. The amendment now authorizes bounding both coefficients to 1 in the reference and
port, making these history updates convex and leaving their normal-rate values unchanged. Tape
Color returns to slice 1 under the full admission gate; no rate/control domain is narrowed.

**Finite input alone does not prove stability.** "kPlate140" has a separate amplitude problem,
even at 44.1 kHz. In `kPlate140/kPlate140Proc.cpp:78`–97, the SubTight section starts from
`out = avgSamp * .00187`. Three stages per channel each compute `scale = .5 + abs(out*.5)`,
`out = history + sin(history-out)*scale`, then `history = out*scale`. Lines 99–100 clamp only
the temporary output after those history writes. No amplitude bound is enforced before them.

The judging round 3 binary64 trace, independently reproduced as a source-derived recurrence in
this revision, uses fresh constructor state, 44,100 Hz, constant stereo `2000.0f` input and
source-order controls `[1, .5, 1, 0, 1]`. It includes Bezier accumulation and zero-predelay startup
(processing lines 57–74; constructor lines 152 and 161–165). The internal average reaches 2000;
at input sample 49, `subCL` is approximately `9.01785351125649e276`, at sample 51 it becomes
infinity, and at sample 53 `outSampleL` becomes NaN. Sample numbers are one-based. This is a
numerical source counterexample, not a compiled C++ oracle result; math libraries may change
the exact timing. Nonzero input makes denormal seeding irrelevant to this recurrence. The NaN
passes the comparison clamps, contaminates the delay network (lines 103 and 118–127), and is
not removed by the final comparison clamps either (lines 423–426).

`crates/mix/src/strip.rs` silences and fails an insert after nonfinite output; it does not limit
input amplitude or establish a finite raw-reference floor. The amendment instead authorizes
clamping each of these six histories to [-1, 1] in the reference and port. The next update then
cannot inherit an unbounded value; the normal-domain invariant leaves this clamp inactive.
Large Plate returns beside Small Hall in slice 3, with the 2000.0-input regression retained,
both nulls required and the inherited Plate Reverb retired only when its replacement lands.
A milder unspecified hot-input fixture still does not resolve the whole-domain obligation.

**No blanket transparency claims.** The float path ordinarily adds dither even at nominally neutral
settings. The explicit unity bypass in "PurestGain" and RNG-free "BitShiftGain" are exceptions,
not a reason to remove dither elsewhere. Output Dither preserves the source's fractional-bit DC
offset; it must not be described as a newly corrected quantizer or automatically inserted at export.

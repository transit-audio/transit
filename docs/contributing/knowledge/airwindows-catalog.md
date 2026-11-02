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
  Near-zero input is commonly replaced with `fpd * 1.18e-17` below `1.18e-23`; float output gets
  exponent-scaled dither using `frexpf`, `5.5e-36l` and `pow(2, expon+62)`. Seeding must occur
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

These are the 13 selected inserts, in slice order. Each row gives the state that the port's reset,
floor and scheduling fingerprint must cover, in addition to its parameters and rate.

| Source / version | Count and exact parameter names | Sample-rate dependence | State and seed notes | Gap and selection evidence |
|---|---|---|---|---|
| "Chorus" / U | 3: Speed, Range, Dry/Wet | **Yes:** phase increment `A^4 * .001 * r`; range is a fixed sample-buffer fraction `B^4 * loopLimit * .499`. Speed in Hz therefore also changes with rate; treble processing is sample-based. | F; two float delay arrays of 16386, shared sweep/count, separate L/R treble histories and flip flag. Same modulation on both sides; no invented stereo spread. | Single moving-tap modulation; notes call it mono chorus that can serve as vibrato. |
| "ChorusEnsemble" / U | 3: Speed, Range, Dry/Wet | **Yes:** phase increment `A^3 * .001 * r`; range `B^3 * loopLimit * .12` in samples; four related tap offsets. | F; two 16386-float buffers, sweep/count, treble histories and flip flag. | Four-tap ensemble for pads, not a second single-tap flavor. The author's section explicitly distinguishes the bank of taps. |
| "ToTape9" / 9 | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | **Yes:** filter, bias, head-bump and flutter formulas use `r`; depth capped at 498 samples; clip spacing `clamp(floor(r),1,16)`, slew span `clamp(floor(2*r),2,32)`. Head frequency `(H*H*175+25)/rate`. | F also drives flutter and clipping noise; encoder/decoder envelopes, IIR/biquads, two 1002-double flutter buffers, phases/targets, bias stages, hysteresis, averaging histories and clip/lookahead state. | Latest tape revision; notes describe the replaced core, brightness compression and new output clipper. Internal processing is retained, not factored into shared algorithms that change its rounding. |
| "Density3" / 3 | 4: Density, Highpas, Output, Dry/Wet | **Yes:** high-pass amount `B^3/r`; zero high-pass clears its histories at block entry. | F; L/R IIR memories. Internal `density=A*5`, display `A*5-1`; preserve both. | General saturation, including inverse saturation; author calls this the refinement of the earlier line. |
| "Pressure6" / 6 | 2: Compres, Ratio | **Yes:** detector speed target `((A*92)+92)*r`; threshold `1-(.9*A)^3`. | F; shared stereo-linked `muComp` and `muSpd`, initialized 1 and 100. Ratio maps to `wet=.9*B`, used inside a nonlinear blend, not an ordinary ratio in x:1. | Character bus compression; latest Pressure line and explicitly aimed at that job in its notes. |
| "DeBess" / U | 5: Intense, Sharp, Depth, Filter, Sense | **Yes:** intensity `A^5*8192/r`; sharpness `max(2,40*B)` samples, speed `.1/sharpness`; Filter itself is not rate-scaled. | F; 41-element sample/comparison arrays per side, alternating ratios/IIRs and flip. No legacy static noise counters. Sense above `.49999` outputs input minus processed. | Author explicitly calls it improved "DeEss" with a longer adjustable comparison window. High-rate, bandwidth-limited material can trigger it less; document this limitation. |
| "Console9Channel" / 9 | 2: Pan, Fader | **No** `getSampleRate`; **block-dependent** pan/fader interpolation using remaining frames / original call length, including first call. | F; `panA/B`, `inTrimA/B`. Encode curves use `log1p`/`expm1`; constructor ramp values must survive. | Latest numbered standalone summing encoder. Notes distinguish the summing core from full processing strips. |
| "Console9Buss" / 9 | 2: Pan, Fader | **No** rate query; same block-based control ramps. | F; pan/trim endpoint memories; complementary decode curve. | Must pair with the selected channel algorithm after the sum; no cross-generation pair. |
| "ADClip9" / 9 | 5: Boost, Match, Noise, Ceiling, Mode | **Yes:** sample spacing `clamp(floor(r),1,16)`; delayed clip history and slew tests follow that spacing. | F also supplies clipping noise; last output and last dry sample, 18-element intermediate and 34-element slew arrays per side, clip flags. **Constructor omits `lastDryL/R` initialization; deterministic patch required.** | Latest clipper revision. Source has three modes, not the predecessor's eight. Noise/Ceiling are nonlinear thresholds, not dB labels. |
| "TPDFDither" / U, Redux | 2: Quant, DeRez | **No:** fixed word-length scaling, independent of sample rate. | T; only parameters and RNG words persist. Quant selects scale 32768 or 8388608; DeRez multiplies by `(1-B)^6`, scale floor `.0001`, output divisor floor 8. | One general output dither. Notes recommend it when no special shaping is needed and acknowledge its half-LSB offset; preserve the actual floor/addition sequence. |
| "Slew4" / 4 | 1: Compres | **Yes:** `compresity=(.95*A)^2*8*r`; filter banks and lookahead remain in samples. | F; L/R 24-by-12 averaging storage, slew sustain, second/third samples, shared count; explicit two-sample lookahead. | Latest brightness compression without saturation; distinct standalone use even though tape includes related processing. |
| "kGuitarHall2" / 2 | 6: Regen, Derez, Filter, EarlyRF, Positin, Dry/Wet | **Yes:** revised Bezier undersampling: `B*2` folds about center, divides by `r`, clamps, then left half locks to reciprocal integer subdivisions while right half stays continuous with rate correction. | F; 3-by-3 early and 6-by-6 main delay networks, counts, feedback, Pear-filter and Bezier histories. Immutable `early[36]`; `int(E*27)` chooses 28 starting positions over nine entries. | A small hall with source positioning; author's notes describe its hall/arena goal and the new Position control. |
| "kPlate140" / U, model 140 | 5: Input, Regen, DeRez, Predlay, Wetness | **Yes:** `derez=C/r`, clamped `.0005..1`, then reciprocal-integer locked; predelay `int(16000*D*derez)` at the internal update rate. Filter/size behavior follows that clock. | F; 5-by-5 matrix delays/counts, low-frequency damping state, gain stages, Pear filters, vibrato, predelay/subdelay and Bezier histories. | Author's next-generation large plate. At Wetness .5 both dry and wet gains are 1; below .5 wet gain is squared. Not a constant-power crossfade. |

## Deferred candidates and predecessors

Deferred means no additional first-set insert. A predecessor is kept in this survey for the choice's
audit trail, not queued for a duplicate port. The original issue's example versions were investigated
even where the selection moved to a later version. F/N meanings are defined above.

| Source / version | Count and exact parameter names | Sample-rate dependence | State and seed notes | Gap / why deferred |
|---|---|---|---|---|
| "Console7Channel" / 7 | 1: Fader | **Yes:** 20 kHz biquad normalized by rate; gain chase capped by `sampleFrames`. | F; gain chase/speed and one stereo biquad. Block-length dependent. | Earlier console encoder; selected 9 gives the latest standalone pair. |
| "Console7Buss" / 7 | 1: Master | **Yes:** two 20 kHz biquads; chase capped by call length. | F; gain chase/speed, two stereo biquads. | Earlier matching decoder, not an alternative for the selected encoder. |
| "ToTape8" / 8 | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | **Yes:** `r` scales flutter, bias and filters; clip spacing capped at 16; depth capped at 498. | F; 1002-sample flutter buffers, companding/bias/head-bump filter state, 16-element clip intermediates. Boundary access needs audit if ever ported. | Tape predecessor; "ToTape9" notes explicitly describe the improved replacement core. |
| "Density2" / 2 | 4: Density, Highpass, Output, Dry/Wet | **Yes:** high-pass `B^3/r`; reconstructed halfway samples and alternating correction histories are sample-based. | F; three previous samples, correction accumulators and two IIR histories per side. | Saturation predecessor; "Density3" is the author's revision, not an arbitrary same-sounding substitute. |
| "PurestDrive" / U | 1: Drive | **No:** sine saturation blended using current/previous sample behavior. | F; previous sample per side. Header's unused B–E do not make this a five-parameter effect. | Simpler saturation; overlaps the selected saturation job. |
| "DeEss" / U | 3: Intense, Max DS, Freq | **Yes:** intensity and IIR amount divide by `r`. | F plus **function-static** `noisesourceL=0`, `noisesourceR=850010` in each precision path. Residue computation squares signed ints beyond their range; static counters leak across instances. Detector histories, ratios, IIRs and flip are instance state. | De-esser predecessor; author recommends "DeBess". Any later port must resolve static ownership and overflow explicitly rather than seed only `fpd`. |
| "StereoChorus" / U | 2: Speed, Depth | **Yes:** processing every `cycle` frames with reconstruction between; different control laws from "Chorus". | F; stereo delay/phase and air-compensation histories, cycle and output references. | Stereo widening variant; first set chooses a simpler independent speed/range/mix design, not a claim that this variant is obsolete. |
| "StereoEnsemble" / U | 2: Depth, FXlevel | **Yes:** `cycle` division and reconstructed output; delay factor `.66+A/3` changes fixed sample lengths. | F; many paired delay banks, maxima/counts, feedback and reconstruction state. | Another ensemble topology; avoid a second ensemble in slice 1. |
| "Vibrato" / U | 5: Speed, Depth, FMSpeed, FMDepth, Inv/Wet | **No:** increments `(0.1+A)^6` and `(0.1+C)^6` per sample; depths derive from their square roots; fixed sample buffers. | F; two 16386-double buffers, two sweeps, shared position, treble histories and flip. Wet spans -1..1. | Dedicated FM vibrato later; the first chorus already has a wet pitch-modulation use. |
| "Flutter2" / 2 | 3: Flutter, Speed, Dry/Wet | **Yes:** depth `A^4*r*90`, capped 498; rate `.02*B^3/r`. | F also updates random flutter targets; two 1002-double buffers, L/R phases/targets and shared count. | Standalone tape instability; selected tape already includes flutter. |
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
| "kPlateA" / U, model A | 5: InputPad, Damping, Low Cut, Predelay, Wetness | **Yes:** `cycle` division; `downRate=rate/cycle` sets filters and `int(downRate*D^2*.5)` predelay. Cycle cap makes very high rates require a predelay bounds audit. | F; early reflections, 5-by-5 delays/counters, feedback, gain/IIR/biquad and reconstruction histories. | Earlier plate candidate. Next-generation large plate chosen; lettered variants would duplicate the first job. |
| "kPlate240" / U, model 240 | 5: Input, Regen, DeRez, Predlay, Wetness | **Yes:** rate-divided reciprocal-integer DeRez clock and scaled predelay, like model 140, with different feedback/blur coefficients. | F; its own 5-by-5 delays and feedback, damping/gain, Pear filters, modulation, predelay/subdelay and Bezier state. | Author describes a smaller, cloudier plate. Different model, not newer because 240 exceeds 140; defer a second plate color. |

## What the survey changes about the implementation

**Seeded does not mean zero residual is optional.** Test fixtures set both seed words and all
initialization state before processing. Self-floors run in one process and a child process;
instance interleaving tests catch hidden shared state. Nulls compare all f32 output bits and final
seed words against the same compiler's original float path. The deterministic patch set is part
of the reference, recorded in `VENDOR.md`; no full null result exists yet for these proposed ports.

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
Per-sample processing does not authorize block restructuring. Adapter nulls replay the same
parameter timing and call sizes as the reference; scheduling equivalence uses the same schedule.

**Physical units must be earned.** "Pressure6" Ratio is a nonlinear blend amount, not a measured
compression ratio. "DeBess" Sense's display labels disagree with the intuitive meaning of its
delta-monitoring branch; transit labels the actual output. Hall Position really selects 28
positions, while De-esser Sharp is not purely stepped because fractional sharpness also affects
speed. Plate Wetness has unity dry and wet at the midpoint. Predelay's exact seconds depend on
integer truncation and the internal clock. Percentage travel is an honest display for uncalibrated
controls; the ADR's specified Hz/dB/list mappings are exceptions backed by source formulas.

**High rates are in scope now.** `crates/time/src/lib.rs` admits up to 768 kHz, with 352.8/384 kHz
already in `SampleRate::COMMON`; the export command also admits down to 1 kHz. The selected
high-rate floors/nulls include 705.6/768 kHz and the low-rate/boundary probes specified in the ADR.
Array bounds, integer-divider transitions, long delay warmup and sample-based tone changes need
evidence. Neither running only 48 kHz nor silently rate-converting the source satisfies the decision.

**No blanket transparency claims.** The float path ordinarily adds dither even at nominally neutral
settings. The explicit unity bypass in "PurestGain" and RNG-free "BitShiftGain" are exceptions,
not a reason to remove dither elsewhere. Output Dither preserves the source's fractional-bit DC
offset; it must not be described as a newly corrected quantizer or automatically inserted at export.

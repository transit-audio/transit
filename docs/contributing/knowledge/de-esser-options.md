# Stock de-esser options after DeBess

Research for #306, against transit `0c8af743`. **Recommend a small native de-esser**, reusing
the Filter's band-pass topology and the Compressor's gain law. Corrected DeBess is the second
choice if preserving an established algorithm matters more than conventional controls.
Neither is implemented or approved by this note. “Admissible” below means a viable design to
take to a decision and implementation gates, not a completed shipping certificate.

## Evidence and ranking

`AW/` below means `plugins/LinuxVST/src/` in the supplied Airwindows checkout at
`7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a`, locally
`E:/transit-targets/codex-deess/airwindows/`. Source citations use that pin's file:line.
Both complete DeBess and DeEss directories were read, including both precision paths, headers,
constructors and parameter handling. The production contract remains the **float** path.
External repository links identify the inspected files; license findings are names and licenses,
not an inference from a repository search badge.

The domain is all normalized controls, arbitrary valid automation, every integer session rate
8–768 kHz and any finite f32 input; 1 kHz remains a robustness probe. Normal-rate comparisons
use 44.1/48/88.2/96/176.4/192 kHz. These are separate obligations: a normal null does not prove
finite output at extreme input. Authority: `docs/adr/0042-stock-effects-airwindows-ports.md:181`,
`:204`, `:264`, `:506`. No cargo, compiled oracle, audio render or listening test was run for
this documentation task. The numerical trace below is a binary64 recurrence calculation.

| Rank | Option | Verdict | Evidence | Cost before shipping |
|---|---|---|---|---|
| 1 | Native band detector plus bounded gain reduction | **Admissible as a proposed design**, subject to proof/tests and listening | Filter `svf.rs:63`, `mods.rs:115`; `dynamics.rs:13`; design below | Small new ADR; extract reusable primitives, new insert/id, help, presets, deterministic and realtime gates. No C++ fidelity claim. |
| 2 | Reference-and-deviate DeBess | **Admissible with a recorded deviation**, conditional on the new amendment and full-domain guards | `AW/DeBess/DeBessProc.cpp:65–99`; trace and exact change set below | ADR 0042 amendment explicitly allowing normal-domain differences, patch manifest, two references, three-compiler nulls, listening. |
| 3 | Another permissive source | **Not admissible as-is** among the inspected implementations; MIT licenses provide leads, not a ready port | mda and Curtis source review below | Separate source/decision, provenance audit, repairs, oracle and interface work; more work than their short loops suggest. |
| 4 | Another Airwindows candidate | **Not admissible as an unchanged stock de-esser** | DeEss failures; Dynamics3/Bandpass/Slew4 table below | DeEss needs more repairs; assembling a detector and dynamics core is a new algorithm, with its own decision and tests. |

The reason for rank 1 is control of the user contract: frequency, threshold, maximum reduction
and listen can mean exactly what they say. It also avoids paying for two reference builds while
explicitly declining to match one of them on normal inputs. It does cost sound-design validation;
this ranking does not claim the unbuilt design sounds better than Johnson's detector.

## 1. Reference-and-deviate on DeBess

The five controls are Intense, Sharp, Depth, Filter, Sense, defaults `[0,.5,.5,.5,0]`
(`AW/DeBess/DeBess.cpp:15–19`, `:115–123`). Let `r=fs/44100`,
`I=A^5*8192/r`, `S=max(2,40*B)`, `v=.1/S`, `d=1/(C+.0001)` and `a=D`.
The slew-of-slews detector, multiplicative rejection window and ceiling at `I` are at
`DeBessProc.cpp:21–63`; alternating IIR/ratio histories are at `:65–85`.
For each channel the output, when engaged, is `h+(x-h)/q`, where
`h=(1-a)*h+a*x`. This attenuates the residual above its low-pass history; it is not a
conventional frequency-calibrated band-pass detector or a threshold/ratio compressor.

### Minimal division fix, and what “bound the ratios” means

Change the B-phase predicates at `DeBessProc.cpp:82–83` to test `ratioBL/BR`, the values
actually used as divisors. Mirror at `:184–185` if maintaining the double source too.
Keep the original multiply/add order and original upper cap at each ratio write
(`:68–71`, `:78–81`). Specify stored ratios as **[0,d]**, with an explicit lower comparison
clamp to 0 if desired; the detector is nonnegative so that lower clamp is inactive on the
admitted domain. An untouched phase's ratio remains within [0,10000] even if Depth changes.
The **effective divisor** is bounded to **[1,10000]**: use the original division only when
that same phase's ratio is greater than 1, otherwise leave `x` alone. Zero/underflowed ratios
are never divided by. This closes the fault without changing detector memory or the depth law.

Do **not** silently floor the stored histories to 1. A patch using
`q=clamp(q,1,max(1,d))` is a different, broader option: low Intense values make sense less
than 1, so the floor changes recovery history in both phases, including on constant input;
later increases in Intense can then change A-phase output too. Also `d<1` near C=1, so
`[1,d]` is not even an ordered clamp interval. Neither stronger history floor is needed to
make the corrected division safe. Record the distinction in the patch and its review.

On the stated domain `I<=45158.4`, `v` is .0025–.05, and `d` is approximately
.99990001–10000. The ratio update is a nonnegative convex combination followed by the upper
cap. Sample differences and detector products of finite f32 input fit binary64: even the
initial quadratic slew terms, the factor `S²<=1600` and `I²` leave enormous binary64
headroom; the window only multiplies by factors less than 1. Arrays are bounded by
integer indices 0–40, including fractional Sharp truncation. All are initialized
(`DeBess.h:59–74`, `DeBess.cpp:20–33`). There is no process allocation or mutable static;
constructor `rand()` must become the recorded per-instance deterministic seeds. Dither and
denormal substitution stay (`DeBessProc.cpp:33–34`, `:93–99`). Rate compensation applies to
Intense, not the Filter coefficient or the detector window's length in samples.

### Round 5 counterexample re-derived

Fresh state has all ratios 1 and `flip=false`. At 44.1 kHz, one frame per call, B=C=D=E=0,
odd frames `(A,x)=(0,+1)` and even frames `(1,-1)` on both channels. Thus S=2, v=.05,
d=10000, h=0. At S=2 the unused `m[2]` remains zero; the detector ceiling gives sense 0
on odd frames and 8192 on even frames (`DeBessProc.cpp:44–63`). After k updates of each phase:

```text
qB(k) = .95^k
qA(k) = 8192 - 8191*.95^k
original odd output after frame 1 = 1/qB(k)
corrected odd output = 1                 (qB never exceeds 1)
even output, both versions = -1/qA(k)
```

| One-based frame | qA after update | qB after update | Original output before dither | Corrected output before dither |
|---|---:|---:|---:|---:|
| 1 | 1 | .95 | 1 | 1 |
| 2 | 410.55 | .95 | -.002435756911460236 | same |
| 3 | 410.55 | .9025 | 1.10803324099723 | 1 |
| 3459 | approximately 8192 | 2.8962573551544587e-39 | 3.452731844496842e38 | 1 |

Repeated binary64 updates reproduce the ADR's frame-3459 value, above f32 maximum
3.4028234663852886e38. No denormal input substitution occurs; output dither never feeds the
detector. This is an analytical/source recurrence, not a C++ oracle measurement. The corrected
output remains bounded for arbitrarily many repetitions, even after qB underflows to zero.
Retain both channels, reversed signs, swapped phases and the unpatched failing fixture.
Authority: `docs/adr/0042-stock-effects-airwindows-ports.md:105–121`, `:645–655`.

### The exact scope of changed audio

For the branch-only fix, all histories and RNG words remain identical. On a B-phase sample,
write `PA=(qA>1)` and `PB=(qB>1)` using the updated B ratio and the current A ratio.
If PA=PB, both versions take the same expression. If they differ, one emits `x` and the
other `hB+(x-hB)/qB`. Results can still coincide when the residual is zero or qB=1;
f32 rounding can hide small differences. Otherwise the pre-dither outputs differ. The
original can also divide by zero; the corrected branch cannot. A-phase output is unchanged.

This set includes **ordinary transients and startup with fixed controls**, not just the
round-5 alternating automation. For example, from reset with A=1, B=C=D=0 and first input
+1, qB becomes 410.55 while qA is 1: the original skips reduction and the correction applies
it on the very first frame. Calling the change “only the alternating-detector path” is true
only if it means the B processing phase, not a special class of alternating audio.

The division fix alone is **not** whole-domain admission. In Removed monitoring, `x-y` can
approach 2*f32::MAX even when the processed path is a convex blend: first charge an IIR
with negative maximum input using D=1, then set D=0 and feed positive maximum while its
ratio exceeds 1. Normal dither can also push maximum-f32 passthrough past the finite
rounding boundary (`DeBessProc.cpp:87–103`). A complete proposal must additionally bound
the output temporary to ±f32::MAX **before** the float conversion for `frexpf`, and again
after dither before the final store, preserving RNG advances. These extreme-only output
bounds require explicit authorization too: ADR 0042 currently limits patch kinds and does
not grant an unrestricted final-output limiter (`:172–179`, `:264–275`). Audit floating-point
IIR rounding over arbitrary histories as part of that proof; mathematical convexity alone
is not a bit-level proof. Never replace the defect with adapter NaN muting.

**Character verdict:** the narrow division fix keeps Johnson's detector, window, rate law,
IIR split, speed and Depth law. It is defensibly *DeBess with a documented correction*, not
sample-identical DeBess throughout normal use. Perceptual equivalence is unmeasured. Compare
original versus corrected on bounded vocal passages where the original stays finite, and
compare corrected versus native on the fixture below and consented vocal recordings. Include
low Intense, sharp transients and listen mode; level-match and judge lisping, dulling and pumping.

**Amendment contract:** propose a unified diff under `crates/airwindows-oracle/patches/`, applied
to build copies of byte-identical vendor files. Record the pin, both branch changes, exact
ratio/divisor bounds, output guards and placement in `VENDOR.md`, the catalog patch table and
the Rust implementation. Keep both seeded reference variants. Full-domain port-to-patched
nulls remain mandatory; original normal-domain N1–N3 nulls become explicitly restricted to
whole renders where PA=PB at every B frame and all added bounds are inactive. Instrument
that restriction and count excluded cases; never silently filter a failing fixture. Run the
excluded normal cases against the patched reference and publish their differences separately.
This is a **new exception**, not compliance with the existing all-normal-input promise
(`docs/adr/0042-stock-effects-airwindows-ports.md:163–177`, `:237–262`).

Proposed help sentence: “Based on Chris Johnson's DeBess (Airwindows, MIT); transit corrects
the alternating-phase divisor test and bounds extreme output, so some normal transients and
automation sound different from the original.” Do not reuse the amendment's unqualified
normal-use identity sentence (`:278–281`). Preserve the required upstream notices; this note
does not add or implement the amendment.

## 2. Other Airwindows candidates

The catalog search finds DeEss and the already-selected Slew4 brightness processor
(`airwindows-catalog.md:88`, `:100`, `:106`); it has no Dynamics3/Bandpass verification rows.
The supplemental survey therefore follows the actual source directories and
`Airwindopedia.txt:18`, `:28`, `:2538`, `:6874`, `:6936–6942`, `:7012–7016`.
Bandpass effects are colored filters, not detectors connected to gain computers.
All are MIT (`airwindows/LICENSE:1–21`). F below means constructor-seeded instance xorshift
words, denormal injection and float dither, with no process allocation; it still needs
deterministic seeding and oracle floors. No mutable processing statics were found in these
supplemental effects. DeEss is the exception explicitly identified below.

| Candidate / exact parameters (normalized 0–1) | Seeds and state | Rate/block behavior | Finite-output finding and verdict |
|---|---|---|---|
| **DeEss**: Intense, Max DS, Freq; defaults `[0,.5,.5]` (`DeEss.cpp:15`, `:110`) | F **plus function-static** noise counters 0/850010 in each precision path; signed-int residue squares overflow; ratio/IIR histories are instance members (`DeEssProc.cpp:34–67`, `:194–227`; `.h:57–107`) | Intensity `A^5*8192/r`; IIR `C²/r`; detector attack/recovery depend on signal. Coefficients at call entry, no block ramp (`Proc.cpp:17–27`, `:74–107`) | **Not admissible unchanged.** Independent state/UB repairs and unstable IIR described below; fixing DeBess is the smaller repair. |
| **Dynamics3**: Thresh, Attack, Release, Inv/Wet; defaults `[1,.382,.618,1]` (`Dynamics3.cpp:15–24`, `:104–107`) | Initialized seven-value Bezier state and F, but dither is `3.553e-44l` with a conditional extra right RNG advance (`Proc.cpp:113–121`) | Rate-dependent, integer-stepped Bezier update and rate-scaled release; coefficients at call entry, state advances per sample (`Proc.cpp:22–33`) | Bounded detector/curve is a viable **wideband dynamics building block**, not a complete de-esser: detector sees both full-band inputs (`:54`). No frequency/listen/range control. **Not admissible as an unchanged de-esser**; joining it to a filter is new behavior. |
| **XBandpass**: Gain, Freq, Nuke, Dry/Wet; defaults `[.5,.5,0,1]` (`XBandpass.cpp:15–21`, `:101–104`) | F; five initialized 15-double biquad arrays, instance-only | `f=max(.00005,B^4*20000/fs)`; gain `(A+.5)^4`; fixed sample filter stages, no control ramp (`Proc.cpp:17–49`) | Clamped ±π/2 temporary and sine feedback bound the wet stages, even when f exceeds Nyquist; do not infer an unbounded recurrence merely from negative tangent (`:68–81`). At Dry/Wet=0, maximum-f32 input plus ordinary F dither can overflow (`:160–176`). **Not admissible unchanged** under the whole-input contract, and no dynamics detector. |
| **YBandpass**: Gain, Freq, Reson8, ResEdge, Output, Dry/Wet; defaults `[.1,.5,.1,.1,1,1]` (`YBandpass.cpp:15–29`, `:117–122`) | F; initialized band-pass/fixed-filter histories and gain/power/coefficient endpoints | `f=max(15,B³*20000)/fs`, Q=`15*C²+.5571`; 20 kHz fixed low-passes. Call-length interpolation of gain/power/main coefficients (`Proc.cpp:17–85`) | At 32 kHz the unbounded fixed-filter recurrence has negative K and a pole outside the unit circle; clipping later does not bound its state (`:55–64`, `:90–97`, `:131–138`). **Not admissible unchanged**. |
| **YNotBandpass**: same six parameters/defaults (`YNotBandpass.cpp:15–26`, `:114–119`) | F; initialized instance band-pass and two fixed-filter arrays; no interpolation endpoints | Same frequency/Q and 20 kHz filters; deliberately removes Y's call ramps (`Proc.cpp:21–54`) | Same 32 kHz fixed-filter failure (`:68–75`, `:109–116`). **Not admissible unchanged**; removing smoothing is not a stability fix. |
| **ZBandpass**: Input, Freq, Output, Poles; defaults `[.1,.5,1,.5]` (`ZBandpass.cpp:15–25`, `:105–108`) | F; six initialized filter arrays plus IIR histories | `f=B^4*14300/fs+.00079`, Q=.314; 15.5 kHz fixed filters, IIR `.00069/r`; no call ramp (`Proc.cpp:17–52`) | Fixed filters at 22.05 kHz have negative K and unstable unbounded histories (`:39–47`, `:157–172`), despite surrounding clipping. **Not admissible unchanged**. |
| **ZBandpass2**: same four parameters/defaults (`ZBandpass2.cpp:15–29`, `:109–112`) | F; initialized main/fixed-filter arrays, IIRs, gain/wet/coefficient endpoints | Same frequencies, call-length coefficient/gain/wet interpolation; input gain fourth power (`Proc.cpp:17–84`) | Main-stage output clamps (`:106–114`) do not bound the 15.5 kHz fixed-filter state (`:173–194`); same low-rate failure. **Not admissible unchanged**. |
| **Slew4**: Compres, default .5 (`Slew4.cpp:15–31`, `:99`) | F; averaging banks, two-sample history, slew sustain, shared bounded count | `(.95*A)²*8*r`; averaging/lookahead in samples; right-derived fractional depth blends both channels (`Proc.cpp:21`, `:33–37`, `:341–354`) | Sustain is a leaky sum; averaging is bounded in binary64. At A=0, delayed maximum-f32 dry samples still meet ordinary F dither. Full conversion proof/guard remains necessary. Already selected as Treble Tamer, **not admissible as a substitute de-esser** with frequency/threshold/range/listen. |

For the low-pass failures, `K=tan(pi*f)` and
`b2=(1-K/Q+K²)/(1+K/Q+K²)`. At the listed fixed frequencies/rates, K<0 and the
denominator is positive, so b2>1. The product of the two poles exceeds 1; nonzero excitation
can grow without bound in the stored recurrence. The following saturator cannot rescue
earlier state writes. These are source-derived failures, not compiled run timestamps.

DeEss has an additional concrete failure even after defining the noise arithmetic. Its IIR
coefficient is `a=(1-abs(x))*C²/r` (`DeEssProc.cpp:87–107`, right `:126–146`).
At 8 kHz, C=1 and x=.1, a=4.96125, so the history multiplier is -3.96125.
At 44.1 kHz and x=2, a=-1, multiplier 2. Either admitted constant input excites an unstable
history. Choose A=1 and B=0 so initial onset raises the ratio above 1 and the divergent
history contributes to output. Even if the ratio later returns to 1, finite persistent state
is independently required. Clamp a to [0,1] would be a behavior change, separate from moving
the noise counters into instances and defining their overflowing integer arithmetic. At A=0,
`sense=0` also makes `recovery=1+.01/sense` infinite; that transient division needs an explicit
definition, not an assumption that a deterministic `fpd` seed solves the reference.

Dynamics3 is not rejected for an invented low-rate runaway: its nonnegative controls have
finite update steps throughout the admitted rates; `bez_comp=1-min(curve,.9999)` remains
approximately [.0001,1], the detector input is capped, and polynomial input is clamped to
±2.305929007734908 (`Dynamics3Proc.cpp:22–30`, `:54–109`). Those invariants support a finite
core; a full audit still must bound accumulator resets under parameter jumps and the final
dry blend/conversion. Its much smaller dither must not be confused with ordinary F dither.
**No surveyed alternative is a verified, unchanged, complete stock de-esser.** This is not
a claim that every generic dynamics/filter primitive necessarily needs a behavioral patch.

## 3. Other permissive sources

Network inspection found the following licenses. No BSD-, Apache- or zlib-licensed complete
de-esser was verified in this bounded survey; that is not proof that none exists. In
particular, an Apache-licensed host or MIT wrapper does not establish its bundled DSP's license.
No license text is reproduced here.

| Name | License evidence | Algorithm and oracle fit | Decision |
|---|---|---|---|
| **mda DeEsser**, Steinberg public SDK sample | **MIT**: [repository LICENSE.txt](https://github.com/steinbergmedia/vst3_public_sdk/blob/586dc5e6c8012c3e4b01c79389375cbe96bdb1da/LICENSE.txt); the processor also carries Paul Kellett's MIT notice at lines 9–21 | Split high-frequency limiter, three controls: Thresh/Freq/HF Drive. Small deterministic per-sample loop, instance state, no process allocation or RNG. But fixed sample coefficients, block-end denormal cleanup, mono fold-down and finite-input failure below. | Permissive lead; **not admissible as-is**. |
| **Jacob Curtis De-esser**, Juce-Plugins | **MIT**: [LICENSE](https://github.com/jcurtis4207/Juce-Plugins/blob/c381f1ad2221bf85fff91b19a23fe844ef37202c/LICENSE) | Linkwitz–Riley split-band or wideband, filtered peak envelope, fixed 4:1 ratio, frequency/threshold/attack/release/listen. Rate-derived coefficients; buffer-based deterministic processing. Variable-block/mono assumptions need repair; do not import JUCE or cited third-party code under the root license without a dependency/provenance audit. | Potential extraction candidate; **not admissible as-is**. |
| **macos-system-deesser** | Root [LICENSE](https://github.com/cristianrgreco/macos-system-deesser/blob/2d8335c170d6058cb3ff72079c0dfac0d61235c6/LICENSE) says **MIT**, but [DeEsserDSP.cpp:5–6](https://github.com/cristianrgreco/macos-system-deesser/blob/2d8335c170d6058cb3ff72079c0dfac0d61235c6/DeEsser/Realtime/DeEsserDSP.cpp#L5) explicitly identifies a Calf port | High-pass/peaking sidechain, wideband or split-band. Per-sample C++ with fixed state; rate-derived coefficients; output sanitizing does not prove finite state. | **Not admissible** without resolving the Calf provenance; root MIT label is insufficient. |
| **Calf Studio Gear** | **LGPL-2.1**, [COPYING](https://github.com/calf-studio-gear/calf/blob/master/COPYING) | Wide/split-band de-esser; no further port audit warranted under this task's license constraint. | **Not admissible**. |
| **LSP plug-ins** | **LGPL-3.0**, [COPYING.LESSER](https://github.com/lsp-plugins/lsp-plugins/blob/master/COPYING.LESSER) | Dynamics/EQ components, not a newly verified permissive de-esser core. | **Not admissible**. |
| **x42 fil4.lv2** | **GPL-2.0**, [COPYING](https://github.com/x42/fil4.lv2/blob/master/COPYING) | EQ example, not evidence of a permissive dynamic-EQ de-esser. | **Not admissible**. |

The mda source at the linked pin is especially easy to wrap as an oracle, but its exact
behavior is a poor default: `tmp=.5f*(a+b)` can overflow for two finite maximum samples;
both outputs receive the same `g`, destroying stereo differences. Detection compares signed
`tmp` with the envelope rather than rectifying it. Attack .01, release .992 and filter
coefficient `.05+.94*Freq²` do not use sample rate. Cleanup at the end of each call makes
block schedules part of the reference. See
[mdaDeEsserProcessor.cpp:90–135](https://github.com/steinbergmedia/vst3_public_sdk/blob/586dc5e6c8012c3e4b01c79389375cbe96bdb1da/samples/vst/mda-vst3/source/mdaDeEsserProcessor.cpp#L90).
Fixing these while adding Range/Listen would be substantial redesign, not a faithful port.

Curtis allocates four buffers in `prepare`, then loops to the **prepared maximum block size**,
not the current input length; `makeCopyOf` can resize buffers, so “no allocation in process”
requires enforcement rather than inference. It assumes two channels while its wrapper accepts
mono. The loop has no RNG or mutable static, but reset, channel count, allocation and short-block
floors need resolution before an oracle is useful. Its source also names earlier compressor
and envelope sources, which must be audited. See
[De-esser.h:5–9,43–67,97–181](https://github.com/jcurtis4207/Juce-Plugins/blob/c381f1ad2221bf85fff91b19a23fe844ef37202c/De-esser/Source/De-esser.h#L43)
and [PluginProcessor.cpp:111–125](https://github.com/jcurtis4207/Juce-Plugins/blob/c381f1ad2221bf85fff91b19a23fe844ef37202c/De-esser/Source/PluginProcessor.cpp#L111).

Other checked leads: [theaudiobible/de-ess](https://github.com/theaudiobible/de-ess) has no LICENSE file in its root listing, so its
license is **unverified** here; [tdon01/Finesse_De-esser](https://github.com/tdon01/Finesse_De-esser) exposes README/DISCLAIMER rather than
an inspectable DSP source/license. Neither is promoted on a “free plug-in” description.
No external candidate's three-platform floor or admitted-domain proof has been run.

## 4. Native design sketch

Use a new Dynamics insert, tentatively **De-esser**, with no revived retired id. Extract the
Filter's unity-at-center TPT band-pass topology (`crates/dsp/src/plugins/filter/svf.rs:63–105`)
and the Compressor's hard-knee gain computer (`crates/dsp/src/plugins/dynamics.rs:11–30`).
The existing Filter follower (`filter/mods.rs:115–139`) caps input at 4 and returns at most 1;
reuse its attack/release structure, not those readout caps. Filter's amplifier is a note ADSR
and output gain (`filter.rs:478–495`), **not** an existing compressor gain computer.
ADR 0038's note plumbing is unnecessary; the new effect detects audio and supports offline
processing. Its stored-parameter snap convention remains relevant
(`docs/adr/0038-inserts-read-the-tracks-note-stream.md:7–16`, `:26–40`).

| Parameter | Proposed range / default | Meaning |
|---|---|---|
| Frequency | 1–16 kHz / 7 kHz; effective maximum `min(16 kHz,.45*fs)` | Detector/band center; display the actual rate-limited value. At low rates a 6–8 kHz band is unavailable. |
| Width (Q) | .7071–8 / 3.5 | Dimensionless Q; larger is narrower. Approximate bandwidth `fc/Q` away from Nyquist, not a falsely exact Hz bandwidth. |
| Threshold | -60–0 dBFS / -30 dBFS | Absolute peak-envelope threshold in the selected band. |
| Max Reduction | 0–24 dB / 6 dB | Positive attenuation ceiling R; no makeup gain. |
| Ratio | 1:1–20:1 / 4:1 | Above-threshold slope; advanced control. |
| Attack / Release | .1–20 ms / 1 ms; 5–250 ms / 60 ms | Envelope time constants, defined in samples below. |
| Mode | Band / Wide; default Band | Reduce the selected band or the entire input. |
| Listen | Off / Detector / Removed; default Off | Audition the selected input band or actual removed signal; label them separately. |

For each channel compute band sample `b` with `k=1/Q`, `g0=tan(pi*fc/fs)`,
`a1=1/(1+g0*(g0+k))`, `a2=g0*a1`, `a3=g0*a2`:

```text
v3 = x - ic2
v1 = a1*ic1 + a2*v3
v2 = ic2 + a2*ic1 + a3*v3
ic1 = 2*v1 - ic1; ic2 = 2*v2 - ic2; b = k*v1
p[n] = max_channels(abs(b[n]))
c = exp(-1/N), N = time_ms * fs / 1000
e[n] = p[n] + c*(e[n-1]-p[n])       (attack if p>e, otherwise release)
L = 20*log10(max(e, 1e-12))
GR = min(R, max(0, (1-1/ratio)*(L-Threshold)))
gain = 10^(-GR/20)
Wide: y = gain*x
Band: y = x + (gain-1)*b
Detector listen: b; Removed listen: x-y
```

Stereo uses independent filters and one shared envelope/gain. The Band equation is a parallel
dynamic band cut, not an assertion that two independent crossover filters sum without phase
change. At gain=1 explicitly return the dry sample; at center the settled band gain is the
computed gain. Time-varying gain creates sidebands, so assess consonants as well as stationary
frequency response. Do not apply a second undocumented attack/release smoother to GR.

**Finite/realtime contract.** Use f64 intermediates and filter/envelope state for the new core,
not an unchecked reuse of the f32 section's overflow-to-zero `flush`. For a conservative
initial design, bound each integrator write to ±H and envelope to [0,H], H=16*f32::MAX;
coefficients are bounded by the stated fc/Q ranges. Then every multiply/add before those
bounds fits f64 even on arbitrary parameter jumps. Clamp finite output/listen temporaries
to ±f32::MAX before conversion. Define these as extreme-input behavior and instrument zero
activations on normal fixtures; do not clip normal dry audio to ±1. NaN/infinite caller input
has a separate sanitize/reset policy. This is a specified finite design, not evidence that
the current Filter already meets that domain. Parameter smoothing and live range decreases
must keep effective gain in `[10^(-R/20),1]` at each sample.

Preallocate channel state in prepare, zero it in reset, snap restored controls, and recompute
bounded coefficients on a specified per-sample smoothing schedule. Per-sample processing,
zero reported latency, no lookahead, locks, allocation, logging or global RNG. Keep controls
and meters outside the inner loop's ownership transfers. Fingerprint every filter memory,
envelope, coefficients, parameter-smoother state, mode/listen transition state and any sample
counter; retain a deterministic gain-reduction readout. Existing precedents are
`filter.rs:510–525` and `dynamics.rs:213–224`, not permission to omit new causal state.

**Deterministic evidence to implement:**

- Synthetic vocal fixture: a 220 Hz vowel tone at amplitude .1 plus a 200 ms Hann-windowed
  burst containing 6, 7 and 8 kHz tones at .08 each, in a one-second render. Fix phases and
  frame offsets; render at all six normal rates. Use additional 7 kHz steady plateaus for
  center-band gain measurement and 220 Hz-only segments for false-trigger checks. At low
  rates use separate below-Nyquist robustness signals, not aliased copies labeled 6–8 kHz.
  Compare native Band/Wide and corrected DeBess; measure band attenuation, vowel loss,
  onset leakage, release and removed-signal energy. DeBess's controls are not calibrated
  equivalents, so compare at matched measured attenuation, not matching knob positions.
- Gain law in isolation: 12 dB above threshold at 4:1 requests 9 dB; R=6 caps it at 6 dB.
  Below threshold, ratio=1 and R=0 give unity. Test endpoint neighbors, large finite levels,
  mode/listen identities and immediate compliance when R is lowered during reduction.
- Inject constant detector levels independently of the filter: from zero,
  `e[m]=p*(1-c_attack^m)`; after a drop to zero, `e[m]=e0*c_release^m`.
  At N samples the error is approximately `exp(-1)`, subject to the specified numeric
  tolerance. Attack/release are sample counts, never sleeping or elapsed-time assertions.
- Unequal/antiphase stereo, silence/subnormals, impulses, DC, ±f32::MAX and hot-to-quiet
  recovery; all state writes finite under parameter jumps. Reset/fresh identity, restored
  parameters, interleaved instances, matched automation at sizes 1/2/7/32/64/127/256/1024
  and irregular schedules. Compare audio bits and causal-state fingerprints at common
  frame boundaries; aggregate meter readouts on identical sample windows. Verify zero
  process/reset allocations. An intentional detector/gain perturbation must fail its test.

These tests and a blinded listening comparison are outstanding acceptance work. Reuse alone
does not establish sound quality. A dedicated small ADR is preferable to adding de-essing to
the Filter's note/LFO interface; implementation can still be a Filter-primitives slice.

## User convention: defer to Ableton and Logic

Ableton's documented stock workflow uses **Multiband Dynamics**: upper crossover around
5 kHz, threshold and ratio, fast attack/release and band solo. Its Amount changes compression
intensity, not a dedicated maximum-reduction ceiling; the sidechain has a headphone listen
button. Do not invent a separate built-in Live de-esser with Logic's exact control set.
[Ableton Live manual, Multiband Dynamics](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/#multiband-dynamics).

Logic **DeEsser 2** exposes Frequency, Threshold, Max Reduction, Split/Wide, filter shape,
Relative/Absolute detection and Filter Solo. Its Range Split/Wide is distinct from the
attenuation ceiling. [Apple's DeEsser 2 reference](https://support.apple.com/en-ae/guide/logicpro/lgcef1bec850/mac),
[Apple's setup guide](https://support.apple.com/en-gb/102006).

The shared transit task flow should be **locate the band → listen → set sensitivity/threshold
→ limit reduction → return to processed audio**, with a reduction meter. The native proposal
can use physical units directly. If DeBess wins, keep Intensity/Sharpness/Depth/Filter as
honest percentage travel and Processed/Removed listen; Filter is not a calibrated frequency
and Intensity is not a dB threshold. Familiar layout does not authorize a false unit conversion
or retuned detector (`airwindows-catalog.md:201–208`). A separately calibrated interface would
be another design decision, not free renaming.

## Open questions for the maintainer

1. Prefer the native frequency/threshold/range contract, or Johnson's detector with explicit
   normal-use differences? Rank 1 assumes conventional stock controls take priority.
2. Start with Band as proposed, offer Wide immediately, and expose Detector plus Removed
   listen? Confirm the small advanced panel (Width, Ratio, Attack, Release).
3. Approve the full finite-input policy, including extreme-only state/output bounds? For
   DeBess this specifically requires a new normal-domain exception and output-bound authority.
4. Which consented vocal recordings and listeners should judge sibilance removal versus
   lisping/dulling at matched reduction? No listening verdict is asserted here.
5. Confirm a new stable id and a small native ADR, or the DeBess amendment with its larger
   oracle cost. Keep the inherited de-esser retired in either case.

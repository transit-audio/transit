# ADR 0042: Stock effects from the Airwindows catalog, with stability bounds and bit-exact Rust ports

- **Status:** proposed 2026-11-02
- **Date:** 2026-11-02
- **Reopen when:** a selected effect cannot pass the three-platform null or realtime gate within the stability-patch contract below; its rate or block behavior cannot fit the insert contract; the upstream license changes; or a demonstrated mixing gap calls for a different selection.

## Context

[ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md) removed the inherited modulation
effects and de-esser and kept dynamics, EQ, reverbs, saturation and gain until replacements land.
Issue #305 asks for a small stock collection from Chris Johnson's original MIT Airwindows source,
with transit interfaces and the port discipline of [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md).
The reference makes fidelity testable; designing every algorithm ourselves would add sound-design
work without that reference.

The [catalog survey](../contributing/knowledge/airwindows-catalog.md) examines the supplied upstream
checkout, commit `7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a` (2026-10-31), with 532 Linux source
directories. The authority is `plugins/LinuxVST/src/<Name>/<Name>.cpp`, `<Name>Proc.cpp` and
`<Name>.h`, plus Johnson's `Airwindopedia.txt` at that commit. Parameter labels below are exact
source spellings, including abbreviations. Quoted effect names are Chris Johnson's, not transit
product names. A numeric suffix identifies a line's revision; an unsuffixed name has no separate
numbered release here. The commit identifies both precisely; the common `getVendorVersion() = 1000`
does not distinguish revisions.

These are not all stateless shapers. They contain delay memories, feedback, detector histories and
seeded noise. Some deliberately change tone with sample rate. Some also depend on the process-call
length: a per-sample loop is not proof of block independence. Fidelity includes those properties.

## Decision

### Selection and order

Choose one algorithm for each distinct job, favoring the newest revision in that line and the
author's recommendations over historical popularity. A new revision does not silently replace a
shipped port: it needs a new provenance review, nulls and an explicit session-compatibility decision.
Do not import the whole catalog or fill the browser with neighboring versions. Two modulation
jobs are intentional: one moving tap and a multi-tap ensemble. The console encoder and decoder
are one paired system, not two competing colors.

The first set is **12 inserts**, implemented in the following three slices after an oracle-only
foundation. The amendment below admits stability patches and returns Tape Color and Wave Saturator
to slice 1 and Large Plate to slice 3. Selection is not a claim that the implementation gates have
passed: each replacement lands only with its patched-reference and normal-domain null evidence.
All source names in this table are Johnson's.

| Slice / gap | Exact source name and revision | Parameters in source order | Transit name / stable id | Reason |
|---|---|---|---|---|
| 1: chorus | "Chorus" (unnumbered) | 3: Speed, Range, Dry/Wet | Modulated Delay / `modulated_delay` | One moving tap, with a wet vibrato setting; distinct from an ensemble. |
| 1: ensemble | "ChorusEnsemble" (unnumbered) | 3: Speed, Range, Dry/Wet | Ensemble Delay / `ensemble_delay` | Four taps provide the richer pad treatment the single tap cannot. |
| 1: character compression | "Pressure6" (6) | 2: Compres, Ratio | Character Compressor / `character_compressor` | Latest compact, stereo-linked character compressor, aimed at bus compression in the author's notes. |
| 1: tape | "ToTape9" (9) | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | Tape Color / `tape_color` | Latest tape core, brightness compression and output clipper; stability patch bounds both companding coefficients. |
| 1: saturation | "Density3" (3) | 4: Density, Highpas, Output, Dry/Wet | Wave Saturator / `wave_saturator` | Preferred general saturation revision; stability patch bounds the high-pass coefficient. |
| 2: console channel | "Console9Channel" (9) | 2: Pan, Fader | Console Encode / `console_encode` | Latest numbered standalone summing encoder, without a whole channel-strip processing chain. |
| 2: console buss | "Console9Buss" (9) | 2: Pan, Fader | Console Decode / `console_decode` | The matching decoder is required after the encoded channels sum. |
| 2: clipper | "ADClip9" (9) | 5: Boost, Match, Noise, Ceiling, Mode | Peak Clipper / `peak_clipper` | Latest line, with level matching and a clipped-signal audition mode. |
| 2: dither | "TPDFDither" (unnumbered, two-control Redux source) | 2: Quant, DeRez | Output Dither / `output_dither` | A general output word-length reducer; the author recommends this simple option when special noise shaping is unnecessary. |
| 2: slew | "Slew4" (4) | 1: Compres | Treble Tamer / `treble_tamer` | Latest dedicated brightness compression, without the tape stage. |
| 3: hall | "kGuitarHall2" (2) | 6: Regen, Derez, Filter, EarlyRF, Positin, Dry/Wet | Small Hall / `small_hall` | A hall with early-reflection positioning, rather than an ambient infinite-space substitute. |
| 3: plate | "kPlate140" (unnumbered model 140) | 5: Input, Regen, DeRez, Predlay, Wetness | Large Plate / `large_plate` | The larger, prominent plate model; stability patch bounds the six SubTight histories. |

The named sections of [Johnson's notes at the pin](https://github.com/airwindows/airwindows/blob/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a/Airwindopedia.txt)
support these version choices: "Console9Channel", "ADClip9",
"Slew4", "kGuitarHall2" and "TPDFDither". The last includes its Redux update;
its earlier no-controls description is historical. "kPlate140" and "kPlate240" are different
plate models, not revisions 140 and 240. The former's notes explicitly describe the larger, more
prominent character; it joins the hall with the stability patch below. The later
stereo modulation variants are different designs, not numbered updates to "Chorus": the first
set deliberately keeps the single-tap and four-tap designs with
independent speed, range and mix controls.

**Three stability counterexamples are retained as regressions.** These are source-derived
analytical/binary64 traces, not compiled C++ oracle results. The amendment changes the reference
contract deliberately; it does not dismiss the findings or relabel them initialization defects.

- **Tape:** in `ToTape9/ToTape9Proc.cpp`, at 8 kHz and Shape = 1,
  `iirDecFreq = C / (rate/44100) = 5.5125`, giving history multiplier -4.5125.
  Fresh state, constant stereo `0.1f` and controls `[.5, 0, 1, 0, .5, .5, 0, .5, 0]`
  reach infinite decoder output on sample 472; zero output gain makes NaN and the delayed
  clipper emits it on the next sample. Bounding `iirEncFreq` and `iirDecFreq` above by **1.0**
  makes the IIR and companding history updates convex combinations; at this corner the decoder
  multiplier becomes zero. The `highPart` clamp alone never bounded `compDecL/R`.
- **Saturation:** in `Density3/Density3Proc.cpp`, at 8 kHz and Highpas = 1,
  `iirAmount = B^3 / (rate/44100) = 5.5125` gives the same multiplier. Constant input 1,
  zero history and controls `[.25, 1, 1, 1]` leave 75% of the growing high-pass residual;
  its contribution is about `1.38e39` by sample 60, outside f32. Bounding `iirAmount` above
  by **1.0** makes `iirSampleL/R` convex histories; at this corner each becomes the current
  input and the high-pass residual is zero. Keep the original zero-coefficient reset.
- **Plate:** in `kPlate140/kPlate140Proc.cpp`, each of three SubTight stages per channel
  computes `s = .5 + abs(u*.5)`, `v = h + sin(h-u)*s`, then `h = v*s`.
  The downstream temporary-output clamp does not bound `h`. Fresh state, 44.1 kHz, constant
  stereo `2000.0f` and controls `[1, .5, 1, 0, 1]` produce infinite `subCL` at sample 51
  and NaN `outSampleL` at sample 53 in the recorded trace, including Bezier accumulation
  and zero-predelay startup; exact timing may vary by math library. Clamp each newly computed
  `subAL/AR`, `subBL/BR`, `subCL/CR` to **[-1.0, 1.0]** immediately at its history write.
  Retain the original `v` passed to the next stage. Then `abs(v) <= 1+s`, so history cannot
  grow across updates; for finite f32 input, the three stages' products fit binary64 before
  the clamps. This closes the recorded SubTight runaway without moving the output clamp.

For both coefficient patches, values already lie in [0, 1] at every normal-domain rate, for
all normalized controls; only a value above 1 is replaced, without recomputing values inside
the interval. The plate's normal-domain invariant and the required dual nulls are below.
These local recurrence arguments do not replace the whole-effect finite-state/output audit.
Do not substitute an older tape, saturation or plate revision without its own analysis.

**Sibilance Control ("DeBess") is deferred from the first set.** Its gap remains open pending
a decision defining bounded B-phase division and the corresponding exception to normal-domain
transparency. At 44.1 kHz, fresh state and one frame per call, hold B=C=D=E=0; on odd frames
set A=0 and feed +1, and on even frames set A=1 and feed -1, identically on both channels.
Sharpness is 2, speed .05, depth 10000 and the IIR coefficient zero. Sense alternates between
0 and 8192. Starting with all ratios at 1 and `flip=false`, the B ratios decay by .95 on odd
frames while the A ratios exceed 1 from frame 2 onward. The B branch tests `ratioAL/AR > 1`
but divides by `ratioBL/BR`, so odd frame `2k-1` after the first emits `1/(.95^k)` before
dither. A source-derived binary64 evaluation gives `ratioBL=2.8962573551544587e-39` and
output `3.452731844496842e38` at frame 3459, beyond finite f32 before the `frexpf` cast.
The right channel follows the same recurrence; denormal injection does not apply and dither
does not feed the detector. This is not a compiled oracle measurement. It occurs inside the
otherwise normal rates, input and control ranges, so an additional bound cannot both close it
and remain inactive throughout normal use. The stability amendment does not authorize that
exception. Retain this schedule for any future admission decision; do not silently change the
branch condition, narrow valid automation, mute the adapter or substitute "DeEss". ADR 0040's
already-retired de-esser is not restored.

Other deferred candidates, with source details and reasons in the survey: "Vibrato", "Flutter2", "PhaseNudge",
"StereoChorus", "StereoEnsemble", "Capacitor2", "PurestGain", "BitShiftGain", "PurestDrive",
"NotJustAnotherDither", "Galactic3", "Verbity2" and "kPlate240". Historical candidates
"Console7Channel"/"Console7Buss", "ToTape8", "Density2", "DeEss", "ADClip8", "Slew3",
"Galactic", "Galactic2", "Verbity" and "kPlateA" are surveyed but not scheduled alongside their
successors. In particular, "PhaseNudge" is an allpass phase rotator with mix, not a complete
modulated multi-stage phaser. A dedicated phaser and feedback flanger remain later product work;
this ADR does not claim those gaps are fully closed.

### Relationship to retirement and routing

New inserts use new ids. No removed id is revived, and no old slot is silently substituted.
Slice 1 retires the inherited Saturator when Wave Saturator passes its gates and lands; Tape
Color fills the separate tape gap. Slice 3 retires Room Reverb and Plate Reverb when Small Hall
and Large Plate respectively pass and land. Each retirement uses ADR 0040's load report,
dropped parameter targets, emptied slot and modified-session rule, with corpus coverage.
A delayed replacement leaves its inherited device available until that replacement lands. The
character compressor has no external key, gate or conventional full dynamics section: the existing
Compressor/Limiter and Expander/Gate stay. EQ and Gain also stay; the deferred gain utility is not
permission to remove the pass-through used by tests.

The console pair is opt-in inserts, never an automatic change to the mixer. Help gives the topology:
processing and ordinary gain/pan first, Encode last on contributing channels, unchanged summing,
Decode first on the receiving bus. Keep mixer gain/pan between Encode and Decode neutral and use
the pair's controls for those adjustments. For a processed return, decode before processing and
encode again before the next encoded sum. Nested sums need matching decode/encode boundaries.
Test an actual routed session against that same ordered sum of oracle instances; do not imply that
placing only the decoder on Main reproduces the system.

## Amendment 2026-11-02: stability patches

Derek admits minimal stability patches to the vendored originals: "I'm OK with patches to fix
things like overflows/infinity issues." This supersedes the three deferrals and the blanket ban
on changing defined DSP for numerical stability. Earlier judging responses remain the historical
record; their counterexamples stand. Console initialization, noise, stereo coupling and counter
preservation findings remain closed and unchanged.

The separate normal-domain "DeBess" counterexample above defers Sibilance Control; it does not
weaken this amendment's transparency requirement or change the three restored selections.

**Form and limits.** Follow ADR 0033's existing build-overlay mechanism: a unified diff under
`crates/airwindows-oracle/patches/`, listed with its reason in that crate's `VENDOR.md`, applied
by the build to copies of the byte-identical vendor files. The concrete precedent is
[`patches/all/0012-modal-voice-fm-phase-offset-saturates-like-the-hardware.patch`](../../crates/macro-osc-oracle/patches/all/0012-modal-voice-fm-phase-offset-saturates-like-the-hardware.patch)
and its entry in [`VENDOR.md`](../../crates/macro-osc-oracle/VENDOR.md), recorded under ADR 0033's
2026-11-01 modal-voice implementation note: the FM phase-offset saturation changed the port and
reference together. That precedent defines an undefined conversion; this amendment additionally
allows bounds on unstable defined arithmetic. It does not imply that every DSP change is allowed.

A stability patch is only the minimal coefficient clamp, history clamp or divisor guard needed
to keep **every state variable finite for every admitted rate and any finite input**, with finite
output too. Every changed variable, numeric bound, placement and reason must appear in a comment
in the patch, in `VENDOR.md` and in the catalog's patch table. Preserve operation order inside the
bound, signed zero, noise and seed advances. Apply the definition in the Rust port at the same
point. A patch for taste, retuning, oversampling, latency, CPU or general cleanup remains forbidden.
So does substituting a different revision without its own analysis or silencing NaN in the adapter
instead of fixing the reference. No narrower per-effect rate or input admission is introduced.

**One admitted domain for the entire set.** All normalized controls in [0, 1], arbitrary valid
parameter schedules, and **any finite f32 input**, including both signs of `f32::MAX`, are in scope
for the selected `processReplacing` path. Internal binary64 arithmetic and the final conversion
to f32 must remain finite. NaN/infinite caller input belongs to the separate adapter containment
contract; a future f64 audio path needs a new domain proof and f64 nulls.

The engine-rate assumption is every integer rate **8,000 through 768,000 Hz**, inclusive:
`crates/engine/src/cmd/file.rs` constructs sessions through `SampleRate::new`, whose interval is
in `crates/time/src/lib.rs`; `crates/playback/src/device_clock.rs` checks that same interval.
`crates/playback/src/devices.rs::STANDARD_RATES` lists 44.1, 48, 88.2, 96, 176.4 and 192 kHz
as device choices, not a restriction on the engine. `SampleRate::COMMON` also lists 352.8 and
384 kHz; **705.6 and 768 kHz are admitted now** even though not in those menus.
The implementer must confirm every prepare/render entry point against this assumption before
admission, including offline renders and loaded sessions, and extend the audit if any admits
more. The earlier text called 1 kHz an export lower edge: the directly verified engine command
is `file.prepare_import` (1–768 kHz), whose comment says mismatched prepared rates cannot be
committed. `crates/dsp/src/util.rs::sane_sr` independently accepts 1 kHz–1.536 MHz; it is not a
session-rate validator. Keep the existing 1 kHz robustness probe; do not infer a narrower engine
domain from a menu or silently clamp/convert a rate. A patched effect must pass at every admitted
rate; an unpatched effect can land only with its own finite-state/output analysis and the same gate.

**Normal-domain transparency.** For each effect the normal rates are exactly 44.1, 48, 88.2,
96, 176.4 and 192 kHz. The normal input domain is per-channel peak magnitude at most 1.0
(0 dBFS) for Modulated Delay, Ensemble Delay, Tape Color, Wave Saturator and
Small Hall. Character Compressor, Console Encode/Decode, Peak Clipper, Output Dither, Treble
Tamer and Large Plate additionally cover bus headroom through **+12 dBFS** (magnitude
`10^(12/20)`, approximately 3.981072). Full normalized control travel remains normal, including
maximum drive, regeneration and wet/dry settings. Start from the documented fresh/reset state;
all preceding samples and parameter events in a normal-domain render obey those bounds.
Returning from an extreme-input history must stay finite, but is not a promise to erase the
different history and immediately null against an original that overflowed.

The following three patch specifications close the recorded recurrences. These are source
arguments and required proof obligations, **not completed null measurements**.

| Effect | Variable and bound, applied after the original expression | Counterexample closed | Normal-domain proof required |
|---|---|---|---|
| "ToTape9" / Tape Color | Cap `iirEncFreq` and `iirDecFreq` at 1.0 before either drives the IIR or companding histories. Leave 0 unchanged. | At 8 kHz, Shape = 1: 5.5125 becomes 1, so decoder history multiplier -4.5125 becomes 0; mirror coverage at Shape = 0 tests the encoder. | At every normal rate both coefficients are already in [0, 1] across the full Shape range. N1–N3 below must show unchanged coefficient bits and zero clamp activations, plus exact output/state/seed bits against the reference without stability patches. |
| "Density3" / Wave Saturator | Cap `iirAmount` at 1.0 before the zero-coefficient reset and L/R high-pass updates. | At 8 kHz, Highpas = 1: the history becomes current input, eliminating the `4.5125^n` residual and the sample-60 overflow on the recorded controls. | `B^3/r <= 1` at all normal rates for every B in [0, 1]. N1–N3 must show zero activations and exact output/state/seed bits, including density below, at and above 1 and 2 internally. |
| "kPlate140" / Large Plate | Clamp `subAL`, `subAR`, `subBL`, `subBR`, `subCL`, `subCR` to [-1.0, 1.0] at each `outSample*scale` history write; do not change the outgoing temporary. | The 2000.0-input trace cannot accumulate an unbounded history; each stage starts its next update with magnitude at most 1. Its final temporary still follows the original clamp. | The invariant below places the six normal histories well inside 1, including +12 dBFS. N1–N3 must compare every history, output and seed bit and record zero history-clamp activations through maximum Regen, all DeRez/predelay settings and tails. |

For the plate invariant, bound the normal bus input conservatively by 4. The positive Bezier
accumulation weights total at most 2 per internal update (the cycle exceeds 1 before reset), so
`bez_Samp` is at most 16 and the two-value average times .125 is at most 4. Predelay only selects
stored values. For `abs(u) <= U`, `abs(h) <= H`, use `abs(sin(h-u)) <= H+U`:
`abs(v) <= H + (.5+.5*U)*(H+U)` and `abs(h_next) <= (.5+.5*U)*abs(v)`.
Starting with `U = .00187*4 = .00748`, conservative invariant history bounds **.01, .022, .06**
for stages A, B, C respectively give outgoing magnitudes below .01881, .04280, .11361 and
next-history magnitudes below their respective bounds. They leave ample rounding margin to 1.
Controls cannot amplify the input pad above 1; Regen and the feedback network are downstream
of SubTight. The implementation must check the accumulation bound under DeRez changes and
compiler rounding as well as fixed controls; the invariant is not a substitute for N1–N3.

**Two references and two nulls per patched effect.** Build isolated reference variants from the
same pin, compiler, flags, deterministic seeds and necessary initialization/UB definitions:
one with stability patches, one without them ("unpatched" here means no stability patches,
not different seeds or undefined initialization). The Rust port must null against the patched
reference over the admitted-domain fixtures, and separately against the unpatched reference
over **all** of these normal-domain fixtures, on each of the three compiler families:

- **N1, `normal_full_sweep`:** the complete Gate 3 parameter sweep, defaults, interaction
  endpoints, list choices, branch-adjacent f32 values, ramps and jumps, at all six normal rates;
  silence/subnormals, signed impulses, positive/negative DC, alternating signs, unequal stereo,
  tones and seeded broadband input up to each effect's stated normal limit. Include both 0 dBFS
  and +12 dBFS limits for bus tools. The full sweep uses no stride for initial admission/release.
- **N2, `normal_long_floor_and_null`:** Gate 4's at-least-60-second render with at least 10
  seconds of silence, motion and excitation, extended until enabled delay paths fill and wrap
  twice, at each normal rate. Both C++ variants get the in-process/cross-process floor, then
  both port comparisons; capture internal state and clamp activation counts, not just output.
- **N3, `normal_lifecycle_and_schedules`:** Gate 3/7's raw and adapter construction/reset,
  interleaving, bypass and prepare scenarios, every listed block schedule with matched event
  boundaries and normal inputs throughout. The plate includes the slowest DeRez and longest
  predelay, control jumps during accumulation, and a fully excited maximum-Regen tail.

These are required fixture names/specifications for implementation, not files that exist yet.
All three compare output bits (signed zero included), every initialized processing-state value
and RNG words; exclude padding and never-read spare storage. Require zero stability-clamp
activations on normal renders and zero differing bits. Also compare patched and unpatched C++
directly on those renders. Turning off a stability patch must fail its extreme regression;
deliberately changing an in-domain coefficient/history operation must fail the normal null.

**Whole-domain admission still needs proof.** The three local fixes do not certify every other
operation. Audit all state writes, intermediates, feedback, casts and output dither across the
whole admitted domain and arbitrary-length input streams, with analytic bounds plus fixtures.
In particular, the Density3 branch below internal density 1 uses an input-dependent fifth-degree
polynomial: with controls `[0, 0, 1, 1]`, finite input `1.0e10f` produces about `2.7557e43`
before dither, beyond f32's approximately `3.4028e38` maximum, even with the coefficient cap.
This source-derived calculation is a separate required regression, not a compiled oracle result.
A maximum-f32 dry path plus output dither also needs
explicit scrutiny, including in the plate. Record any further required bound with the same
variable/bound/reason and normal-domain proof before admission; a repair beyond coefficient,
history or divisor bounds requires another decision. Do not call the three regressions a proof
for all finite inputs or reduce that domain to make a test pass. An unresolved case blocks the
affected implementation, not the safety gate or the selection of its revision.

**User documentation.** Each patched effect's help carries this sentence, substituting the
actual variable names (and including any further admitted bounds): "transit bounds `<variable>`
so the effect stays finite at extreme settings; within normal use it is sample-identical to
Chris Johnson's original." Define normal use there by the effect's rates and input/headroom
limit above. The catalog carries the patch table and the proof results when implemented;
release notes distinguish stability patches from deterministic initialization fixes.

## Implementation

### Original source and one shared oracle

Use **one `transit-airwindows-oracle` crate**, at `crates/airwindows-oracle`, separate from the
instrument oracle. It holds only the selected originals as they land, not all 532 effects. One
compiler configuration, seed protocol, subset policy and provenance check are less likely to drift
than per-family oracle crates. Cargo builds this small selected library once per configuration;
an unrelated port test may pay its compile cost, which is the accepted tradeoff. Family modules,
separate C++ translation units and split build helpers keep it within the Rust file-size budget.

Follow [oracle-null-testing.md](../contributing/knowledge/oracle-null-testing.md) and
`crates/macro-osc-oracle/{README.md,VENDOR.md,build.rs,tests/}`:

- `vendor/plugins/LinuxVST/src/<Name>/` contains the original `.cpp`, `Proc.cpp` and `.h`, byte
  for byte, with original notices; vendor the upstream LICENSE too. Record URL, full commit,
  file set and blob ids in `VENDOR.md`. Mark vendored paths `-text` and verify their committed
  blob ids against the pin. No community port, binary extraction or alternate catalog is a source.
- `cpp/` supplies a minimal test-only host facade for the original classes, sample rate and parameter
  access, and a C ABI per family. Compile the original processing functions with the recorded
  patch overlays, plus the separate no-stability-patch variant for normal-domain comparisons;
  never copy a second implementation. Isolate duplicate entry points, global enum names and header
  guards per translation unit using wrapper/build symbol isolation. Record the mechanism; it must
  not change arithmetic.
  Do not pull a proprietary host SDK into the oracle or ship host wrappers.
- `src/` contains descriptive family wrappers, sweep definitions, floor and residual helpers;
  `tests/` holds floors, isolation and scenario checks. Add the crate to `DEV_ONLY` in the layer
  gate; `publish = false`, only dev-dependencies, no app linkage. Production crates forbid unsafe.
  On acceptance, this ADR extends ADR 0013's existing dev-oracle exception to this crate's
  **`src/ffi.rs` only**: safe checked API, documented handle/buffer lifetimes, lengths, parameter
  and rate bounds, and `// SAFETY:` on every unsafe block. Register that narrow allowlist entry
  before the first oracle lands; no general FFI exemption for the ports.
- `patches/` contains small deterministic unified diffs applied only to build-directory copies.
  Allowed changes pin constructor RNG seeds, explicitly initialize reads of unwritten state, and
  define demonstrated UB or unspecified evaluation that the three compilers expose. Each patch
  has the original expression, counterexample, chosen definition, rationale and a discriminating
  regression in `VENDOR.md`; application fails if the expected source does not match. Reproduce
  the same definition in Rust. Stability patches additionally follow the amendment's scope,
  comments, catalog table and dual references. No tone correction, coefficient change outside
  that scope, reordered accumulation, removed noise, added oversampling or general cleanup.
  Other desired changes to defined DSP reopen this ADR instead of hiding in an oracle patch.

One concrete initialization defect is already visible: "ADClip9" declares `lastDryL/R`, reads
them into `overshootL/R` before writing them in `Proc.cpp`, and its constructor never initializes
them. The foundation must add a recorded constructor patch initializing both to zero, then prove
the floor; reading zeroed heap storage by accident is not evidence. Arrays with spare uninitialized
end elements also need bounds/read-before-write audits, not blanket claims that every element is used.

Each selected processor has instance-owned DSP state, but its original constructor uses global
`rand()` to choose `fpdL/R`. Replace those draws in the deterministic reference, before any
processing, with fixed nonzero test seeds (default `0x12345678`, `0x87654321`). The harness can
set both words explicitly and reads them back. The production Rust instance uses the same default
seeds and restores them on reset; no process-global RNG or audio-thread entropy source. Any other
noise history, phase or distribution state is part of initialization and fingerprinting. A
different-seed probe must demonstrate that the seed reaches a noise-sensitive output. Interleaving
two instances must not affect either result. Deferred "DeEss" needs more than this: its static
noise counters and signed-overflow arithmetic would require a separately justified deterministic
definition before admission.

Build the C++ with `cc`, optimization level 2, no fast-math and no contraction (`/fp:strict` on
MSVC; `-fno-fast-math -ffp-contract=off` on GCC and Apple Clang). Record toolchain, flags and
floating-point mode with the floor. Use default round-to-nearest and gradual underflow here;
do not copy the instrument firmware's FTZ setting into this desktop reference. Preserve the
source's own denormal injection. Rust must not change the host's floating-point mode. The `l`
suffix in floating-point dither expressions can mean extended `long double` on GCC x86-64;
the port must reproduce the reference's intermediate rounding where it affects the result,
using safe arithmetic helpers if necessary. Removing that suffix, substituting approximate
transcendentals or accepting a small residual is not a determinism fix.

### Rust families and the insert boundary

| Rust crate | Inserts |
|---|---|
| `transit-modulated-delay` | Modulated Delay, Ensemble Delay |
| `transit-character-dynamics` | Character Compressor |
| `transit-tape-color` | Tape Color |
| `transit-wave-saturation` | Wave Saturator |
| `transit-console-summing` | Console Encode, Console Decode |
| `transit-output-tools` | Peak Clipper, Output Dither, Treble Tamer |
| `transit-space-reverb` | Small Hall, Large Plate |

Production crate names, Rust identifiers, stable ids, parameter ids, preset ids and device titles
are descriptive, without upstream brand-like effect names. Original names remain in unchanged
vendor files and provenance/credit text. The explicitly named dev-only oracle is the catalog-name
exception, not a naming pattern for production crates.

The shipped app is Rust. Keep the original operation order, literals, casts, precision, integer
wrapping where defined, state update order and per-sample loop, with only the recorded stability
bounds and deterministic definitions. The transit f32 audio path ports
**`processReplacing`**, including its actual noise operations and double intermediates; it must not
call the double path and cast at the end. In particular, "ADClip9" comments out float-output dither
additions but retains RNG advances, the conditional extra right-channel advance and clipping noise;
do not add ordinary float dither to it. `processDoubleReplacing` remains vendored for reference;
claiming a future f64 path requires separate f64 nulls. No SIMD regrouping, generic smoother,
oversampling, resampling, mix-law correction or latency removal is added. Preserve any rate
division, interpolation or filtering already inside the original.

Preserve the specific quirks recorded in the survey: "Slew4" uses the right channel's
`depthTrim` in both final fractional-depth blends; "kGuitarHall2" leaves `c6ZAR` at its
initialized index while advancing `c6ZBR` twice per internal update. Both precision paths do
this. These are defined processing behaviors, not initialization defects eligible for patches.

"ADClip9" and "ToTape9" copy their intermediate and slew arrays downward in descending index
order, propagating the just-written value through the array. Preserve those copies in both
precision paths; replacing them with an ordinary delay-line shift changes the algorithm. On the
unclipped clipper-stage path, `lastSampleL/R` becomes the current sample and is emitted on the
next frame even when `spacing > 1`. Establish each insert's latency from output, not the spacing
comment; the tape's other stages still need their own latency analysis.

Keep block-entry work at block entry. In particular, the console pair interpolates `panA/B` and
`inTrimA/B` using remaining frames divided by the original call length. Preserve its constructor
endpoints (all 0.5) and resulting transient in the **raw core and constructor nulls**. The adapter
initialization below is a separate contract. The adapter passes each transit process span and
parameter event boundary through unchanged. Null each such schedule against C++ with identical boundaries; do not reblock into
single samples or assert equality across different block schedules where the original differs.
The scheduling oracle compares serial and worker runs of the **same** schedule. Help must disclose
that changing the audio block size can change these ramps. A later demand for block-independent
automation is an explicit algorithm change and reopens this decision.

**Console initialization at the Plugin boundary.** `Plugin::snap_initial_params` in
`crates/dsp/src/lib.rs` requires stored values to be initial state, not automation moves;
`crates/mix/src/instance.rs` applies the stored controls before calling it. For both console
adapters, implement that hook by setting **both pan endpoints to `A*1.57079633` and both trim
endpoints to `B*2.0`**, using the source's f32 controls promoted to double and its exact literals,
before the first sample. Expose the same explicit state-initialization operation in the reference
harness, outside the original processing function, and null this adapter setup separately from
raw construction. This is a documented host initialization rule, not a deterministic repair to
the vendored constructor. It changes no processing-loop operation or subsequent automation ramp.

**Console reset and prepare.** At the Plugin boundary, reset preserves the current controls,
restores the fixed default seeds and clears processing history, then snaps the endpoints to those
controls with the same operation. Prepare establishes this state too, including for a new default
insert when no stored parameters trigger the hook. `snap_initial_params` updates the endpoints
after restoration without resetting seeds or other state again. A reset after that hook must
therefore keep the snapped targets, never leave the constructor's 0.5 endpoints behind. Raw-core
reset tests use raw constructor state; adapter reset tests compare with a fresh, prepared and
snapped reference at the same current controls. Regular parameter events on a running instance
only update the controls; the original block-length-dependent ramps still apply.

Wrap each processor in `crates/dsp/src/plugins/` using `Plugin`, `PluginInfo`, `ParamInfo` and the
registry, as Texture does. Allocate state and scratch buffers in construction/prepare off the
audio thread; process, reset and parameter delivery allocate, lock and block nowhere. Preserve
stereo coupling; mono feeds both original inputs and takes the left output, and wider layouts
use independent adjacent stereo pairs (an odd last channel follows mono). Test these adapter
rules separately from the stereo core. Expose measured fixed latency through `Plugin::latency`
(notably the slew processor's two-sample lookahead and the clippers' delayed samples); modulation
delay and reverb predelay remain musical delays, not compensation targets. Do not guess all
latencies are zero because the old host wrapper reports none.

Implement `fingerprint` under `testing` for every mutable value that can affect a later sample:
parameters, ramps, seeds, detector/filter memories, delay contents and indices, lookahead and
adapter state. No empty default for these stateful inserts. Include each family in scheduling
equivalence scenarios. Files are split by concern before approaching 1500 Rust lines; no new
ratchet exemption, and no growth of an already over-budget integration file.

### Controls, help and presets

Store the original normalized f32 controls without a lossy display-and-inverse round trip.
Transit adds readable names, display units/ranges and editors, not a new transfer function.
For controls without a defensible physical calibration, show **0–100% control travel**, explicitly
described as such. Never invent milliseconds for compressor recovery or an input/output ratio
for the source's `Ratio` control. That control actually governs a nonlinear wet contribution.
Rename abbreviations in the UI (Compression, Early Reflections, Position, Predelay); the survey
keeps the exact source names.

Concrete exceptions to the percentage rule:

- Console Pan displays -100% left to +100% right (`200*a - 100`); Fader remains 0–100% travel,
  since its shaped gain is not an ordinary linear level control.
- Clipper Boost and Match display 0–18 dB (`18*a`, `18*b`). Noise and Ceiling remain percentages
  of their original threshold controls, not dB ceilings. Mode is a list: Boost, Match, Clipped
  Signal; normalized anchors 0, 0.5, 1 select the source's three modes, including "ClipOnly".
- Dither Quant is the list 16-bit / 24-bit (normalized 0 / 1); DeRez remains 0–100% travel and
  is not a linear bit-depth readout. The original quantizer's offset and floor operation stay.
- Hall Position is a list of positions 1–28: the source uses `int(E*27)`. Use interval midpoints
  for indices 0–26 and 1 for index 27, with tested boundary mappings. Its Derez is one continuous
  control with labeled halves (stepped left, full at center, continuous right); do not quantize
  it to a list because the right half is continuous.
- Tape Head Frequency displays `25 + 175*h*h` Hz using the source control arithmetic. Its
  other controls retain percentage travel; Input and Output are not assumed to share a gain law.
  Saturation Density may display the source's `5*a-1` amount, explicitly not dB. Plate Wetness
  retains unity dry and wet at its midpoint and squared wet gain below it; Predelay retains
  percentage travel with the internal-rate dependency explained, not an invented fixed time.

Each slice provides `docs/help/<descriptive-id>.toml`, registered help ids and hovered widgets,
a row in `docs/help/README.md`, and device documentation. Every control has its range, default,
meaning and dependencies. Each effect's help describes its own sample-rate behavior from the
survey: sample-based delays and modulation, rate-scaled detectors, discrete internal-rate changes,
or the absence of compensation. No promise of identical sound at different rates. Explain the
hall's resolution/position changes and preserved counter behavior. Dither belongs
last after gain, fades and sample-rate conversion; an insert before an export resampler does not
guarantee dithered final integer output. Do not silently enable or double up export dither.

Presets live under `assets/presets/<id>/` with stable descriptive ids: the source default plus
at least two useful settings per insert (paired console examples include topology instructions).
They are parameter sets, not DSP variants, and must load, round-trip and exercise the wet path.
The default preserves the original constructor's parameter values; a silence or neutral preset
does not license bypassing the original dither or state updates.

### Attribution

Each production family and the oracle carry `LICENSE-THIRD-PARTY` with the full upstream MIT
text, including `Copyright (c) 2018 Chris Johnson`, and preserve per-file notices. Record source
paths, the pinned commit and patches in each family's `VENDOR.md`; distinguish transit's code
license from the upstream MIT material. Add rows for shipped ports, the dev-only vendor files
and factory assets to `ATTRIBUTION.md`, and a Chris Johnson/Airwindows entry to `NOTICE`.

Each insert's help and Help > About transit say **"a port of Chris Johnson's <Name> (Airwindows,
MIT)"**, substituting the exact name from the table. About makes the full notice available,
as it does for Émilie Gillet's ports. Credit is the explicit exception to brand-free UI titles;
it is not an endorsement claim. Release notes say: "New stock effects with transit interfaces,
ported from Chris Johnson's Airwindows algorithms (MIT)." List transit names with their original
source names and revisions, link notices, describe deterministic and stability patches, and call
out ADR 0040 removals as breaking changes. Claim bit-exactness only for effects that passed the
gate, and name the pinned patched desktop reference and matched schedules. State the separate
normal-domain sample identity to the original only after N1–N3 pass.

## Gate

These are implementation gates, not claims that this documentation change has run DSP tests.

1. **Reference first.** Per effect and rate, render the same input/parameter schedule twice with
   fresh instances in one process, then in a second process. Compare every output bit in-process
   and complete render/state digests across processes. Required floor: zero differing samples,
   peak and RMS residual `-inf` dBFS. Repeat with different initialization memory patterns to
   expose unwritten reads. A failing floor blocks all port claims; identify and define the
   uninitialized/undefined behavior before proceeding. Compare the port against the recorded
   C++ reference on the same runner, not a Rust-generated golden. For stability-patched effects,
   run both reference variants and both nulls as specified by N1–N3 in the amendment.
2. **Rates.** Every effect runs the sweep and floor at 44.1, 48, 88.2, 96, 176.4, 192, 352.8,
   384, **705.6 and 768 kHz**. Include 8, 11.025, 16, 22.05, 24 and 32 kHz, plus the
   retained 1 kHz robustness probe. The common admitted domain and the entry-point confirmation
   obligation are defined once in the amendment, not separately for each effect.
   Bounds audits cover the entire admitted rate interval; add probes immediately on either
   side of every integer-divider/clamp transition and any newly supported rate. Do not silently
   clamp an admitted high rate to 192 kHz or add conversion to make the null pass. Distinguish
   UB/initialization definitions from the amendment's stability bounds in the patch manifest.
   Every selected effect requires a finite-state/output argument over the whole domain, including
   intermediate operations and output conversion; sampled nulls alone cannot establish it.
3. **Sweep and scenarios.** For each continuous parameter use 0, 0.25, 0.5, 0.75, 1 around
   source defaults, both endpoints of interactions, all list choices and adjacent f32 values
   around branch boundaries. Include all-controls-low/high, paired drive/output and rate/depth
   extremes, ramps and jumps. Use identical seeded stereo input: silence, subnormals, impulses,
   unequal channels, tones, broadband noise, hot finite samples and quiet tails. Exercise each
   mode's wet/nontrivial path after its delays fill; enumerate intentional silence separately.
   Compare f32 `to_bits()` including signed zero, and both RNG words; no epsilon, ULP allowance
   or noise-floor tolerance. Nonfinite output on valid finite input is a blocker, not an exact
   NaN match. Check internal state after every update in instrumented safety fixtures, not only
   output. Mandatory unstrided regressions are the three counterexamples above (both channels),
   the tape's mirror encoder corner, clamp-boundary neighbors and sign-reversed/alternating
   variants; run at least 4096 samples and extend through delayed emission and the long floor.
   Include positive/negative `f32::MAX`, adjacent finite values, large finite powers of two,
   `2000.0f`, sustained DC and hot-to-quiet recovery across every admitted-rate fixture. Never
   shrink hot-input levels to pass. Show patch activation and finite state/output in the patched
   reference and port, and the discriminating failure without the relevant patch. Test raw reset
   versus raw fresh construction, adapter reset versus prepared and
   snapped fresh construction, interleaved instances, bypass/unbypass and
   rates reconfigured through prepare. Use process sizes 1, 2, 7, 32, 64, 127, 256, 1024 and an
   irregular sequence, with the same schedule on both sides. Preserve upstream block dependence.
4. **Long floor and null.** At every listed rate render at least 60 seconds of sample time per
   effect, with parameter motion, an impulse/noise excitation and at least 10 seconds of silence.
   Run this for oracle self-comparison and port comparison; N2 repeats it against the unpatched
   reference at every normal rate within that effect's normal input domain. Extend slowest-delay/
   resolution cases until all enabled paths have emitted processed input and wrapped their longest advancing memory
   at least twice. Exercise stationary-index paths at their source-reachable indices without
   changing their counters; 60 seconds of an unfilled wet buffer is not evidence. Record frame
   counts and final state. No assertion depends on elapsed wall time or scheduling luck.
5. **Three compilers, feedback before review.** Windows x86-64 MSVC, Linux x86-64 GCC and macOS
   Apple silicon/Apple Clang each run floors, nulls and scenarios in CI's workspace tests. Require
   an AArch64 runner explicitly rather than assuming a moving `macos-latest` label supplies one.
   Each family compares only within itself; cross-OS bit identity is not claimed. Before handing
   a port, oracle patch or shared numeric helper to review, run the affected floors and nulls
   with GCC in WSL as well as MSVC, on a quiet host and private target directory. This is the
   feedback rule: GCC findings must reach the author before the reviewer or remote CI discovers
   them. Preserve failure output verbatim; never rerun a failed test to get a pass or its message.
   Investigate first and follow the loaded-host exception in the contributor gates.
6. **Cost control without coverage holes.** One oracle subset helper follows
   `TRANSIT_FULL_ORACLE=1`: full parameter sweep and its floor in release chains and before a
   new effect's first review on the three compiler families; ordinary CI runs fixed indices
   0, 10, 20, ... per effect/rate plus mandatory defaults, endpoints, modes and regressions.
   Print selected and total counts. Never stride away an effect, rate, long-run floor, long null,
   initialization check or scenario, either reference variant, or either kind of null. N1 follows
   the same full/strided policy; N2, N3 and every stability regression are mandatory in ordinary CI.
   Set the variable for the release workspace test. Measure
   build and test cost before changing this policy, not by dropping a slow test silently.
7. **Integration.** Adapter tests cover normalized/plain mappings, presets, save/load/reset,
   invalid parameters and input containment, no allocations on process/reset, reported latency,
   routing, help and scheduling fingerprints. If shared DSP primitives change, run the **full
   workspace test**, not only the new family, with full oracles; ordinary integration also
   follows CONTRIBUTING.md. Run `cargo xtask layers`, `assets`, `timing`, `help` and `file-size`;
   generate help with `cargo xtask help --write` before checking it. Refutation review under
   ADR 0009 includes each deterministic or stability patch, its normal-domain proof, FFI safety
   invariant and a deliberate perturbation that makes each new null suite fail. Fidelity does not
   replace the audio-thread safety gate.
   Console regressions cover restored Fader = 0, default and off-center Pan, prepare with no
   stored controls, and reset after restoration, at 1- and 64-frame first calls and irregular
   schedules. Check endpoint bits before rendering and output/seed bits against the explicitly
   snapped reference, retaining source dither rather than asserting exact silence at Fader = 0.
   A missing snap must fail the 64-frame restoration case. Subsequent automation and the separate
   raw-constructor case must still null against their original ramps.
   Unequal-channel slew scenarios must exercise the shared final `depthTrim`; hall wet-path
   scenarios must exercise the stationary and twice-advanced counters recorded in the survey.
   Clipper and tape scenarios must distinguish descending propagation from an ordinary delay-line
   shift at `spacing > 1`, compare intermediate/slew state and verify reported latency from output.

## Consequences

The stock collection fills distinct mixing jobs with small interfaces and a reproducible reference.
Tape, saturation and plate return to the selected first set through explicit stability bounds
and two reference comparisons. Their replacements retire inherited devices only when they land;
the de-essing, dedicated phaser and feedback flanger gaps remain outside this set.
Users get the author's algorithms and quirks, including sample-rate coloration, seeded noise,
nonlinear mix laws and block-length-dependent ramps. Gain staging and console placement need help
text; a famous algorithm name is not a substitute for it.

Maintainers carry C++ only in development, a compiler-dependent numerical reference, deterministic
and stability patches, dual reference variants and three-platform test cost. A bit-exact claim is
bounded by the pinned reference, runner and tested schedules; it is not a claim about every
released binary or every imaginable input.
The long-run and full-sweep gates can be expensive at high rates. A failed fidelity or safety gate
delays that insert rather than weakening the claim. No port is implemented or retired by this ADR.

## Alternatives considered

- **Design all effects ourselves.** Useful for uncovered needs later, but loses a mature reference
  and spends the first slices on algorithm design instead of closing the agreed gaps.
- **Port from another MIT catalog.** Licensing alone does not supply this breadth of compact,
  independently testable effects or satisfy #305's chosen source; another catalog can be evaluated
  for an uncovered job, not mixed into this oracle reference.
- **Keep the inherited collection.** Contradicts ADR 0040's direction and accumulates more sessions
  on devices scheduled for retirement. Only its explicit stage 2 dependencies stay until covered.
- **Ship every version, or a large channel-strip bundle.** Adds overlapping choices and multiplies
  maintenance, state and UI scope. Choose one algorithm per job and the small paired console core.
- **One oracle crate per family.** Offers smaller isolated builds but duplicates toolchain, seed,
  floor and subset policy. One crate with family modules fits the selected set and existing practice.
- **Ship the C++ or accept perceptual equivalence.** The former expands the production unsafe
  surface; the latter cannot enforce faithful translation. Keep the app Rust and the residual zero.

## Judging round 1: responses

C1 accepted: Deferred "Density3" / Wave Saturator from the first shippable set (now 12 inserts), retained the inherited Saturator under ADR 0040, documented the 8 kHz unstable recurrence and the required later domain or DSP decision, and removed its first-set crate and control mapping.

C2 accepted: Preserved raw console constructor nulls while specifying `snap_initial_params`, prepare and reset endpoint initialization at the Plugin boundary, the identical reference-harness operation, and discriminating restoration/reset/automation nulls.

M1 accepted: Qualified the catalog's common F convention, recorded "Density3"'s `3.553e-44l` dither and conditional extra right RNG advance, and recorded "ADClip9"'s disabled float-output dither, retained RNG advances and clipping noise in the catalog and port contract.

## Judging round 2: responses

C1 accepted: Deferred "ToTape9" / Tape Color from the first shippable set (now 11 inserts), documented the 8 kHz decoder counterexample and required later domain or DSP/reference decision, removed its slice, crate and control entries, and updated the catalog and indexes; the tape gap remains open.

M1 accepted: Corrected the "kPlate140" catalog state summary to identify `gainIn`, `gainOutL/R` and `pearA`–`pearF` as declared and initialized but unused in both processing paths, rather than active gain/Pear stages.

## Judging round 3: responses

C1 accepted: Deferred "kPlate140" / Large Plate for nonfinite SubTight histories on hot finite input, reduced the first set to 10 inserts, retained the inherited Plate Reverb, removed the plate from slice 3 and its family insert list, and documented the counterexample and required later input-domain/enforcement or DSP/reference decision in the ADR and catalog; updated both indexes.

M1 accepted: Recorded "Slew4"'s use of the right channel's `depthTrim` in both final fractional-depth blends in the catalog and port contract; preserve this coupling in both precision paths.

M2 accepted: Recorded "kGuitarHall2"'s stationary `c6ZAR` and twice-advanced `c6ZBR` in the catalog and port contract; preserve the initialized, bounded counter behavior in both precision paths.

## Judging round 5: responses

C1 accepted: Deferred "DeBess" / Sibilance Control, reducing the first set to 12 inserts. Its
one-frame alternating input/control schedule overflows f32 within normal use because the B phase
tests the A ratio while dividing by its decaying B ratio. Recorded the schedule, source-derived
frame-3459 result and required later division/transparency decision in the ADR and catalog;
removed the first-set row, family assignment, normal-domain entry and implementation control
mapping. The de-essing gap stays open, with no restored inherited device or substitute revision.
The three stability-patched selections and the normal-domain transparency gate remain unchanged.
Round 4's branch-asymmetry observation is preserved; its newly demonstrated overflow consequence
is accepted rather than treated as a previously refuted finding.

M1 accepted: Carried forward round 4's "ADClip9" descending-copy preservation note and extended
it to restored "ToTape9" in the catalog and implementation contract. Both intermediate and slew
copies propagate newly written values; they must not become ordinary delay-line shifts. Added
discriminating state/output scenarios at `spacing > 1` and retained measured insert latency,
distinguishing the clipper stage's one-frame delay from the whole tape processor's behavior.

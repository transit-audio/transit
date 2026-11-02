# ADR 0042: Stock effects from the Airwindows catalog, ported bit-exact with transit interfaces

- **Status:** proposed 2026-11-02
- **Date:** 2026-11-02
- **Reopen when:** a selected effect cannot pass the three-platform null or realtime gate without changing defined DSP behavior; its rate or block behavior cannot fit the insert contract; the upstream license changes; or a demonstrated mixing gap calls for a different selection.

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

The first set is **13 inserts**, implemented in the following three slices after an oracle-only
foundation. All source names in this table are Johnson's.

| Slice / gap | Exact source name and revision | Parameters in source order | Transit name / stable id | Reason |
|---|---|---|---|---|
| 1: chorus | "Chorus" (unnumbered) | 3: Speed, Range, Dry/Wet | Modulated Delay / `modulated_delay` | One moving tap, with a wet vibrato setting; distinct from an ensemble. |
| 1: ensemble | "ChorusEnsemble" (unnumbered) | 3: Speed, Range, Dry/Wet | Ensemble Delay / `ensemble_delay` | Four taps provide the richer pad treatment the single tap cannot. |
| 1: tape | "ToTape9" (9) | 9: Input, Tilt, Shape, Flutter, FlutSpd, Bias, HeadBmp, HeadFrq, Output | Tape Color / `tape_color` | Latest tape line, with the author's revised brightness compression and output clipping. |
| 1: saturation | "Density3" (3) | 4: Density, Highpas, Output, Dry/Wet | Wave Saturator / `wave_saturator` | General drive and inverse saturation with a bass-cut control, beyond tape coloration. |
| 1: character compression | "Pressure6" (6) | 2: Compres, Ratio | Character Compressor / `character_compressor` | Latest compact, stereo-linked character compressor, aimed at bus compression in the author's notes. |
| 1: de-essing | "DeBess" (unnumbered successor to "DeEss") | 5: Intense, Sharp, Depth, Filter, Sense | Sibilance Control / `sibilance_control` | The author recommends its longer adjustable detection window and improved rejection of non-sibilant material. |
| 2: console channel | "Console9Channel" (9) | 2: Pan, Fader | Console Encode / `console_encode` | Latest numbered standalone summing encoder, without a whole channel-strip processing chain. |
| 2: console buss | "Console9Buss" (9) | 2: Pan, Fader | Console Decode / `console_decode` | The matching decoder is required after the encoded channels sum. |
| 2: clipper | "ADClip9" (9) | 5: Boost, Match, Noise, Ceiling, Mode | Peak Clipper / `peak_clipper` | Latest line, with level matching and a clipped-signal audition mode. |
| 2: dither | "TPDFDither" (unnumbered, two-control Redux source) | 2: Quant, DeRez | Output Dither / `output_dither` | A general output word-length reducer; the author recommends this simple option when special noise shaping is unnecessary. |
| 2: slew | "Slew4" (4) | 1: Compres | Treble Tamer / `treble_tamer` | Latest dedicated brightness compression, without the tape stage. |
| 3: hall | "kGuitarHall2" (2) | 6: Regen, Derez, Filter, EarlyRF, Positin, Dry/Wet | Small Hall / `small_hall` | A hall with early-reflection positioning, rather than an ambient infinite-space substitute. |
| 3: plate | "kPlate140" (unnumbered model 140) | 5: Input, Regen, DeRez, Predlay, Wetness | Large Plate / `large_plate` | The author's next-generation large plate; one plate character in the first set. |

The named sections of [Johnson's notes at the pin](https://github.com/airwindows/airwindows/blob/7fd5f5cae262a41e6dd7d9a83d60c46e84d3092a/Airwindopedia.txt)
support these version choices: "ToTape9", "Density3", "DeBess", "Console9Channel", "ADClip9",
"Slew4", "kGuitarHall2", "kPlate140" and "TPDFDither". The last includes its Redux update;
its earlier no-controls description is historical. "kPlate140" and "kPlate240" are different
plate models, not revisions 140 and 240. The former's notes explicitly describe the larger, more
prominent character. The later stereo modulation variants are different designs, not numbered
updates to "Chorus": the first set deliberately keeps the single-tap and four-tap designs with
independent speed, range and mix controls.

Deferred, with source details and reasons in the survey: "Vibrato", "Flutter2", "PhaseNudge",
"StereoChorus", "StereoEnsemble", "Capacitor2", "PurestGain", "BitShiftGain", "PurestDrive",
"NotJustAnotherDither", "Galactic3", "Verbity2" and "kPlate240". Historical candidates
"Console7Channel"/"Console7Buss", "ToTape8", "Density2", "DeEss", "ADClip8", "Slew3",
"Galactic", "Galactic2", "Verbity" and "kPlateA" are surveyed but not scheduled alongside their
successors. In particular, "PhaseNudge" is an allpass phase rotator with mix, not a complete
modulated multi-stage phaser. A dedicated phaser and feedback flanger remain later product work;
this ADR does not claim those gaps are fully closed.

### Relationship to retirement and routing

New inserts use new ids. No removed id is revived, and no old slot is silently substituted.
"Density3" supplies the saturation replacement; the hall and plate supply the reverb replacements.
Their implementation slices retire the corresponding stage 2 devices using ADR 0040's load report,
dropped parameter targets, emptied slot and modified-session rule, with corpus coverage. The
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
  access, and a C ABI per family. Compile the original processing functions, not a copied second
  implementation. Isolate duplicate entry points, global enum names and header guards per translation
  unit using wrapper/build symbol isolation. Record the mechanism; it must not change arithmetic.
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
  the same definition in Rust. No tone correction, coefficient change, reordered accumulation,
  removed noise, added oversampling or general cleanup. A desired change to defined DSP reopens
  this ADR instead of hiding in an oracle patch.

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
| `transit-tape-color` | Tape Color |
| `transit-wave-shaper` | Wave Saturator |
| `transit-character-dynamics` | Character Compressor, Sibilance Control |
| `transit-console-summing` | Console Encode, Console Decode |
| `transit-output-tools` | Peak Clipper, Output Dither, Treble Tamer |
| `transit-space-reverb` | Small Hall, Large Plate |

Production crate names, Rust identifiers, stable ids, parameter ids, preset ids and device titles
are descriptive, without upstream brand-like effect names. Original names remain in unchanged
vendor files and provenance/credit text. The explicitly named dev-only oracle is the catalog-name
exception, not a naming pattern for production crates.

The shipped app is Rust. Keep the original operation order, literals, casts, precision, integer
wrapping where defined, state update order and per-sample loop. The transit f32 audio path ports
**`processReplacing`**, including its output dither and double intermediates; it must not call the
undithered double path and cast at the end. `processDoubleReplacing` remains vendored for reference;
claiming a future f64 path requires separate f64 nulls. No SIMD regrouping, generic smoother,
oversampling, resampling, mix-law correction or latency removal is added. Preserve any rate
division, interpolation or filtering already inside the original.

Keep block-entry work at block entry. In particular, the console pair interpolates `panA/B` and
`inTrimA/B` using remaining frames divided by the original call length, including its constructor
transient. The adapter passes each transit process span and parameter event boundary through
unchanged. Null each such schedule against C++ with identical boundaries; do not reblock into
single samples or assert equality across different block schedules where the original differs.
The scheduling oracle compares serial and worker runs of the **same** schedule. Help must disclose
that changing the audio block size can change these ramps. A later demand for block-independent
automation is an explicit algorithm change and reopens this decision.

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
Rename abbreviations in the UI (Compression, High-pass, Flutter Speed, Head Bump, Early Reflections,
Position, Predelay); the survey keeps the exact source names.

Concrete exceptions to the percentage rule:

- Tape Head Frequency displays `25 + 175*h*h` Hz, 25–200 Hz, as the source does. Input, Output,
  Tilt and Bias retain normalized travel; the help describes their nonlinear gain and bipolar
  behavior rather than claiming a calibrated tape-machine measurement.
- Saturation Amount displays `5*a - 1`, range -1 to 4, dimensionless, following the source
  display (the processor's internal expression differs). Its high-pass remains a percentage.
- Console Pan displays -100% left to +100% right (`200*a - 100`); Fader remains 0–100% travel,
  since its shaped gain is not an ordinary linear level control.
- Clipper Boost and Match display 0–18 dB (`18*a`, `18*b`). Noise and Ceiling remain percentages
  of their original threshold controls, not dB ceilings. Mode is a list: Boost, Match, Clipped
  Signal; normalized anchors 0, 0.5, 1 select the source's three modes, including "ClipOnly".
- Dither Quant is the list 16-bit / 24-bit (normalized 0 / 1); DeRez remains 0–100% travel and
  is not a linear bit-depth readout. The original quantizer's offset and floor operation stay.
- De-esser Sense becomes Listen: Processed / Removed (0 / 1). The DSP uses `E > 0.49999` for
  input minus processed audio; its "Vox"/"Ess" label strings are not a reliable description.
  Sharp remains continuous: integer window length changes coexist with continuous speed scaling.
- Hall Position is a list of positions 1–28: the source uses `int(E*27)`. Use interval midpoints
  for indices 0–26 and 1 for index 27, with tested boundary mappings. Its Derez is one continuous
  control with labeled halves (stepped left, full at center, continuous right); do not quantize
  it to a list because the right half is continuous.

Each slice provides `docs/help/<descriptive-id>.toml`, registered help ids and hovered widgets,
a row in `docs/help/README.md`, and device documentation. Every control has its range, default,
meaning and dependencies. Each effect's help describes its own sample-rate behavior from the
survey: sample-based delays and modulation, rate-scaled detectors, discrete internal-rate changes,
or the absence of compensation. No promise of identical sound at different rates. Explain the
plate's 50% full-dry-plus-full-wet law and the hall's resolution/position changes. Dither belongs
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
source names and revisions, link notices, describe any recorded deterministic patches, and call
out ADR 0040 removals as breaking changes. Claim bit-exactness only for effects that passed the
gate, and name the pinned, deterministically patched desktop reference and matched schedules.

## Gate

These are implementation gates, not claims that this documentation change has run DSP tests.

1. **Reference first.** Per effect and rate, render the same input/parameter schedule twice with
   fresh instances in one process, then in a second process. Compare every output bit in-process
   and complete render/state digests across processes. Required floor: zero differing samples,
   peak and RMS residual `-inf` dBFS. Repeat with different initialization memory patterns to
   expose unwritten reads. A failing floor blocks all port claims; identify and define the
   uninitialized/undefined behavior before proceeding. Compare the port against that original
   C++ reference on the same runner, not a Rust-generated golden.
2. **Rates.** Every effect runs the sweep and floor at 44.1, 48, 88.2, 96, 176.4, 192, 352.8,
   384, **705.6 and 768 kHz**. The last class is required now: `crates/time/src/lib.rs`
   accepts 8,000–768,000 Hz, not just its eight `COMMON` rates. Include 8, 11.025, 16, 22.05,
   24 and 32 kHz, and the 1 kHz lower edge admitted by the export command and DSP sanitizer.
   Bounds audits cover the entire admitted rate interval; add probes immediately on either
   side of every integer-divider/clamp transition and any newly supported rate. Do not silently
   clamp an admitted high rate to 192 kHz or add conversion to make the null pass. A source
   unsafe at an admitted rate needs a recorded definition of its UB, or its slice remains blocked.
3. **Sweep and scenarios.** For each continuous parameter use 0, 0.25, 0.5, 0.75, 1 around
   source defaults, both endpoints of interactions, all list choices and adjacent f32 values
   around branch boundaries. Include all-controls-low/high, paired drive/output and rate/depth
   extremes, ramps and jumps. Use identical seeded stereo input: silence, subnormals, impulses,
   unequal channels, tones, broadband noise, hot finite samples and quiet tails. Exercise each
   mode's wet/nontrivial path after its delays fill; enumerate intentional silence separately.
   Compare f32 `to_bits()` including signed zero, and both RNG words; no epsilon, ULP allowance
   or noise-floor tolerance. Nonfinite output on valid finite input is a blocker, not an exact
   NaN match. Test reset versus fresh construction, interleaved instances, bypass/unbypass and
   rates reconfigured through prepare. Use process sizes 1, 2, 7, 32, 64, 127, 256, 1024 and an
   irregular sequence, with the same schedule on both sides. Preserve upstream block dependence.
4. **Long floor and null.** At every listed rate render at least 60 seconds of sample time per
   effect, with parameter motion, an impulse/noise excitation and at least 10 seconds of silence.
   Run this for oracle self-comparison and port comparison. Extend slowest-delay/resolution cases
   until all enabled paths have emitted processed input and wrapped their longest active memory
   at least twice; 60 seconds of an unfilled wet buffer is not evidence. Record frame counts and
   final state. No assertion depends on elapsed wall time or scheduling luck.
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
   initialization check or scenario. Set the variable for the release workspace test. Measure
   build and test cost before changing this policy, not by dropping a slow test silently.
7. **Integration.** Adapter tests cover normalized/plain mappings, presets, save/load/reset,
   invalid parameters and input containment, no allocations on process/reset, reported latency,
   routing, help and scheduling fingerprints. If shared DSP primitives change, run the **full
   workspace test**, not only the new family, with full oracles; ordinary integration also
   follows CONTRIBUTING.md. Run `cargo xtask layers`, `assets`, `timing`, `help` and `file-size`;
   generate help with `cargo xtask help --write` before checking it. Refutation review under
   ADR 0009 includes each deterministic patch, FFI safety invariant and a deliberate perturbation
   that makes each new null suite fail. Fidelity does not replace the audio-thread safety gate.

## Consequences

The stock collection fills distinct mixing jobs with small interfaces and a reproducible reference.
Users get the author's algorithms and quirks, including sample-rate coloration, seeded noise,
nonlinear mix laws and block-length-dependent ramps. Gain staging and console placement need help
text; a famous algorithm name is not a substitute for it.

Maintainers carry C++ only in development, a compiler-dependent numerical reference, deterministic
patches and three-platform test cost. A bit-exact claim is bounded by the pinned reference, runner
and tested schedules; it is not a claim about every released binary or every imaginable input.
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

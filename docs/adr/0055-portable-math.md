<!-- transit-index
title: Portable math, so an offline render is bit-identical on every operating system and CPU family: one seam crate `transit-math` (layer 0, `no_std`) that every rendered crate calls for every transcendental function, backed in v1 by the pinned pure-Rust `libm` crate with each function's outputs pinned by a table test; the IEEE default floating-point environment on every rendering thread (no hardware flush-to-zero, which Rust does not allow), checked and restored at the audio callback and after hosted plug-in calls; software denormal flushing stays; an `xtask math` check over the rendered crates; cross-platform golden renders on all three CI runners; existing renders move by rounding-level amounts once, announced as a Change
-->
# ADR 0055: Portable math

- **Status:** proposed 2026-11-12
- **Date:** 2026-11-12
- **Implementation:** not started. Slices in the Slices section. Issues #437 and #277 (the decision), #475 (the floating-point mode and the software denormal flush).
- **Reopen when:** a `libm` release changes a pinned output table and the change is wanted; a pure-Rust correctly rounded library covers the v1 function list; a per-sample device misses the cost budget of decision 6 even with a fast variant; Rust gains a sound way to run code under flush-to-zero; a hosted plug-in is measured slowing down on denormals because transit sets no mode; a target without IEEE binary32 and binary64 semantics is adopted (ADR 0037); or the maintainer extends the promise to real-time playback.

## Context

The maintainer ruled on 2026-11-11 (#437 and #277): a bounce should be bit-identical across operating systems, through portable math, designed in one decision record after v0.7.0. Two accepted records already route their transcendental functions through a crate-local `math` module and point at this one: [ADR 0053](0053-multiband-dynamics.md) decision 7 (f32 `log2` and `exp2` per sample, `tan`, `exp2` and `exp` at control rate) and [ADR 0054](0054-parametric-eq.md) decision 11 (f64 `exp2`, `log2`, `sin` and `cos` only, none per sample, its K-weighting table as pinned literals, its own FFT because `rustfft` dispatches SIMD code at run time). Both say: when the shared portable helpers exist, the `math` module forwards to them and nothing else changes.

**How it went wrong.** Two landing chains stopped on 2026-11-10 because a hash pinned on Windows differed on Linux (#437: a Sampler test fixture built with `f32::sin`). Earlier, on 2026-11-01, `crates/dsp/tests/lfo_sync_golden.rs` (168 hashes, #253) and `crates/mix/tests/tempo_goldens.rs` (6 hashes, #254) failed under glibc with every hash different (#277). Both files, and `crates/dsp/tests/note_label_golden.rs` (ADR 0051), are now compiled on Windows only (`#![cfg(windows)]` or per-item `#[cfg(windows)]`), and their headers say why: "the rendered path calls libm, whose last bit differs between MSVC's CRT, glibc and Apple's libm". The standing test rule since then: a pinned hash covers only IEEE-exact values; anything that calls `sin`, `exp2`, `tan`, `pow` or `powf` with a non-trivial argument is compared within one process.

**What Rust promises** (read in the standard library source of the toolchain in use, rustc 1.99.1, `library/std/src/num/f32.rs` and `library/core/src/primitive_docs.rs`):

- `+`, `-`, `*`, `/`, `sqrt`, `mul_add`, `abs`, `floor`, `round`, `trunc` and the conversions are exact IEEE operations (`sqrt`: "guaranteed to be the rounded infinite-precision result"; `mul_add`: a single rounding, "guaranteed not to change"). Rust never contracts `a * b + c` into a fused multiply-add and never reassociates.
- `powi`, `powf`, `exp`, `exp2`, `ln`, `log`, `log2`, `log10`, `cbrt`, `hypot`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `sin_cos`, `exp_m1`, `ln_1p`, `sinh`, `cosh`, `tanh` and the inverse hyperbolics carry "Unspecified precision: the precision of this function is non-deterministic. This means it varies by platform, Rust version, and can even differ within the same execution from one invocation to the next." `powi` is on that list.
- NaN results have a non-deterministic sign and payload.
- Changing the floating-point environment is undefined behavior. The x86 `_mm_setcsr` documentation (`library/stdarch/crates/core_arch/src/x86/sse.rs`): modifying the rounding mode or the denormals-are-zero flag "leads to **immediate Undefined Behavior**: Rust assumes that these are always in their default state and will optimize accordingly", even when restored before any floating-point operation; the getter and setter are deprecated since 1.75. The ported firmware crates already rely on this: `crates/texture/src/float.rs` says "changing that mode under Rust is undefined behavior. So the port reproduces flush-to-zero explicitly".

So in transit's own Rust code the only sources of cross-platform difference are the functions on the second list, NaN bits, and whatever order the code itself chooses. The rest of this Context checks each candidate source in the repository at `7244e915`.

### Inventory: platform math on the rendered path

Counted as call sites of the functions on Rust's "unspecified precision" list (direct method calls and the two crate `math` modules' forwards), in production code only: test files and inline `#[cfg(test)]` modules excluded, the `math.rs` modules themselves excluded. Wrappers multiply the reach: `db_to_gain` (`powf`) has 49 call sites in 21 files of `transit-dsp`, `transit-mix`, `transit-playback`, `transit-sampler-core` and `transit-instrument-core`, `gain_to_db` (`log10`) 11 in 6, `time_coef` (`exp`) 10 in 3.

| Crate | Sites | Files | Per sample (hot) | Control rate | When set, per block or at setup |
|---|---|---|---|---|---|
| `transit-dsp` | 76 | 24 | Compressor/Limiter and Expander/Gate: `gain_to_db` (`log10`) and `db_to_gain` (`powf`) per frame (`plugins/dynamics.rs`); Saturator: `tanh` per channel and frame plus two per frame (`plugins/harmonic.rs`); Room and Plate Reverb: `sin_cos` per frame (`plugins/reverb.rs`); signal generator `sin` (`osc.rs`); destructive fades per sample (`offline.rs` `FadeShape::gain`) | Filter insert: `exp2` every 16 samples, `tan` in the SVF design (`plugins/filter.rs`, through `transit-instrument-core`) | pan law `cos`, `sin`, `powf` and the surround panner's `atan2`, `sin`, `cos` (`pan.rs`); biquad designs `sin_cos`, `powf`, `tan` (`biquad.rs`, the inherited EQs); parameter tapers `ln`, `powf` (`lib.rs`); sinc tables `sin` (`offline.rs`, `plugins/resample.rs`); `time_coef` and `Smoother` `exp`, `powi` (`util.rs`); K-weighting `tan`, `powf` (`meter.rs`); `rustfft` in `time_stretch`, `pitch_shift` and `detect_transients` (`offline.rs`) |
| `transit-instrument-core` | 13 | 5 | none | LFO `sinf` (`lfo.rs`), matrix curves `expf`, `cosf`, `sinf`, `lnf` (`matrix.rs`), shaper `exp2f` (`shaper.rs`), SVF `tanf` (`svf.rs`), all on the voice's control grid | `powf` (Q map), `expf` and `lnf` (ADSR rates), `log2f` (cutoff targets) |
| `transit-sampler-core` | 3 | 2 | none | f64 `pow(2, semis/12)` for the pitch step on the 12-frame grid (`voice.rs`) | `powf` for the zone level (`patch.rs`); `pow` at voice start |
| `transit-mix` | 2 (direct) | 1 | none | none | through `db_to_gain` and `pan::gains`: fader, trim, sends, automation ramps (`automation.rs` `lerp_gain`), surround matrix (`matrix.rs`); click synthesis `exp`, `sin` (`click.rs`) |
| `transit-model` | 9 | 3 | none | clip fades `sin`, `cos`, `exp2` every 32 samples (`clip.rs`, evaluated in `transit-mix` `clips.rs`) | clip gain `powf` (`clip.rs`); rack macro curves `powf` (`rack.rs`); fader taper `powf` (`mixer.rs`) |
| `transit-audio-io` | 3 | 3 | none | none | Opus writer's sinc table `sin` (`resample.rs`); Opus read gain `powf` (`opus.rs`); AIFF rate `powi` (`aiff.rs`) |
| `transit-engine` | 12 | 4 | none | none | AudioSuite pitch ratio `powf` (`cmd/audiosuite.rs`); MIDI curve tools `powf`, `cos` (`cmd/event_more.rs`); demo sessions and DAWproject import (not rendered math) |
| `transit-macro-osc` | 4 | 3 | the six-operator engine's restart crossfade `cos` per sample over 48 frames (`engine2/six_op_engine.rs`) | none | the `Exact` modes of `utils/filter.rs` (`tan`, `sin`) and `utils/cosine_oscillator.rs` (`cos`), unused in production |
| `transit-modal-voice` | 1 | 1 | none | none | `sin` in `set_f_q_exact`, called with constant arguments at setup (`fm_pair.rs`) |
| `transit-resonator` | 3 | 2 | none | `powf` twice per block (`fm_voice.rs`) | the `F::powf` wrapper (`float.rs`) |
| `transit-texture`, `transit-granular`, `transit-modulator`, `transit-random-sequencer` | 1 each | 1 each | none | none | only the unused `F::powf` wrapper in `float.rs`; the rest is tables, polynomials and `sqrt` |
| `transit-time`, `transit-rt` | 0 | 0 | | | |
| `transit-playback` | 8 | 5 | none on a rendered path | none | monitor, preview and metronome gains, latency probes: live only |

The ported firmware crates are portable already, apart from five call sites: they carry the firmware's own lookup tables and approximations, and their flush-to-zero is software (`F`, `utils::ftz`). transit's own devices and the mixer are where the platform runtime is called, on every render: a fader that is not at 0 dB, any pan law other than 0 dB at center, any automation ramp, any clip fade and any resampled source reach `powf`, `cos`, `sin` or `exp2` today.

### The other candidate sources, checked

| Source | What the code does today | Verdict |
|---|---|---|
| Fused multiply-add contraction | Rust never contracts. No `mul_add` in any production crate (grep over `crates/`, the oracle excepted). No `-C target-cpu` or `target-feature` in `.cargo/config.toml`, `Cargo.toml` profiles or the CI workflows. The release profile is `opt-level = 3`, thin LTO. | Not a source in Rust. It is one in C and C++ built without `-ffp-contract=off`: the oracle sets it (`crates/macro-osc-oracle/build.rs`), the libopus build does not (`crates/opus/build.rs`). |
| x87 versus SSE | Every shipped target is x86_64 (SSE2 baseline) or aarch64 (`.github/workflows/release.yml`, `packaging/`). The `libm` crate selects x87 code only on 32-bit x86 without SSE2 (`src/math/arch/mod.rs` of `libm` 0.2.16). | Not a source. 32-bit x86 is not a target. |
| Denormals, flush-to-zero, denormals-are-zero | No product code sets or reads a floating-point mode (grep for `mxcsr`, `_mm_setcsr`, `FPCR`, `asm!`: none outside the oracle's C++ `cpp/ftz_scope.h`). transit's own devices flush their states in software (`util::flush` in `transit-instrument-core`; ADR 0053 decision 8). The ported crates flush every arithmetic result in software (`F` in `float.rs`, `utils::ftz`), reproducing the oracle, which runs the C++ with flush-to-zero inside each call only (`crates/macro-osc-oracle/README.md`). #475 measured no denormal slowdown in Texture for that reason. | Bit-identical today, because every thread runs in the default mode and the flushing is arithmetic. Decision 4 keeps it that way. |
| SIMD chosen at run time | No `is_x86_feature_detected`, `target_feature` or `std::arch` in product code. `rustfft` 6.4.1 (`Cargo.lock`) is used by `transit-dsp` in `offline.rs` (`time_stretch`, `pitch_shift`, `detect_transients`), `pitch_detect.rs` and `spectrum.rs`; its planner picks AVX, SSE or NEON code at run time (its documentation, as ADR 0054 decision 11 cites it; not verified here). libopus is built without run-time CPU detection (`crates/opus/build.rs`). | A source for the offline time and pitch tools and for transient detection, whose markers become session data (ADR 0041 Slices). The spectrum analyzer is display only. |
| Integer and float conversions | Rust's `as` saturates and maps NaN to 0. Export quantizes with `round`, `clamp` and `as i32` (`crates/audio-io/src/pcm.rs`). The ported crates reproduce x86 truncating conversions explicitly (`float.rs`). | Not a source. |
| NaN payloads | `contain_failure` (`crates/mix/src/strip.rs`) silences a block holding NaN or infinity; the PCM and float writers map NaN to 0 (`pcm.rs`). | Not a source in a written file. |
| Summation order and thread count | Offline renders run strips on Rayon's global pool when there are enough of them; "the strips are independent and are summed afterwards in a fixed order" (`run_strips`, `crates/mix/src/render_pool.rs`). The serial tail routes phase-1 strips in plan order (`crates/mix/src/tail.rs`); the graph tail commits slot by slot in task order (`crates/mix/src/strip_pool/tail.rs`); the matrix sums sources in index order (`matrix.rs`). | Deterministic by reading; the golden test of decision 5 proves it across thread counts. |
| Real-time timing | A strip that overruns its deadline is silent for the block (ADR 0029; `tail.rs`: "An overrun, a faulted or a suspect strip is silence for the block"). The device sets the block size. | Real-time output depends on timing and the device. |
| Block size | Offline bounce, freeze and AudioSuite all render in 1024-frame blocks (`crates/engine/src/io/bounce.rs`, `render_track_pre_fader_timed` in `crates/mix/src/offline.rs`, `crates/engine/src/cmd/audiosuite.rs`). Built-ins get parameter values at block starts (`apply_params`, `crates/mix/src/automation.rs`), so automation is block-quantized. | Output at one block size is reproducible; across block sizes it is not, whenever automation moves. |
| `f32` and `f64` | Both are IEEE binary32 and binary64 on every target. On `thumbv7em-none-eabihf` the FPU is single precision only and `f64` runs in software (compiler-builtins). | Not a source, given correctly rounded basic operations (the software `f64` is assumed correctly rounded; not verified). |
| Sample-rate conversion | transit's own windowed sinc (`transit_dsp::offline::resample`, the streaming copy in `crates/audio-io/src/resample.rs`), used on import (`add_source`, `at_rate` in `crates/engine/src/io.rs`) and for the Opus writer. Its tables call `sin`. | A source through `sin` only; portable once routed. |
| Decoders and encoders | Native WAV, AIFF and FLAC readers and writers in `transit-audio-io`; `symphonia` 0.5.5 for MP3, AAC, Vorbis, ALAC, ADPCM and others (`crates/audio-io/src/symph.rs`); libopus 1.6.1 from vendored C (`transit-opus`). | Integer PCM and lossless decoding is exact. `symphonia`'s lossy decoders are pure Rust whose math is not verified here. libopus is C compiled with each platform's default contraction and its C runtime. |
| Random numbers | Fixed seeds and integer generators: the macro oscillator's voices start from `Random::DEFAULT_SEED + i` (`crates/dsp/src/plugins/macro_osc.rs`); dither is xorshift64* (`pcm.rs`). No `rand`, `fastrand` or OS entropy in a rendered crate. | Not a source. |
| Time and addresses | `Instant::now` appears in rendered crates only for deadlines, state-capture timeouts and logs (`strip_pool`, `texture/freeze_state.rs`); `HashMap` is used for lookups in plan compilation (`crates/mix/src/compile.rs`), not for summation order. | Not a source offline; the golden test covers any iteration order that would leak. |

**Embedded and arm64 note.** [ADR 0037](0037-adaptable-core-small-hardware.md) says floating-point defaults differ on arm64 (flush-to-zero and fused multiply-add). That holds for C and C++ builds (and for the oracle's AArch64 `FPCR.FZ`, which also zeroes subnormal inputs), not for Rust code, which runs in the default mode with no contraction on every target. The `no_std` builds of `transit-instrument-core` and `transit-sampler-core` already call `libm`; the desktop builds call the platform. `docs/contributing/evidence/gates.md` records that the bare-metal check "does not prove ... numerical equivalence of the `libm` path".

## Decision

### 1. The promise

**Bit-identical:** for the same session, the same source files and the same build of transit (one `Cargo.lock`), these outputs are identical in every bit on every supported platform:

- offline bounce and export of the main mix or a bus (`file.bounce_mix` offline, `crates/engine/src/io/bounce.rs`) to WAV, AIFF and FLAC at every bit depth, with or without dither, and the float samples handed to any encoder;
- freeze (`render_track_pre_fader_timed`), commit and consolidate (`render_clips`), and AudioSuite processing of built-in devices;
- the offline edits of `transit_dsp::offline` (fades, gain, normalize, resample, time stretch, pitch shift) and the sample-rate conversion on import;
- analyses whose results become session data: transient and beat-grid markers (`detect_transients`) and pitch detection where it writes notes;
- every built-in device and instrument inside those renders, the Sampler included, and the mixer (faders, pans, sends, automation ramps, clip fades and gains, rack macro curves, the surround panner).

**Platforms.** Proven on every CI run: x86_64 Windows (MSVC), x86_64 Linux (glibc, ubuntu-22.04), aarch64 macOS (`macos-latest`). Claimed by construction and not tested: x86_64 macOS and FreeBSD, aarch64 Linux (the Pi 5 run of ADR 0037 adds the golden test of decision 5 when it runs), and the Tier B crates on `thumbv7em-none-eabihf` (compile only).

**Toolchain.** Any stable rustc at or above the workspace's `rust-version`, any cargo profile (dev, test, release, LTO), with or without extra target features: Rust's arithmetic does not change with them and `libm`'s architecture paths are exact operations (`sqrt`, `fma`, `rint`). Fast-math style operations (`fadd_fast` and the other `*_fast` intrinsics, the `algebraic_*` methods) are forbidden in rendered crates (decision 5).

**Not promised:**

- Different block sizes. Automation reaches built-ins at block starts, so a render at 512 differs from one at 1024 when automation moves. Every offline path keeps its fixed 1024.
- Real-time playback and anything recorded from it: overruns silence strips, the device chooses the block size, live input is live. The External Instrument's real-time render ([ADR 0027](0027-external-instrument-real-time-render.md)) is real-time.
- Hosted plug-ins (VST3, CLAP, AU) and any render that contains one. transit does not control their math or their floating-point mode.
- The bytes of an Opus file: libopus is C built with each platform's default contraction and C runtime. The float input to the encoder is promised.
- Audio decoded by a lossy decoder (`symphonia`'s MP3, AAC and Vorbis; libopus): not verified. A source in such a format joins the promise when a slice proves its decoder by the golden test.
- Different sample rates, a different `Cargo.lock`, a different transit version.
- Values computed for display only: meters, the analyzer, response curves, readouts.
- NaN bit patterns (never written, see Context).

### 2. One math seam: `transit-math`

**2.1 The crate.** A new crate `transit-math` (`crates/math`), the only place a rendered crate may get a transcendental function.

- `#![no_std]` always (no `std` feature: it needs none), `#![forbid(unsafe_code)]`, no allocation.
- Layer 0 in `xtask`'s `LAYERS`, in `STANDALONE`, and in the bare-metal compile job from its first slice.
- One dependency: `libm`, pinned exactly (`libm = "=0.2.16"` in the workspace) so an upgrade is a deliberate change. `libm` is MIT licensed, `no_std`, maintained in the Rust project's compiler-builtins repository, and its single-precision functions are FreeBSD msun ports (each file's origin line).
- **Layer rule amendment.** `STANDALONE` crates may depend on `transit-math`: `xtask` gains a `FOUNDATION` list holding only `transit-math`, which the standalone check ignores. Without it `transit-instrument-core`, `transit-macro-osc`, `transit-resonator`, `transit-granular`, `transit-texture`, `transit-modulator` and `transit-random-sequencer` would fail `cargo xtask layers`.

**2.2 The surface.** Two modules, `transit_math::f32` and `transit_math::f64`, of free functions with the platform names. v1, from the inventory and ADRs 0053 and 0054:

| Width | Transcendental (backed by `libm`) | Exact (re-exported for `no_std` callers) |
|---|---|---|
| `f32` | `sin`, `cos`, `tan`, `exp`, `exp2`, `ln`, `log2`, `log10`, `powf`, `tanh`, `atan2` | `sqrt`, `floor`, `ceil`, `round`, `trunc`, `mul_add` |
| `f64` | `sin`, `cos`, `tan`, `exp`, `exp2`, `ln`, `log2`, `log10`, `pow` | `sqrt`, `floor`, `ceil`, `round`, `trunc`, `mul_add` |

- `sin_cos(x)` is `(sin(x), cos(x))`, two calls, so it can never disagree with them.
- `powi(x, n)` is transit's own: binary exponentiation in a stated order of plain multiplications, exact by construction and identical everywhere (Rust's `powi` is on the unspecified list).
- A `consts` module holds the conversion literals the devices share, written as literals and never computed at run time: `LOG2_TO_DB` (20·log10(2), the `6.020_6` of ADR 0053), `DB_TO_LOG2` (its reciprocal), `LOG2_E`, `LOG2_10`.
- `db_to_gain` and `gain_to_db` move onto `exp2` and `log2` with those literals (`transit-dsp` keeps the names and the -144 dB floor). It drops `powf` and `log10` from the widest-reaching helper and matches ADRs 0053 and 0054.

**2.3 Tests in the seam.** Each function carries:

- **A pinned output table**: the bits of its outputs over a fixed argument set (4,096 points spread over the useful domain, plus zeros of both signs, the smallest normal, subnormals, the largest finite values, infinities, and for `sin`, `cos` and `tan` arguments up to 1e6 where range reduction works hardest), hashed with FNV-1a and pinned. These outputs come from plain Rust code and are IEEE-exact by construction, so the pin holds on every platform; a change of `libm` that moves one bit fails here first.
- **An accuracy bound** against a higher-precision reference: each `f32` function against its `f64` counterpart rounded to `f32`, at most 1 ulp over the same set; each `f64` function against a table of reference values generated once with an arbitrary-precision tool, committed as literals with the generating script beside them (the method of ADR 0054's weighting table), at most 1 ulp. The measured worst cases are recorded in the slice's result.
- **Identities** that must hold exactly: `exp2` of an integer, `log2` of a power of two, `sin(0) = 0`, `cos(0) = 1`, `powf(x, 0) = 1`, `powi` against repeated multiplication.

**2.4 Adding a function.** A pull request that adds a `libm`-backed function with its table, bound and identity tests, and names its caller and call rate. No amendment needed. Anything that is not a direct `libm` call needs an amendment of this record: a fast variant (2.5), or replacing the backing of an existing function.

**2.5 Fast variants (option b, only when measured).** A device over its budget (decision 6) may get `transit_math::fast::<name>`: transit's own polynomial or table with a stated maximum error in ulps over a stated domain, a test of that bound against the `f64` reference on a dense grid, and a pinned table. It is portable for the same reason (plain arithmetic in a fixed order). None in v1.

**2.6 The crate-local `math` modules.** `transit-instrument-core` and `transit-sampler-core` keep their `pub(crate) math` modules, and the new device crates of ADRs 0053 and 0054 keep theirs, as the list of what each crate uses; each forwards to `transit-math` on every build, `std` or not. The `cfg(feature = "std")` split in them goes away: desktop and bare metal then run the same code.

**2.7 FFT.** Rendered code gets one FFT, `transit_math::fft`: the radix-2 real FFT ADR 0054 specifies (power-of-two sizes, twiddles from the seam, a fixed arithmetic order). Whichever lands first, the equalizer's slice 1 or slice 4e here, writes it there; the other uses it. `time_stretch`, `pitch_shift`, `detect_transients` and `pitch_detect` move to it. `rustfft` stays for display (`spectrum.rs`, the equalizer's analyzer).

**2.8 The ported firmware crates and their oracles.** The ports are portable already except five call sites (Context): the six-operator restart crossfade `cos`, the unused `Exact` modes in `transit-macro-osc`, `set_f_q_exact`'s `sin` in `transit-modal-voice`, and `fm_voice`'s two `powf` in `transit-resonator`. They move to the seam. Their tables, polynomials and software flush-to-zero stay as they are. Where the vendored C++ that an oracle compares against calls a C math function on a compared path (`sinf` behind `set_f_q_exact`, and any future oracle such as the Airwindows one of ADR 0042), the oracle's force-included header (`cpp/compat.h`) redirects that function to a C-ABI shim over `transit-math` in the oracle crate (which already holds FFI under ADR 0013 and ADR 0033), so port and oracle call the same code and the nulls keep holding on every OS. That redirect lands in the same slice as the five call sites.

### 3. Why `libm` behind a seam (option a), and the others

| Option | Portable | Accuracy | Speed | Cost to adopt | Verdict |
|---|---|---|---|---|---|
| (a) `libm` crate behind `transit-math` | Yes: plain Rust, already the `no_std` backing | FreeBSD msun quality; `exp2f` documents "peak error < 0.501 ulp" (its source); others not documented per function; the seam's tests measure them | Not measured against MSVC's CRT, glibc or Apple's libm; slice 2 measures it | One small crate, mechanical call-site changes | **Chosen** |
| (b) transit's own polynomials and tables for everything | Yes | Whatever we prove, function by function | Can beat a general library where the domain is narrow | Error analysis and tests for every function, now | Only as measured fast variants (2.5) |
| (c) a correctly rounded library | Yes, and identical to any other correctly rounded implementation | Best possible | Not measured; not verified | CORE-MATH (C, MIT; not verified) would bring a C build with strict flags back into the render; no pure-Rust correctly rounded library covering the list was found (`libm` has ported a few CORE-MATH functions, `cbrt` among them, per its source) | Rejected for v1; reopen when a pure-Rust one covers the list |
| (d) platform math, restricted tests | No | Platform's | Platform's | None | Rejected by the maintainer's ruling |

`core` offers no transcendental functions on stable Rust (its `core_float_math` module is unstable and holds only `floor` to `cbrt`), so `no_std` code needs a library either way; with (a) the desktop uses the same one.

### 4. The floating-point environment

- **Every thread that runs transit's rendering code runs in the IEEE default environment:** round to nearest, no flush-to-zero, no denormals-are-zero, exceptions masked. That is the audio callback (a driver's thread: WASAPI, ASIO, JACK, CoreAudio), the render workers (`StripPool`), the Rayon global pool threads that offline renders use (`run_strips`), and the bounce, freeze and AudioSuite threads. transit's Rust code never changes the mode: Rust treats a change as undefined behavior (Context), and constant folding at compile time already assumes the default.
- **Bit-identity requires one mode everywhere; the default is the only mode Rust allows,** and it exists on every target, so targets without a flush-to-zero mode need nothing.
- **A guard, not a setting.** `transit-rt` gains `fp_env::ensure_default() -> bool`: it reads the x86 MXCSR or the AArch64 FPCR in a small `asm!` block (under ADR 0013), and if any mode bit differs from the default, writes the default back and returns false. It runs at each audio callback entry (a driver could hand over a thread in another mode) and after each hosted plug-in's process call (a plug-in that sets flush-to-zero and does not restore it). A false result increments an engine status counter ("floating-point mode restored", shown with the other render counters). The repair cannot undo what ran in the wrong mode; it stops it spreading into transit's devices for the rest of the block. Offline renders run without hosted plug-ins in the promise, so the guard matters mostly for live playback.
- **Hosted plug-ins get no mode from transit.** They manage their own denormals, as plug-in frameworks commonly do (not verified). Setting flush-to-zero around their calls would need an assembly trampoline whose whole call stays inside one `asm!` block, and host callbacks made by the plug-in during processing would run transit's Rust code in that mode. Not in v1 (open question 3).
- **Software denormal flushing stays.** transit's own state flushes (`util::flush`, ADR 0053 decision 8) are arithmetic and portable. The ported crates' per-operation flush reproduces the oracle (C++ run with flush-to-zero inside each call: x86 MXCSR.FTZ, AArch64 FPCR.FZ) and is what keeps their nulls in the subnormal range; the oracle's own mode is unchanged by this record. #475's option 1 (hardware flush-to-zero on the audio threads, dropping the software flush) is therefore rejected. A cheaper software scheme in the ports (flushing states instead of every operation) would change their bits against the oracle; it is a separate change with its own measurement and a re-measured oracle floor, not part of this record.
- **Bare metal.** Startup code for a Tier B target must leave flush-to-zero off (on Cortex-M, FPSCR.FZ clear), or the promise does not hold there.

### 5. Enforcement

**5.1 `cargo xtask math`.** A syntax-based check (`syn`, like `timing_loops.rs`), run in CI beside `cargo xtask timing`:

- **Scope.** A `RENDERED` list in `xtask`: `transit-dsp`, `transit-mix`, `transit-model`, `transit-instrument-core`, `transit-sampler-core`, `transit-macro-osc`, `transit-modal-voice`, `transit-resonator`, `transit-texture`, `transit-granular`, `transit-modulator`, `transit-random-sequencer`, `transit-audio-io`, `transit-engine`, and every new DSP crate (ADRs 0053 and 0054). Every crate in `LAYERS` at layers 1 to 4 must be either in `RENDERED` or in a short `NOT_RENDERED` list with its reason (the plug-in hosts; `transit-playback`, live only), so a new crate cannot be forgotten.
- **Flags.** A method call or path named on Rust's unspecified-precision list (`.sin(`, `f32::powf`, `.powi(` and the rest), any `libm::` path, any `rustfft::` path, and the fast-math operations (`*_fast`, `algebraic_*`), outside `transit-math`.
- **Not checked.** `tests/`, `benches/` and `examples/` directories and inline `#[cfg(test)]` modules (tests may use the platform for references and in-process comparisons). A line computing a display-only value in a rendered crate (a response curve for a panel, a meter readout in dB, the tuner) carries `// math: display <reason>` on it or the line above; the reason is mandatory, as for the timing markers.
- **Modes.** `report` prints the inventory and exits 0 (slice 1); `deny` fails on any finding (slice 6). In between, a per-crate list of converted crates is denied while the rest report, so a converted crate cannot regress.
- **Not clippy.** `disallowed_methods` was considered: CI runs clippy with `-D warnings`, so it has no report mode; the list would apply to the UI crates unless each allows it; and whether it matches inherent methods of primitive types was not verified. It can be added later as a second layer.

**5.2 Cross-platform golden renders.** `crates/mix/tests/portable_goldens.rs` (or `transit-engine`'s tests where a command is needed), compiled on every platform with no `cfg` guard:

- **Sessions**, built in code from integer noise and fixed MIDI phrases (the shared exact-noise helper proposed in #437), no files: (1) mixer: tracks at non-unity faders and pans under the default -3 dB law, sends to an aux, a gain automation ramp, one clip fade of each shape, a clip gain, a 44.1 kHz source in a 48 kHz session, a rack with a curved macro, the surround panner on a 5.1 bus; (2) one track per built-in instrument with non-default settings (the Sampler pitched, key-tracked, shaped and LFO-modulated; the macro oscillator over its engines; resonator, modal voice, texture synthesizer); (3) one track per built-in effect in the registry at non-default settings; (4) offline: a WAV and a FLAC export at 16 and 24 bits with dither, a freeze, AudioSuite runs, the offline edits and transient detection.
- **Rendered** the way the product renders them: `render_range` at 1024-frame blocks and the export writers. Each render runs twice in process, inside a one-thread and a four-thread Rayon pool (`ThreadPoolBuilder::install`), and the two must be equal.
- **Hashed** with FNV-1a 64 over the little-endian bytes (the existing goldens' hash), pinned as constants with the capturing commit in a comment.
- **Updating.** A deliberate DSP change that moves a hash states its audible reason in the pull request ([tests as evidence](../contributing/evidence/tests-as-evidence.md), rule 8). The author recaptures on any one platform (`UPDATE_GOLDEN=1`, as `plan_parity.rs`); the other two CI jobs passing unchanged is the proof that the new value is portable. A hash that differs between platforms is a defect in the seam's coverage, never a reason to pin per platform.

**5.3 The test rule after the seam.** Once slice 6 lands, a pinned hash may cover any render whose crates are all denied by `cargo xtask math`, proven by the three CI jobs. In-process comparison stays the rule for renders through hosted plug-ins, lossy decoders, Opus bytes, real-time playback or a crate still in report mode. Until slice 6, the standing rule (#437) holds unchanged. `docs/contributing/evidence/tests-as-evidence.md` records the new rule in slice 6.

### 6. Cost

**6.1 Speed.** Not measured. Per-sample use is the risk, and today it is: the Compressor/Limiter and Expander/Gate (`log10` and `powf` per frame, which become `log2` and `exp2`), the Saturator (`tanh` per channel and frame), the Room and Plate Reverb (`sin` and `cos` per frame), the six-operator restart crossfade (`cos` per sample for 48 frames), the destructive fades, and, when built, Multiband Dynamics (six `log2`/`exp2` per sample). Control-rate users (the Filter every 16 samples, Sampler and shaper voices every 12 frames, clip fades every 32) and per-block users (faders, pans, sends) are far below that. glibc's single-precision functions are fast table-driven implementations and may beat `libm`'s; MSVC's CRT is not characterized here. Slice 2 measures, in a recorded wall-clock bench (`crates/math/examples/bench.rs`, not a CI gate, like `plugin_bench`): each seam function against the platform on the implementer's Windows machine, Linux, and the Pi 5 when available; and `plugin_bench` before and after for each per-sample device in its conversion slice. **Budget:** a device's `plugin_bench` cost may rise by at most 25 % from its conversion; above that it gets a fast variant (2.5) or a restructuring, measured again.

**6.2 What existing sessions hear.** Every render that touches converted math changes in the last bits on every platform, Windows included (MSVC's CRT and glibc both give way to `libm`): differences around one ulp of f32, about 140 dB below the signal, inaudible. Alpha rules allow it (no shim). The release that carries the mixer conversion (slice 4b) says it under Changed: "Renders are now bit-identical on Windows, Linux and macOS; renders of existing sessions differ from earlier builds by inaudible rounding-level amounts."

**6.3 Tests that move.** Re-pinned once, at one math-only commit (slice 6), with the guards lifted so all three CI jobs prove them: `crates/dsp/tests/lfo_sync_golden.rs` (168), `crates/mix/tests/tempo_goldens.rs` (6), `crates/dsp/tests/note_label_golden.rs` (ADR 0051; its header says "never regenerate these from a new renderer": this commit is the one sanctioned exception, because the renderer did not change, only the math library, and the header is updated to say so). Each golden file under `crates/*/tests/golden` is checked: those compared with a tolerance (`plan_parity.rs`) stay; those compared exactly are recaptured at the same commit. Oracle nulls do not move except where 2.8's redirect is needed. In-process tests do not move.

### 7. ADRs 0053 and 0054 in the meantime

- **Parametric EQ slice 1** keeps its crate-local `math` module with exactly its four f64 functions, and backs them with `libm` (a direct dependency, pinned `=0.2.16`) on every build from the start, not with the platform. Its bits are then already those of the seam; when `transit-math` lands, the module forwards there and nothing moves. Its rule of no pinned hash over a render (its decision 19) stays until slice 6 here. Its FFT goes to `transit_math::fft` if slice 2 here has landed, otherwise into its crate and moves later as a pure move (2.7).
- **Multiband Dynamics** does the same with its f32 `log2`, `exp2`, `tan` and `exp`.
- Neither record needs amending: both say their module "forwards to them and nothing else changes".

## Slices

Each builds, passes the gates and lands on its own.

1. **Inventory and the check in report mode.** `cargo xtask math` with the `RENDERED` and `NOT_RENDERED` lists, the markers and the report; CI runs it. Display-only lines get their markers. Invisible.
2. **The seam.** `transit-math` with the v1 surface, `consts`, `powi`, the table, bound and identity tests, the bare-metal enrollment, `FOUNDATION` in `xtask`, the exact `libm` pin, the speed bench recorded. Invisible.
3. **The golden harness.** `portable_goldens.rs` with every session of 5.2, asserting only that the one-thread and four-thread renders agree; the hashes are printed, not pinned. Invisible.
4. **Conversion, one slice each, lowest risk first:**
   - 4a. `transit-instrument-core` and `transit-sampler-core`: their `math` modules forward to the seam on every build (the Sampler, the shaper, the Filter's SVF and LFO). Small and mechanical.
   - 4b. The mixer path: `db_to_gain` and `gain_to_db` on `exp2` and `log2`, `time_coef`, the smoothers, the pan law and the surround panner, `midi_to_hz`, the parameter tapers, clip fades and gain and rack curves in `transit-model`, the click, both sinc resamplers. Widest reach: every session. Release note of 6.2.
   - 4c. Built-in effects and instrument wrappers in `transit-dsp` (dynamics, Saturator, reverbs, biquad EQs, utility and the rest), with `plugin_bench` before and after for the per-sample ones. Includes the inherited effects ADR 0040 will retire (open question 6).
   - 4d. The ported crates' five call sites and the oracle redirect (2.8), with the oracle nulls run on Windows, Linux (GCC) and macOS before review.
   - 4e. The offline tools and analyses onto `transit_math::fft`; `transit-engine` and `transit-audio-io` call sites.
   Each conversion moves its crate to deny in `cargo xtask math`.
5. **The floating-point guard.** `fp_env::ensure_default` in `transit-rt` with its tests (a forced non-default mode is detected and restored, inside the asm block only), the calls at callback entry and after hosted plug-in calls, the status counter.
6. **Pin and deny.** One math-only commit re-pins 6.3's goldens and lifts their `cfg(windows)` guards; the hashes of `portable_goldens.rs` are pinned; `cargo xtask math` turns to deny everywhere; the test rule of 5.3 is recorded.

Order: 1, 2 and 3 first (any order between them after 2 for 3's sessions to use the helper); 4a to 4e in order; 5 any time after 2; 6 last.

## Consequences

- A bounce, export, freeze or offline edit made on one machine is the same file on any other, and a hash pinned on one CI runner holds on all three: goldens become a cross-platform regression tool again.
- Desktop and bare-metal builds run the same math, so the Tier B compile job's "numerical equivalence of the `libm` path" gap closes by construction (still not executed on the target).
- Every transcendental call in a rendered crate is visible in one crate's surface and one check's report.
- `libm` upgrades become DSP changes, caught by the seam's tables.
- Some per-sample devices may get slower until measured; the budget bounds it.
- transit still sets no flush-to-zero mode; denormal protection stays in each device's arithmetic.

## Alternatives rejected

- **Platform math with tests restricted to IEEE-exact values** (the status quo): contradicts the maintainer's ruling and leaves bounces platform-dependent.
- **Hardware flush-to-zero and denormals-are-zero on rendering threads** (#475 option 1): undefined behavior for Rust code; makes compile-time folded and run-time results disagree; AArch64's FZ also zeroes inputs while x86's FTZ does not, so the mode is not even one mode across CPU families.
- **Correctly rounded functions now** (option c): no pure-Rust library covering the list was found; a C library would bring compiler-flag discipline into the render.
- **Each crate its own approximations** (option b everywhere): duplicated error analysis; the seam keeps one implementation per function.
- **Vendoring `libm`'s source into `transit-math`:** more code to own for no portability gain while the exact pin and the table tests detect any change; reopen if upstream changes in a way transit does not want.
- **Per-platform pinned hashes:** turns a portability defect into data.
- **Clippy `disallowed_methods` as the only check:** no report mode under `-D warnings` and no per-crate scope without duplicated configuration (5.1).
- **Promising identical output across block sizes:** built-ins take automation at block starts; making every built-in sample-accurate is a separate decision.

## Open questions for the maintainer

1. **Real-time playback in the promise?** Recommendation: no. Overruns and device block sizes make it timing-dependent; the promise is for offline renders.
2. **Depend on `libm` with an exact pin, or vendor the functions?** Recommendation: depend with `=0.2.16`; vendor only if an upstream change is unwanted.
3. **Flush-to-zero around hosted plug-in calls?** Recommendation: no in v1; reopen on a measured slowdown.
4. **Make Opus bytes identical** (build libopus with `-ffp-contract=off` and `/fp:strict` and route its math)? Recommendation: no; the encoder's input is promised, a lossy file's bytes are not worth the C build discipline.
5. **CI runners for aarch64 Linux or x86_64 macOS?** Recommendation: no new runner; the Pi 5 benchmark run includes the golden test.
6. **Convert the inherited effects ADR 0040 will retire, or exempt them?** Recommendation: convert; each is a few call sites, and the promise then has no device exceptions.
7. **The 25 % cost budget** (6.1)? Recommendation: accept it as the trigger for a fast variant, revisited with the slice 2 numbers.

## Related

[ADR 0013](0013-unsafe-only-in-soundcraft-rt.md) (the guard's `asm!` lives in `transit-rt`), [ADR 0029](0029-strip-job-slots.md) (overruns), [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md) and [ADR 0042](0042-stock-effects-airwindows-ports.md) (oracles), [ADR 0037](0037-adaptable-core-small-hardware.md) (Tier B and the Pi 5), [ADR 0038](0038-inserts-read-the-tracks-note-stream.md) (the sample-clock rule), [ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md), [ADR 0048](0048-export-formats-opus-no-mp3.md) (Opus), [ADR 0051](0051-note-names-one-namer-and-a-middle-c-preference.md) (the note-label golden), [ADR 0053](0053-multiband-dynamics.md) and [ADR 0054](0054-parametric-eq.md) (the two math seams). Issues #437, #277, #475.

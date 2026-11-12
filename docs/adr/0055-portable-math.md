<!-- transit-index
title: Portable math, so an offline render is bit-identical on every operating system and CPU family: one seam crate `transit-math` (layer 0, `no_std`) that every rendered crate calls for every transcendental function, backed in v1 by the pinned pure-Rust `libm` crate with each function's outputs pinned by a table test; the IEEE default floating-point environment on every rendering thread (no hardware flush-to-zero, which Rust does not allow), checked and restored at the audio callback and after hosted plug-in calls; software denormal flushing stays; an `xtask math` check over the rendered crates; cross-platform golden renders on all three CI runners; existing renders move by rounding-level amounts once, announced as a Change
-->
# ADR 0055: Portable math

- **Status:** proposed 2026-11-12
- **Date:** 2026-11-12
- **Implementation:** not started. Slices in the Slices section. Issues #437 and #277 (the decision), #475 (the floating-point mode and the software denormal flush).
- **Reopen when:** (to be completed)

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

## Decision

(to be written)

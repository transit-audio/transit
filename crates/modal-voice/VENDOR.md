# Provenance

`src/` is transit's Rust port of Émilie Gillet's modal voice firmware, written for #198 slice 2
directly from her MIT C++ (ADR 0033, option (c)). No third-party Rust code is vendored. The step 0
check of slice 2 (2026-11-01) looked at the one Rust candidate, `mutated-elements` 0.3.1 (MIT,
Céline Alameda): a clean hand transcription of the firmware (no GPL text, nothing from VCV), but
not nullable and not the whole module (run-time `f32` tables where the firmware has float64
tables rounded once, rates rebuilt per host rate, the modal resonator only, a global `fastrand`
generator, a mutex in the render path), so it was not used. The port was never written from VCV
Rack's Audible Instruments (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (`elements/dsp/`, `elements/resources.cc`) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers it uses), both MIT, Émilie Gillet |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/elements/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `elements/dsp/part.{h,cc}` (`Part`, `PerformanceState`), `patch.h`, `dsp.h` | `src/part.rs` (`ModalVoice`, `Patch`, `PerformanceState`), `src/lib.rs` |
| `elements/dsp/voice.{h,cc}` (`Voice`, `ResonatorModel`, the chord table) | `src/voice.rs`, `src/part.rs` (`ResonatorModel`) |
| `elements/dsp/exciter.{h,cc}` | `src/exciter.rs` |
| `elements/dsp/multistage_envelope.{h,cc}` | `src/envelope.rs` |
| `elements/dsp/resonator.{h,cc}` | `src/modal.rs` (`ModalResonator`) |
| `elements/dsp/string.{h,cc}` | `src/string.rs` (`KarplusString`; the resonator port's string at 32 kHz, with this firmware's `lut_svf_shift` and dispersion mapping) |
| `elements/dsp/tube.{h,cc}` | `src/tube.rs` |
| `elements/dsp/ominous_voice.{h,cc}` (the hidden voice) | `src/fm_pair.rs` (`FmPairVoice`) |
| `elements/dsp/fx/{fx_engine,diffuser,reverb}.h` | `src/fx.rs` (`Diffuser` on 32-bit memory, `Reverb` on 16-bit memory) |
| `elements/resources.cc` (18 float tables, `lut_sine`, the sample tables) | `src/resources.rs`, `src/resources_sine.rs`, `src/samples_1.rs` to `samples_5.rs`, `src/noise_1.rs`, `src/noise_2.rs` (generated), joined in `src/samples.rs` |
| `stmlib/dsp/filter.h` (`Svf` with its coefficient setters, normalized band-pass and multimode output; `NaiveSvf` with its exact tuning), `stmlib/dsp/dsp.h` (`InterpolateWrap`) | `src/dsp.rs` |
| `stmlib/dsp/{dsp,units,delay_line,cosine_oscillator,parameter_interpolator}.h`, `filter.h` (`DCBlocker`, the tangent approximations), `stmlib/utils/random.h` | reused from transit's resonator port (`transit_resonator::stmlib`, the same stmlib commit) with its `F` float |

Not ported: everything outside `elements/dsp` and `elements/resources.cc` (the drivers, the UI, the
CV scaler, the calibration, the metering LEDs). `Part::Panic` is declared upstream but never
defined; the port has none. The module's CV scaler and input noise gate are the instrument's
business (see the instrument note in ADR 0033).

## Generated tables

`tools/gen_resources.py` (run from the repository root: `python crates/modal-voice/tools/gen_resources.py`)
writes `src/resources.rs`, `src/resources_sine.rs`, `src/samples_*.rs` and `src/noise_*.rs` from the
vendored `elements/resources.cc`. Each float table keeps the C++ literals verbatim as `f64` and
narrows them to `f32` with a const fn, as the C++ `const float` initializer narrows the same double
literals; the int16 sample tables are copied value for value, 20 to a line, in chunks that fit the
line budget. The output is byte-identical to the committed files (`git diff --exit-code
crates/modal-voice/src` after a run). The null tests prove every table the sweep reaches.

The sample tables are 128 013 + 40 963 int16 values (256 026 + 81 926 bytes); they are compiled
into the port, as the module keeps them in flash, so they ship with the app (about 330 KiB). The
oracle's own copy (`vendor/elements/resources.cc`, 1.6 MB of source) is never linked into the app.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including behavior the C++ does not
specify:

- **Flush-to-zero** (`transit_resonator::float::F`): every arithmetic result is flushed as x86
  MXCSR.FTZ does. The firmware's desktop harness (`elements/test/elements_test.cc`) does not set
  flush-to-zero, and the module never writes FPSCR; the oracle keeps the family's convention (every
  oracle module runs with the flush, the random sequencer's harness had none either), and the
  port follows it so that decaying tails reach zero instead of crawling through subnormals on the
  audio thread. The 250 ms sweep never reaches the subnormal range; the long-silence and FM pair
  scenarios do, and fail with the flush turned off (checked 2026-11-01).
- **Double arithmetic**: `Resonator::BowTable` adds the double literal `0.75` to `fabs(bow)`, so that
  sum is computed in double and rounded to float once (`src/modal.rs`).
- **Conversions out of range** (`F::trunc_i32`, `F::trunc_u32`): the x86-64 truncations the C++
  compilers emit. The FM pair's phase modulation converts a negative `fm * 2^31` to `uint32_t`,
  which wraps on x86-64 (undefined in C++).
- **Zero-initialized state**: members the firmware's `Init` never writes start at zero, as the
  module's globals in static storage, and survive re-initialization: `Part::panic_`, the modal
  resonator's `clock_divider_` and `lfo_phase_`, the exciters' `phase_` (and `particle_range_`,
  written before it is read), the whole tube (the voice's `Init` never calls `Tube::Init`), the
  string's `src_phase_`, the envelope's fifth level (read once it is done), the FM pair's smoothed
  `damping_` and `feedback_` and its oscillators' phases, the reverb's low-pass states.
- **The random generator**: stmlib's one static linear congruential generator becomes one generator
  per `ModalVoice`, starting at stmlib's power-on state (`0x21`); the bow's flow noise, the blow's
  restarts, the particles and the strings' dispersion noise draw from it in the firmware's order.
- **Table reads**: an index the firmware would read past a table (inputs outside the module's
  ranges) is clamped to the table instead; inside the ranges the arithmetic is unchanged.

## Upstream findings

For the maintainer's list (ADR 0033 note; nothing is filed by agents):

- `elements/dsp/part.h` declares `void Panic();` with no definition anywhere in `elements/`; the
  runaway guard calls `Voice::Panic` instead. Harmless (never called), but dead.
- `elements/dsp/voice.cc`'s `Init` never calls `tube_.Init()`, so the tube's states depend on static
  zeroing (fine on the module, an indeterminate read for any other instance).
- `Part::panic_`, `Resonator::clock_divider_` and `lfo_phase_` are read before any write on a
  non-static instance (same class as the resonator's and meta-modulator's findings).
- `elements/resources.cc` has a zero-length `lookup_table_uint32_table[]`, which MSVC rejects
  (C2466; patched out for the oracle).
- `elements/dsp/ominous_voice.h`'s `FmOscillator::SineFm` adds `static_cast<uint32_t>(fm *
  2147483648.0f)` to the phase, where `fm` is a signed offset in half turns (the carrier's reaches 2
  in magnitude). For a negative or out-of-range product the conversion is undefined behavior. The
  module's Cortex-M4 (`vcvt.u32.f32`) saturates it, as AArch64 (`fcvtzu`) does, so on the hardware
  every negative modulation becomes 0 and the FM is one-sided; x86-64 desktop builds wrap it modulo
  one turn instead. The port matches the hardware (`F::sat_u32`, Rust's saturating `as u32`), and
  the oracle writes the same saturation as defined C++ on every host (`patches/all/0012` in the
  oracle crate); Derek's decision 2026-11-03. Found when the FM pair null failed on macOS (Apple
  silicon) only (#198).

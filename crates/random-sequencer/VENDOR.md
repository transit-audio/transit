# Provenance

`src/` is transit's Rust port of Émilie Gillet's random sequencer firmware, written for #202
directly from her MIT C++ (ADR 0033, option (c)). No third-party Rust code is vendored.

The evaluation of #202 (2026-10-31): the eurorack README says "Code (STM32F projects): MIT
license" and "Code (AVR projects): GPL3.0"; the random sequencer (`marbles/`) is an STM32 project
and every one of its source files carries her MIT notice. (The topographic drum sequencer,
`grids/`, is an AVR project under GPL 3.0, so it is out.) crates.io has no port: searches for the
module's name find unrelated crates. GitHub code search finds one Rust derivative of the firmware,
in `dexterlabs1/openpatch` (`crates/openpatch-core/src/recall_random/`, MIT, created 2026-10-13,
two stars): a module inside an application rather than a crate, with its own declared "native
extension" deviations from the firmware and no oracle evidence. It is not used. The port was never
written from VCV Rack code (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (`marbles/random/`, `marbles/ramp/`, `marbles/resources.cc`, `marbles/scale_recorder.h`; the main loop's glue from `marbles/marbles.cc`, the preset scales from `marbles/settings.cc`) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers they use), both MIT, Émilie Gillet |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/marbles/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `marbles/marbles.cc` (`Process`: the deja vu dead band, the loop lengths, the Y divisions, the order of the generators) | `src/generators.rs` (`Generators`, `Controls`) |
| `marbles/settings.cc` (`preset_scales`) | `src/scales.rs` (`PRESETS`; `CHROMATIC` is transit's) |
| `marbles/random/t_generator.{h,cc}` | `src/t_generator.rs` |
| `marbles/random/x_y_generator.{h,cc}` | `src/xy_generator.rs` |
| `marbles/random/output_channel.{h,cc}` | `src/output_channel.rs` |
| `marbles/random/lag_processor.{h,cc}` | `src/lag.rs` |
| `marbles/random/quantizer.{h,cc}` | `src/quantizer.rs` |
| `marbles/random/random_generator.h`, `random_stream.h`, `random_sequence.h` | `src/random.rs` |
| `marbles/random/distributions.h` (`BetaDistributionSample`, `FastBetaDistributionSample`) | `src/distributions.rs` |
| `marbles/ramp/ramp.h`, `ramp_divider.h`, `slave_ramp.h` | `src/ramp.rs` |
| `marbles/ramp/ramp_extractor.{h,cc}` | `src/ramp_extractor.rs` |
| `marbles/scale_recorder.h` | `src/scale_recorder.rs` |
| `marbles/resources.cc` (`lut_raised_cosine`, `lut_logit`, the 45 `dist_icdf_*`, `distributions_table`) | `src/resources/` (generated) |
| `stmlib/dsp/{dsp,units,hysteresis_quantizer,parameter_interpolator}.h`, `units.cc` | `src/stmlib.rs`, `src/pitch_tables.rs` |

`src/sequencer.rs` (`Sequencer`) is transit's: the generators locked to a host's transport, every
random decision made by the ported code above (see its module documentation and the README).

Not ported: the drivers (the DAC, the ADCs, the clock inputs, the gate outputs, the hardware random
number generator), the UI, the CV reader and its channels (the port takes the values it hands the
generators), the calibration and the settings storage, the clock self-patching detector, the note
filter (it filters the CV input, which a host does not have), the I/O buffer, the output test and
`lut_sine` (its tone), the bootloader, the discrete-distribution quantizer (`marbles/random/
discrete_distribution_quantizer.*`, which the firmware never calls) and the ramp generator (unused
by the generators). `src/float.rs` and `src/pitch_tables.rs` are the meta-modulator port's
(`crates/modulator`), copied: the ports stay independent crates.

## Generated tables

`src/resources/` is generated from the vendored `marbles/resources.cc` by `tools/gen_resources.py`
(`python crates/random-sequencer/tools/gen_resources.py` from the repository root; `git diff
--exit-code crates/random-sequencer/src` afterwards proves the files are byte-identical). Every
literal there is a double literal the C++ narrows to `float`, so each table is kept as its `f64`
literals, verbatim, and narrowed in a `const fn`, as the C++ initializer narrows them.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including what the C++ does not specify.
Where the oracle's build pins the choice (a patch under `patches/all` in the oracle crate), the port
makes the same one:

- **Reads past an array** (`patches/all/0011`): a random component of exactly 1.0 (the top 128
  words over 2^32) reads past the inverse-cdf table in a pulse width's beta sample, and picks one
  past the drum and clusters pattern lists at a bias end; a channel phase of exactly 1.0 reads
  past `lut_raised_cosine`. The oracle and the port clamp each index: a read at an end returns the
  end, and every read upstream keeps inside is unchanged.
- **Zero-initialized state**: the Y channel's ramp divider never initializes
  `reset_at_next_pulse_`, which its first wrap reads; it starts at zero (false), as the module's
  global.
- **The random source**: the module's stream reads a buffer the hardware random number generator
  fills and falls back to a linear congruential generator; nothing fills it here or in the oracle,
  so every word comes from the generator, seeded per instance (`Generators::new`, `Sequencer::new`).
- **Flush-to-zero** (`src/float.rs`): every arithmetic result is flushed as x86 MXCSR.FTZ does,
  matching the family's oracles. The sweep rendered without the oracle's flush moved no value
  (`the_flush_moves_no_value` in the oracle crate); the flush is kept for the exponential smoothers
  that can reach the subnormal range (the lag processor's low-pass toward a voltage of exactly zero,
  the clock recovery's accuracies after a run of bad predictions).
- **Conversions** (`F::trunc_i32`, `F::trunc_u32`): the x86-64 truncations the C++ compilers emit;
  every conversion the generators make is in range.
- **Integer arithmetic**: the firmware's linear congruential steps multiply by `1664525L`, a `long`
  (64 bits on Linux, 32 on Windows); the result is kept to 32 bits either way, as here.
- **A slave ramp's pulse counter** is an `int` that grows every sample of a Bernoulli ramp that
  never restarts (after about 18 hours at 32 kHz it would overflow, undefined in C++); it wraps
  here. It only matters for a zero pulse width, which no tick sets.

## Changes

None: every point of the sweep nulls bit-identical against the oracle as written (see the README).
A deviation, if one is ever needed, is its own commit with its null test as the proof and a line
here.

# Provenance

`src/` is transit's Rust port of Émilie Gillet's texture synthesizer firmware, written for #241
directly from her MIT C++ (ADR 0033, option (c)). No third-party Rust code is vendored: the step 0
evaluation of #241 (2026-10-31) found no Rust port derived from this firmware. crates.io has none
(its "beads" crates are issue trackers); GitHub code search for the firmware's classes
(`WavetableGranularPlayer`, `delay_granular_player`, `kMaxNumGrains`) finds only C++ copies of the
upstream directory inside other firmware projects, no Rust. The port was never written from VCV
Rack code (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `f4cfa7697095a28d292bff758beb51deb49b5a86` (`beads/dsp/`, `beads/resources.cc`, `beads/gain_control.cc`; the directory arrived in `d8f47391`, 2026-10-11) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers it uses), both MIT, Émilie Gillet. The upstream README says the code "builds on a Desktop platform but not verified to build and run on a module": the reference is the C++ as its desktop harness builds it (ADR 0033). |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/beads/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `beads/dsp/processor.{h,cc}` (`Processor`: `Init`, `Configure`, `Process`, `Trim`), `frame.h`, `modulated_parameter.h` | `src/processor.rs` (`Texture`), `src/lib.rs` (`Media`, `BufferSize`, `Controls`, `SeedButton`) |
| `beads/dsp/granular_processor.h` | `src/granular_processor.rs` |
| `beads/dsp/delay_line.h`, `mu_law.{h,cc}` | `src/delay_line.rs`, `src/mu_law.rs` |
| `beads/dsp/grain.h`, `grain_envelope.h`, `voice_stealing_envelope.h`, `granular_player.h` | `src/grain.rs` |
| `beads/dsp/delay_grain.h`, `delay_granular_player.h`, `tap.h`, `multitap_delay.h` | `src/delay_grain.rs` |
| `beads/dsp/trigger_generator.{h,cc}` | `src/trigger.rs` |
| `beads/dsp/delay_clock.{h,cc}`, `delay_controller.{h,cc}` | `src/delay_controller.rs` |
| `beads/dsp/randomized_parameter.h` | `src/randomized.rs` |
| `beads/dsp/limiter.h`, `fx/fx_engine.h`, `fx/reverb.h`, `fx/wow_and_flutter.h` | `src/fx.rs` |
| `beads/dsp/sine_oscillator.h` | `src/sine.rs` |
| `beads/dsp/synth.{h,cc}`, `wavetable_voice.h`, `wavetable_grain.h`, `wavetable_granular_player.h` | `src/synth.rs` |
| `beads/gain_control.{h,cc}` | `src/input.rs` |
| `beads/resources.cc` (`lut_sine`, `wav_integrated_waves`), `beads/dsp/mu_law.cc` (`lut_ulaw_f`) | `src/resources.rs`, `src/wavetables_a.rs`, `src/wavetables_b.rs` (generated), `src/wavetables.rs` |
| `beads/dsp/sample_rate_conversion_filters.h`, `stmlib/dsp/sample_rate_converter.h` | `src/stmlib/sample_rate_converter.rs` |
| `stmlib/dsp/{dsp,units,filter,cosine_oscillator,parameter_interpolator,hysteresis_filter,hysteresis_quantizer,delay_line}.h`, `units.cc`, `stmlib/utils/{random,gate_flags}.h` | `src/stmlib/` (`pitch_tables.rs` generated) |

Not ported: everything outside the audio path. The drivers (codec, ADCs, LEDs, switches, the freeze
and seed inputs), the UI, the CV reader and its calibration (the port takes the values the CV reader
hands the processor), the settings, the factory test, the normalization detector, the persistent
buffer backup, and the main loop's glue in `beads/beads.cc` (the 2.5 ms delay of the SEED input,
the switch to the synth after 10 s without input, the codec's float to int32 conversion). Also left
out: `GranularProcessor::WasteCPUCycles` (a dummy grain player run from the main loop to load the
CPU; its output is discarded, though on the module it draws from the shared random generator),
the raw byte access for the backup (`delay_buffer_raw_bytes`, `mutable_delay_write_ptr`) and the
unused head-crossing policies of `DelayGrain::Render` (the firmware only uses STOP).

`src/float.rs`, `src/stmlib/filter.rs`, `cosine_oscillator.rs`, `parameter_interpolator.rs`,
`random.rs` and `pitch_tables.rs` are the granular port's (`crates/granular`), copied: the ports stay
independent crates.

## Generated tables

`src/resources.rs`, `src/wavetables_a.rs` and `src/wavetables_b.rs` are generated from the vendored
`beads/resources.cc` and `beads/dsp/mu_law.cc` with their literals copied verbatim, so each is parsed
as the C++ parses it: `lut_ulaw_f`'s `f`-suffixed literals straight to `f32`; `lut_sine`'s double
literals as `f64`, narrowed to `f32` in a `const fn` as the C++ initializer narrows them;
`wav_integrated_waves` as `i16`, split in two files for the line budget, then one wave of zeros (see
below). The generator is a short script: find each `name[] = { ... };` array, keep its tokens, write
them 8 (floats) or 20 (integers) to a line.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including what the C++ does not specify.
Where the oracle's build pins the choice (a patch under `patches/all` in the oracle crate), the port
makes the same one:

- **Argument evaluation order** (`patches/all/0003`): the grain players pass three or four random
  draws as arguments of one call (`p.time.value()`, `p.pitch.pitch_value()`, `p.shape.value()`,
  and in the synth `p.size.value()`). C++ leaves their order unspecified; the module's ARM GCC
  evaluates arguments in source order, x86-64 GCC and MSVC right to left. The oracle and the port
  draw in source order.
- **Ties in the grain list** (`patches/all/0004`): `std::partial_sort` is not stable and each
  library orders equal elements its own way; every finished grain has 0 samples left, and which tie
  is stolen decides the order the grains are summed in. The oracle and the port use the heap
  selection of GCC's libstdc++, the module's library (`grain::partial_sort`).
- **Frozen delay slices** (`patches/all/0005`): with the SEED button latched and nothing patched,
  a negative density CV can stretch the clock interval to about 13 times the buffer; no slice fits,
  the quantizer returns -1 and the play head starts before the buffer, its fixed-point delay
  overflowing (an out-of-bounds read). A SEED clock faster than 1/16 of the sample rate gives slices
  shorter than one sample, an integer division by zero (x86 traps; the module's Cortex-M7 returns
  0, leaving the play head silent). A slice is at most the whole buffer, and zero slices count as
  the module counts them.
- **The last wave of the last bank** (`patches/all/0006`): a wave position of exactly 1.0
  interpolates toward the next wave with a zero fraction; for the last bank that wave is past the
  table. The oracle's build and the port append one wave of zeros, so both read the same values.
- **Flush-to-zero** (`src/float.rs`): every arithmetic result is flushed as x86 MXCSR.FTZ does,
  matching the module's FPSCR.FZ and the oracle (see the README).
- **Conversions out of range** (`F::trunc_i32`): the x86-64 truncations the C++ compilers emit. The
  input clamp keeps every conversion in range.
- **Zero-initialized state**: members `Init` never writes start at zero, as the module's globals in
  static storage (the manual gain's smoothing, the limiters' and reverb's states before their first
  `Init`, every field of a grain that never started).
- **Shared delay memory**: the firmware's four quality processors share one set of bytes, and only
  the selected one runs; every quality or mono change clears them. The port keeps one memory per
  `Texture` with one `i16` per stored sample (the 16-bit value, the 12-bit value, or the 8-bit
  mu-law code), and each processor's own state (its filters, feedback and modulation) persists
  across quality changes as in the firmware. The firmware's `Trim` falls through its switch (no
  `break`), trimming the selected quality's line and those after it; the port does the same.

## Changes

None: every quality, the synth and the input stage null bit-identical against the oracle as
written (see the README). A deviation, if one is ever needed, is its own commit with its null test
as the proof and a line here.

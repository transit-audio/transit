# Provenance

`src/` is transit's Rust port of Émilie Gillet's texture processor firmware (its granular playback
mode), written for #199 directly from her MIT C++ (ADR 0033, option (c)). No third-party Rust code
is vendored: the step 0 evaluation of #199 (2026-10-30) found no Rust port of this module derived
from the firmware. crates.io has none; on GitHub, the one Rust file that names the module's
playback modes (`tacogips/vactr`, `src/dsp/ported/clouds.rs`, MIT) states in its own notice that
every mode is an "architectural adaptation" with no source port and no firmware table, so it
cannot null. The port was never written from VCV Rack's Audible Instruments (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (`clouds/dsp/`, `clouds/resources.cc`) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers it uses), both MIT, Émilie Gillet |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/clouds/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `clouds/dsp/granular_processor.{h,cc}` (`GranularProcessor`: `Init`, `Process`, `ProcessGranular`, `Prepare`, the quality), `parameters.h`, `frame.h` | `src/processor.rs` (`Granular`, `Parameters`, `Quality`, `BufferSize`) |
| `clouds/dsp/granular_sample_player.h` | `src/sample_player.rs` |
| `clouds/dsp/grain.h` | `src/grain.rs` |
| `clouds/dsp/audio_buffer.h`, `mu_law.{h,cc}` | `src/audio_buffer.rs` |
| `clouds/dsp/fx/diffuser.h`, `fx/reverb.h`, `fx/fx_engine.h` | `src/fx.rs` |
| `clouds/dsp/sample_rate_converter.h` | `src/sample_rate_converter.rs` |
| `clouds/resources.cc` (the six tables the granular mode reads), `clouds/dsp/mu_law.cc` (`lut_ulaw`) | `src/resources.rs` (generated) |
| `stmlib/dsp/{filter,cosine_oscillator,parameter_interpolator,dsp,units,rsqrt}.h`, `units.cc`, `stmlib/utils/random.h` | `src/stmlib/` (`pitch_tables.rs` generated) |

Not ported (later slices): the other playback modes, the pitch shifter and time stretcher
(`wsola_sample_player.h`, `correlator.{h,cc}`, `window.h`, and the low-pass and high-pass filters
of `Process`), the looping delay (`looping_sample_player.h`, `fx/pitch_shifter.h`) and the spectral
mode (`pvoc/`); the persistent-buffer save and load (`GetPersistentData`, `LoadPersistentData`);
and everything outside `clouds/dsp` (drivers, UI, CV scaler, settings, meter). The oracle already
wraps the stretch mode and sweeps it, for its port.

The helpers in `src/float.rs` and `src/stmlib/` (except `mod.rs`'s `soft_convert` and
`fast_rsqrt_carmack`, and the filter's `set` and `process_channel`) are the resonator port's
(`crates/resonator`), copied: the two ports stay independent crates.

## Generated tables

`src/resources.rs` and `src/stmlib/pitch_tables.rs` are generated from `clouds/resources.cc`,
`clouds/dsp/mu_law.cc` and `stmlib/dsp/units.cc`: each C++ float literal is a decimal double
assigned to a float array, so each value is rounded decimal to double to `f32` (as the C++ compiler
does) and written as the shortest decimal that parses back to the same `f32`; `lut_ulaw` is int16.
The null tests prove every value used. To regenerate after an upstream change, parse the
`const float <name>[] = { ... };` arrays of `src_filter_1x_2_45`, `lut_sin`, `lut_window`,
`lut_xfade_in`, `lut_xfade_out` and `lut_grain_size` and the `lut_ulaw` array the same way.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including behavior the C++ does not
specify:

- **Flush-to-zero** (`src/float.rs`): every arithmetic result is flushed as x86 MXCSR.FTZ does
  (see the crate README).
- **Conversions out of range** (`F::trunc_i32`, `F::trunc_u32`): the x86-64 truncations the C++
  compilers emit; a grain's phase increment is `uint32_t(pitch_ratio * 65536)` passed on as an
  `int32_t`, which the port wraps the same way.
- **Zero-initialized state**: members the firmware's `Init` never writes start at zero, as the
  module's globals in static storage, and survive re-initialization: the freeze low-pass, the grain
  player's rate phasor (not reset by its `Init`, so it carries across a quality change), the
  reverb's low-pass states, every field of a grain that never started, the crossfade tail.
- **Out-of-bounds reads**: three reads one past a table happen in the firmware, each multiplied by
  a zero fraction: the grain window at its peak (a grain whose envelope phase lands exactly on
  1.0, which every power-of-two grain width gives, reads entry 4097 of `lut_window`'s 4097), and,
  at the module's top values, size 1.0 (`lut_grain_size`) and dry/wet 1.0 (`lut_xfade_in` and
  `lut_xfade_out`). The crossfade from the freeze tail also reads one entry past its 256-sample
  tail at a zero gain. The port reads the last entry instead, which gives the firmware's value
  whenever its neighboring memory holds a finite number. The oracle refuses size and dry/wet at
  1.0 (it stops one `f32` step below), and the window read it cannot avoid stays inside its own
  read-only data, where a non-finite neighbor would show as a null failure.
- **Odd blocks at low fidelity**: the firmware's 2:1 converters take frames in pairs and would run
  past an odd block; the port renders the even part and leaves the last frame silent (the module
  always renders 32 frames; the oracle refuses odd low-fidelity blocks).

## Changes

None: every quality nulls bit-identical against the oracle as written (see the README). A
deviation, if one is ever needed, is its own commit with its null test as the proof and a line
here.

# Provenance

`src/` is transit's Rust port of Émilie Gillet's resonator firmware, written for #198 directly from
her MIT C++ (ADR 0033, option (c)). No third-party Rust code is vendored: the step 0 evaluation of
#198 (2026-10-30) found no Rust port of this module to start from (Oliver Rockstedt's repositories
carry only the macro oscillator; crates.io and GitHub have none). The port was never written from
VCV Rack's Audible Instruments (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (`rings/dsp/`, `rings/resources.cc`) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers it uses), both MIT, Émilie Gillet |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/rings/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `rings/dsp/part.{h,cc}` (`Part`, `ResonatorModel`, the chord table) | `src/part.rs` (`Resonator`, `ResonatorModel`), `src/chords.rs` |
| `rings/dsp/resonator.{h,cc}` | `src/modal.rs` (`ModalResonator`) |
| `rings/dsp/string.{h,cc}` | `src/string.rs` (`KarplusString`) |
| `rings/dsp/plucker.h` | `src/plucker.rs` |
| `rings/dsp/fm_voice.{h,cc}`, `follower.h` | `src/fm_voice.rs` |
| `rings/dsp/note_filter.h` | `src/note_filter.rs` |
| `rings/dsp/limiter.h` | `src/limiter.rs` |
| `rings/dsp/fx/reverb.h`, `fx/fx_engine.h` (16-bit memory) | `src/reverb.rs` |
| `rings/dsp/patch.h`, `performance_state.h`, `dsp.h` | `src/part.rs`, `src/lib.rs` |
| `rings/resources.cc` (five tables) | `src/resources.rs` (generated) |
| `stmlib/dsp/{filter,delay_line,cosine_oscillator,parameter_interpolator,dsp,units}.h`, `units.cc`, `stmlib/utils/random.h` | `src/stmlib/` (`pitch_tables.rs` generated) |

Not ported: the string synth (`string_synth_*`, the module's easter egg), the strummer and onset
detector (`strummer.h`, `onset_detector.h`: the module derives strums from its trigger input and
from onsets in the audio input; the instrument will strum from MIDI), the chorus and ensemble
(`fx/chorus.h`, `fx/ensemble.h`, used only by the string synth), and everything outside `rings/dsp`
(drivers, UI, CV scaler, settings).

## Generated tables

`src/resources.rs` and `src/stmlib/pitch_tables.rs` are generated from `rings/resources.cc` and
`stmlib/dsp/units.cc`: each C++ literal is a decimal double assigned to a float array, so each
value is rounded decimal to double to `f32` (as the C++ compiler does) and written as the shortest
decimal that parses back to the same `f32`. The null tests prove every value used. To regenerate
after an upstream change, parse the `const float <name>[] = { ... };` arrays of those two files the
same way.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including behavior the C++ does not
specify:

- **Flush-to-zero** (`src/float.rs`): every arithmetic result is flushed as x86 MXCSR.FTZ does
  (see the crate README).
- **Conversions out of range** (`F::trunc_i32`, `F::trunc_u32`): the x86-64 truncations the C++
  compilers emit. The FM voice's sine phase modulation, `(fm + 4) * 2^29` converted to 32 bits,
  reaches 2^32 at the largest modulation index; the port wraps it as x86-64 does (AArch64 saturates
  instead, so the macOS oracle can differ there).
- **Zero-initialized state**: members the firmware's `Init` never writes start at zero, as the
  module's globals in static storage, and survive re-initialization (`Part`'s voice rotation
  counter, `String::src_phase_`, `FMVoice::previous_sample_`, `Plucker::comb_filter_gain_`, the
  reverb's low-pass states).
- **Out-of-bounds reads**: a table index the firmware never produces for in-range inputs is
  clamped to the table instead of reading past it (in-range inputs are unaffected).

## Changes

None yet: every model nulls bit-identical against the oracle as written (see the README). A
deviation, if one is ever needed, is its own commit with its null test as the proof and a line
here.

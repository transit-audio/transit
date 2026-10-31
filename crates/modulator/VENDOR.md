# Provenance

`src/` is transit's Rust port of Émilie Gillet's meta-modulator firmware, written for #200
directly from her MIT C++ (ADR 0033, option (c)). No third-party Rust code is vendored: the
evaluation of #200 (2026-10-31) found no usable Rust port derived from this firmware. crates.io has
none (searches for the module's name, "meta modulator" and the family's name find only the macro
oscillator's and the modal synthesizer's crates). GitHub code search for the firmware's identifiers
(`ALGORITHM_DIGITAL_RING_MODULATION`, `lut_bipolar_fold`) finds two Rust files: `ajkachnic/toby`
(`src/modulator.rs`) has no license, so all rights are reserved, and it is a partial adaptation
(the comparator commented out, the crossfade rewritten as a linear one); `tacogips/vactr`
(`src/dsp/ported/warps.rs`, MIT) is an inventory that declares itself "an architectural
adaptation rather than a translated equation" with "no row a validated source-equivalent port".
Neither is used. The port was never written from VCV Rack code (GPLv3).

| | |
|---|---|
| Ported from | https://github.com/pichenettes/eurorack `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (`warps/dsp/`, `warps/resources.cc`) and https://github.com/pichenettes/stmlib `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the helpers it uses), both MIT, Émilie Gillet |
| Checked against | the same C++, built as the dev-only oracle (`../macro-osc-oracle`, `vendor/warps/` and `vendor/stmlib/`, byte for byte) |
| Licenses | [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) |

## What maps to what

| Firmware | Port |
|---|---|
| `warps/dsp/modulator.{h,cc}` (`Modulator`: `Init`, `Process`), `parameters.h` | `src/modulator.rs` (`Modulator`), `src/lib.rs` (`Parameters`, `CarrierShape`) |
| `warps/dsp/modulator.h` (`SaturatingAmplifier`) | `src/amplifier.rs` |
| `warps/dsp/modulator.{h,cc}` (`Xmod<...>`, `ProcessXmod`, `Diode`, `xmod_table_`) | `src/xmod.rs` |
| `warps/dsp/oscillator.{h,cc}` | `src/oscillator.rs` |
| `warps/dsp/sample_rate_converter.h`, `sample_rate_conversion_filters.h` | `src/converter.rs`, the kernels in `src/resources.rs` |
| `warps/dsp/vocoder.{h,cc}`, `filter_bank.{h,cc}`, `limiter.h` | `src/vocoder.rs` |
| `warps/resources.cc` (`fb_*`, `lut_sin`, `lut_xfade_in`, `lut_xfade_out`, `lut_bipolar_fold`, `lut_midi_to_f_high`, `lut_midi_to_f_low`) | `src/resources.rs` (generated) |
| `stmlib/dsp/{dsp,units,filter,parameter_interpolator}.h`, `units.cc`, `stmlib/utils/random.h` | `src/stmlib/` (`pitch_tables.rs` generated) |

Not ported: everything outside the audio path. The drivers (codec, ADCs, LEDs, switches, the
normalization probe), the UI, the CV scaler and its calibration (the port takes the values the CV
scaler hands the modulator: the level knobs squared as the drives, the algorithm and timbre knobs,
the note), the settings and the main loop's glue in `warps/warps.cc` (the audio normalization
detector that zeroes an unpatched input). Also left out: the frequency shifter (the module's
hidden mode, `ProcessEasterEgg`, with `quadrature_transform.h`, `quadrature_oscillator.h` and
the `wav_*` and `lut_ap_poles` tables) and the bypass (a copy of the input). The firmware's
`kNumOscillators` and the `MODULATION_ALGORITHM_*` enum are unused upstream.

`src/float.rs`, `src/stmlib/filter.rs` (its `Svf`), `parameter_interpolator.rs`, `random.rs` and
`pitch_tables.rs` are the texture port's (`crates/texture`), copied: the ports stay independent
crates. `src/stmlib/filter.rs` gained `CrossoverSvf` and the normalized band-pass mode.

## Generated tables

`src/resources.rs` is generated from the vendored `warps/resources.cc` and
`warps/dsp/sample_rate_conversion_filters.h` by `tools/gen_resources.py` (`python
crates/modulator/tools/gen_resources.py` from the repository root; `git diff --exit-code
crates/modulator/src` afterwards proves the file is byte-identical). Every literal there is a double
literal the C++ narrows to `float`, so each table is kept as its `f64` literals, verbatim, and
narrowed in a `const fn`, as the C++ initializer narrows them.

## Choices the C++ leaves open

The port reproduces the firmware's arithmetic bit for bit, including what the C++ does not specify.
Where the oracle's build pins the choice (a patch under `patches/all` in the oracle crate), the port
makes the same one:

- **Table reads past the end** (`patches/all/0010`): the crossfade algorithm reads `lut_xfade_in`
  and `lut_xfade_out` at a ramp that reaches 1.0 (one entry past the 257) and can overshoot it by
  rounding; the fold algorithm reads `lut_bipolar_fold` at an index computed from the signals, past
  either end once the amplifiers and the 6x up converter push them over full scale. The oracle and
  the port clamp every index to its table (and bound the scaled index first, so its conversion is
  defined): a read at an end returns that end's value, and every read upstream keeps inside is
  unchanged (`stmlib::interpolate_clamped`).
- **Zero-initialized state**: the input amplifiers' level followers and gains (`level_`,
  `pre_gain_`, `post_gain_`), which `Init` never writes and the first block reads, start at zero,
  as the module's global object in static storage. `Modulator::reset` zeroes them too.
- **Flush-to-zero** (`src/float.rs`): every arithmetic result is flushed as x86 MXCSR.FTZ does,
  matching the family's oracles, which flush by a pinned choice. The module's own flush state
  during `Process` (an interrupt handler) is not established, and its GCC build fuses
  multiply-adds, so the hardware's bits may differ from the oracle's. Over the sweep's slowest decays
  the oracle rendered without the flush moved no output sample (int16 out), so the flush is kept for
  the internal state's sake and is not discriminated by any null.
- **Conversions** (`F::trunc_i32`, `F::trunc_u32`): the x86-64 truncations the C++ compilers emit.
  From int16 inputs every conversion of the processing path is in range; floats from the device can
  reach the internal oscillator's phase modulation out of range, where the x86 result is reproduced.
- **The circular branch of the 6x down converter**: the firmware's converter takes a block branch
  for inputs of at least 384 oversampled samples (blocks of 64 frames and more, the desktop
  harness's 96) and a circular-buffer branch below; the module's 60-frame blocks always take the
  circular one, the only one ported. Blocks are whole groups of 12 frames up to 60: the vocoder's
  3x and 4x decimators leave their bands unwritten for any other length.
- **One random generator per modulator**: the firmware's is a static shared by everything; here
  each modulator has its own (`Modulator::with_seed`), drawn only by the noise carrier.

## Changes

None: every point of the sweep nulls bit-identical against the oracle as written (see the README).
A deviation, if one is ever needed, is its own commit with its null test as the proof and a line
here.

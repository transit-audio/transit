# Vendored sources

`vendor/` holds Émilie Gillet's original C++ DSP (the macro oscillator, #194, the resonator, #198,
the granular texture processor, #199, and the texture synthesizer, #241), copied unchanged from two
upstream repositories.
It is the oracle: never edit it. Licenses are in [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

| Directory | Upstream | Commit |
|---|---|---|
| `vendor/plaits/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (2023-08-16, firmware 1.2: the eurorack commit the oracle pins; the vendored files last changed in `ebe6bbf`, 2023-05-30) |
| `vendor/rings/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4`, the same commit (the vendored files last changed in `c5f910f`, 2022-01-22, typo fixes, after `43b9c9f`, 2020-04-29, which initializes the resonator's `previous_position_`) |
| `vendor/clouds/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4`, the same commit (the vendored files last changed in the merge `d1d8839`, 2023-03-13, which lost a line of `clouds/dsp/window.h`; see Local changes) |
| `vendor/beads/` | https://github.com/pichenettes/eurorack | `f4cfa7697095a28d292bff758beb51deb49b5a86` (2026-10-12: the directory arrived in `d8f47391` the day before; this commit only adds the README footnote "builds on a Desktop platform but not verified to build and run on a module". Both pin the same stmlib, `e3bd7c9c`) |
| `vendor/stmlib/` | https://github.com/pichenettes/stmlib | `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the submodule commit the eurorack commit above pins) |

## File set

From eurorack, paths kept relative to the repository root:

- `plaits/dsp/**/*.cc` and `plaits/dsp/**/*.h` (all engines, oscillators, physical models, speech,
  FM, effects, and `plaits/dsp/voice.cc` and `voice.h`)
- `plaits/resources.cc` and `plaits/resources.h` (the generated lookup tables)
- `plaits/user_data.h`: `plaits/dsp/voice.cc` includes it; with `-DTEST` it returns no user data, so
  the six-operator engines load their built-in banks, as on a module with no uploaded patches

- `rings/dsp/**/*.cc` and `rings/dsp/**/*.h` (the part, the modal resonator, the strings, the
  plucker, the FM voice and its follower, the note filter, the limiter, the reverb and its FX
  engine, and the string synth and strummer the oracle does not call yet) and `rings/resources.cc`
  and `rings/resources.h`. 28 files, each carrying Émilie Gillet's MIT notice (23 dated 2015, 5
  dated 2014); `rings/test/` (no notice), the drivers, the UI, the CV scaler and the settings are
  not vendored.

- `clouds/dsp/**/*.cc` and `clouds/dsp/**/*.h` (the granular processor and its four playback
  modes: the grain player, the WSOLA player with its correlator and window, the looping player, the
  phase vocoder; the recording buffer and mu-law codec, the 2:1 converters, the diffuser, reverb and
  pitch shifter on their FX engine), `clouds/resources.cc` and `clouds/resources.h`, and
  `clouds/drivers/debug_pin.h` (`granular_processor.cc` includes it for its `TIC` profiling macro,
  which the `-DTEST` branch defines away). 28 files, each carrying Émilie Gillet's MIT notice
  (2014). `clouds/test/`, the other drivers, the UI, the CV scaler, the settings and the meter are
  not vendored.

- `beads/dsp/**/*.cc` and `beads/dsp/**/*.h` (the effect chain: the four quality processors, the
  delay lines and mu-law codec, the grain players and their scheduler, the trigger generator, the
  delay clock and controller, the multitap delay, the limiter, the reverb on its FX engine, the
  tape wow and flutter; and the synth: the wavetable voice, grain and granular player),
  `beads/resources.cc` and `beads/resources.h`, and `beads/gain_control.cc` and `.h` (the input
  stage: automatic gain and soft clipping, which shapes everything the chain hears on the module).
  38 files, each carrying Émilie Gillet's MIT notice (2014 to 2021). `gain_control.h` includes the
  codec driver's header for one type, its raw frame; the driver is hardware code and is not
  vendored: `cpp/shim/beads/drivers/codec.h` stands in for it (see Local changes). `beads/test/`, the
  other drivers, the UI, the CV reader, the calibration, the settings, the factory test, the
  normalization detector, the buffer backup, `beads.cc` and `beads/resources/` (the generators) are
  not vendored.

From stmlib:

- `stmlib.h`, `dsp/*.h`, `dsp/units.cc`, `dsp/atan.cc` (the phase vocoder's `fast_atan2r` table),
  `fft/shy_fft.h` (its FFT), `utils/random.cc`, and `LICENSE`
- from `utils/`, only the headers the DSP includes: `buffer_allocator.h`, `dsp.h`, `random.h`, and
  `gate_flags.h` (the texture synthesizer's SEED gates; MIT, 2017).
  The other `utils/` headers are unused, and two of them (`crc32.h`, after Gary S. Brown, and
  `murmurhash3.h`, public domain) carry no MIT notice, so they are left out.

Nothing from `drivers/`, the UI, settings, the bootloader, `user_data_receiver`, `test/` or
`resources/` (the table generators) is vendored. 231 files in all (134 for the macro oscillator,
28 for the resonator, 28 for the granular processor, and `dsp/atan.cc` and `fft/shy_fft.h` added to
stmlib for it, 38 for the texture synthesizer and `utils/gate_flags.h` for it), every source file
carrying Emilie Gillet's MIT notice.

## Verification

Every vendored file is the upstream blob, byte for byte. The check compares committed blob ids, not
working-tree bytes; `.gitattributes` marks `vendor/` `-text`, so every checkout (Windows included)
also has the upstream bytes, with LF line endings. To check, clone both repositories and compare
blob ids (run from the repository root):

```sh
git clone --filter=blob:none https://github.com/pichenettes/eurorack /tmp/er
git clone --filter=blob:none https://github.com/pichenettes/stmlib /tmp/stm
( git -C /tmp/er ls-tree -r 08460a69a7e1 plaits rings/dsp rings/resources.cc rings/resources.h \
    clouds/dsp clouds/resources.cc clouds/resources.h clouds/drivers/debug_pin.h | awk '{print $3, $4}'
  git -C /tmp/er ls-tree -r f4cfa769 beads/dsp beads/resources.cc beads/resources.h \
    beads/gain_control.cc beads/gain_control.h | awk '{print $3, $4}'
  git -C /tmp/stm ls-tree -r e3bd7c9c | awk '{print $3, "stmlib/"$4}' ) | sort -k2 > /tmp/upstream.txt
git ls-files -s crates/macro-osc-oracle/vendor \
  | awk '{sub("crates/macro-osc-oracle/vendor/", "", $4); print $2, $4}' | sort -k2 > /tmp/ours.txt
join -1 2 -2 2 /tmp/ours.txt /tmp/upstream.txt | awk '$2 != $3 {print "differs:", $1}'
```

No output means every file matches (checked 2026-10-29: 134 of 134; with the resonator, 2026-10-30:
162 of 162; with the granular processor, 2026-10-30: 192 of 192; with the texture synthesizer,
2026-10-31: 231 of 231).

## Local changes (outside vendor/)

- `cpp/compat.h` is force-included into every file on every compiler. It defines
  `_USE_MATH_DEFINES` (MSVC and MinGW hide `M_PI` without it), defines `__attribute__(x)` away on
  MSVC (`stmlib/utils/dsp.h` uses a GCC attribute), and includes `<cstdio>` (the `-DTEST` branch of
  `plaits/user_data.h` calls `printf` without including it). Nothing in it changes arithmetic.
- `build.rs` passes `-flifetime-dse=1` to GCC. The wrappers value-initialize their holders so that
  every member a constructor and `Init` leave unwritten starts at zero, as on the module, where
  these objects are globals. GCC's default (`-flifetime-dse=2`) treats an object's storage as
  indeterminate when its constructor begins and drops the zeroing in front of a user-provided empty
  constructor when that object fills the whole holder (the input stage's holder, #241); level 1
  keeps stores made before a constructor. Clang and MSVC keep them without a flag.
- `patches/msvc/` holds unified diffs (`git apply -p0` format, paths relative to `vendor/`) that
  `build.rs` applies on MSVC only, to copies in the build's output directory. GCC and Clang compile
  `vendor/` exactly as fetched.
  - `0001-fm-algorithms-specialization-declarations.patch`: MSVC (error C2737) reads the four
    declarations of explicit specializations of static data members in
    `plaits/dsp/fm/algorithms.h` as definitions without an initializer. They are removed; the
    specializations are still defined in `plaits/dsp/fm/algorithms.cc`. Without the declarations
    MSVC's copy is formally ill-formed, no diagnostic required (an explicit specialization must be
    declared before a use that could instantiate it), but it is safe: the class template has no
    primary definition of `opcodes_` or `renderers_`, so nothing is ever implicitly instantiated and
    every use links to the definitions in `algorithms.cc`. Data and arithmetic are unchanged.
  - `0002-resources-empty-table.patch`: MSVC (error C2466) rejects the zero-length array
    `lookup_table_i32_table` in `plaits/resources.cc`. It is empty and nothing references it; its
    definition is removed.
  - `0003-rings-resources-empty-tables.patch`: the same MSVC error (C2466) on the two empty tables
    `lookup_table_int16_table` and `lookup_table_uint32_table` in `rings/resources.cc`. Nothing
    references them; their definitions are removed.
- `patches/all/` holds unified diffs `build.rs` applies on every compiler, to copies in the build's
  output directory, like the MSVC patches. Two:
  - `0001-clouds-window-start-clears-done.patch`: restores `done_ = false;` in `Window::Start`
    (`clouds/dsp/window.h`). Upstream, two commits each removed one of the two duplicate
    `done_ = false;` lines there (`fbb53ba`, a contributor's, and `0e3756f`, the author's, both
    March 2023), and the merge `d1d8839` (2023-03-13) kept both removals, so no line is left: no
    WSOLA window ever starts and the pitch-shifter / time-stretcher mode is silent at `08460a69`
    (the 2015 firmware had the line). The granular mode never includes this header's class in its
    path, so its reference is the upstream file as fetched; the stretch mode's reference is the
    firmware's evident intent, one assignment restored. An upstream finding for the maintainer to
    report.
  - `0002-clouds-past-the-end-reads-stay-inside.patch`: the granular processor reads one past an
    array in normal use, from inputs no check can refuse: `lut_window` at a grain's envelope peak
    (entry 4097 of 4097 whenever the phase lands on exactly 1.0), `lut_xfade_in` and
    `lut_xfade_out` when the per-sample dry/wet ramp (`ParameterInterpolator`) overshoots 1.0 by
    rounding (a non-zero fraction then blends in whatever follows the table), and `tail_buffer_`
    on every resume after a freeze (entry 256, at a zero gain; for the right channel past the
    array into `parameters_`). The patch repeats the last entry of `lut_window`, `lut_xfade_in`,
    `lut_xfade_out` and `lut_grain_size` in `clouds/resources.cc` and makes `tail_buffer_`
    `[2][257]` in `clouds/dsp/granular_processor.h`, so each read stays inside its array and
    returns exactly what the port's clamp returns (`a + (a - a) * f == a`; the extra tail entry is
    zero and multiplied by zero). It also pins `Correlator::EvaluateNextCandidate`'s shift by 32
    (`destination[i + 1] >> (32 - offset_bits)` with `offset_bits == 0`, undefined; x86 scalar
    code keeps the word, SSE and the module's Cortex-M4 give 0) to 0, the hardware's result, so
    the stretch mode's reference does not depend on the compiler. The shift is a second upstream
    finding. No granular-mode output changed (the floor, null, long-run and scenario tests pass
    unchanged).
- `patches/msvc/` also holds three for the texture synthesizer (#241), MSVC only:
  - `0004-texture-resources-empty-tables.patch`: the same error (C2466) on the empty
    `lookup_table_i16_table` and `lookup_table_i8_table` in `beads/resources.cc`; nothing references
    them; their definitions are removed.
  - `0005-texture-no-variable-length-arrays.patch`: MSVC has no variable-length arrays; the five
    block-sized scratch arrays of `processor.cc`, `synth.cc`, `trigger_generator.cc` and
    `granular_processor.h` get the codec's largest block, 24 frames (the oracle never passes more).
  - `0006-texture-packed-12-bit-items.patch`: GCC packs a 12-bit item's two signed 12-bit fields
    into 3 bytes (`__attribute__((packed))` bit fields); MSVC never lets a bit field straddle its
    storage unit (and `compat.h` defines the attribute away), so the items would take 4 bytes and
    the 12-bit delay lines would outgrow their banks. The patch packs the same two values into 3
    bytes by hand: the same values and the same item size.
- `patches/all/` holds seven more for the texture synthesizer (#241), each defining what the C++
  leaves open or undefined, so the reference is the same on every compiler (each an upstream
  finding where it is a defect):
  - `0003-texture-grain-arguments-in-source-order.patch`: the grain players pass three
    (`delay_granular_player.h`) or four (`wavetable_granular_player.h`) random draws as the
    arguments of one call; C++ leaves the evaluation order of arguments unspecified (the module's
    ARM GCC evaluates them in source order, x86-64 GCC and MSVC right to left). They are sequenced
    in source order.
  - `0004-texture-grain-list-sort-pinned.patch`: `std::partial_sort` (the grain list's resort) is
    not stable, and each standard library orders equal elements its own way; every finished grain
    has 0 samples left, and which tie is stolen decides the order the grains are summed in. The
    patch writes out the heap selection of GCC's libstdc++, the module's library.
  - `0005-texture-frozen-delay-slices-defined.patch`: in the frozen delay mode a slice can be
    longer than the buffer (the SEED button latched, nothing patched, a negative density CV
    stretching the clock interval to about 13 times the buffer: the quantizer returns -1, the play
    head starts before the buffer and its 22.10 fixed-point delay overflows, an out-of-bounds read)
    or shorter than one sample (a SEED clock faster than 1/16 of the sample rate: an integer
    division by zero, which traps on x86; the module's Cortex-M7 returns 0). A slice is at most the
    buffer, and zero slices count as the module counts them; defined results are unchanged.
  - `0006-texture-wavetable-read-stays-inside.patch`: a wave position of exactly 1.0 interpolates
    toward the next wave at a zero fraction, which for the last bank lies past `wav_integrated_waves`;
    one wave of zeros is appended, which the port reads too.
  - `0007-texture-buffer-size-overridable.patch`: `kDelayBufferSizeBits` and the delay line's bank
    size become macros with the module's values as defaults, for the oracle's second build of the
    effect chain at the port's longest buffer (8 s, in a renamed namespace; see `build.rs`).
  - `0008-texture-quiet-test-build.patch`: removes the desktop build's diagnostic `printf` in the
    wavetable grain player (one line on stdout per stolen grain); it computes nothing.
  - `0009-texture-input-manual-gain-initialized.patch`: the input stage (`beads/gain_control.h`)
    smooths `manual_gain_` toward the knob's gain on every block before anything writes it; neither
    the constructor nor `Init` sets it. On the module the object is a global, zeroed at startup, so
    it starts at 0; any other instance reads an indeterminate value. The constructor starts it at 0,
    as the module does. Found on Linux: the wrapper's value-initialized holder lost its zeroing
    under GCC's lifetime dead-store elimination, so each render after the first in a process began
    from the previous one's state (`build.rs` now also passes `-flifetime-dse=1` to GCC).
- `cpp/shim/beads/drivers/codec.h` stands in for the codec driver's header, which
  `beads/gain_control.h` includes for one type: the codec's raw frame (two `int32_t`). It is on the
  include path after the overlay and before `vendor/`, and no vendored file of that name exists.
- `cpp/texture_oracle.cc` wraps the texture synthesizer's effect chain (`beads::Processor`, #241).
  The processor, its three banks of delay memory and its 32768-word reverb memory are
  value-initialized (zeroed), as the module's globals. It drives the chain as the firmware's desktop
  harness does (`Init`, `Configure`, then `Process` per block), refuses blocks the chain cannot take
  (empty, over 24 frames, odd at the clean tape quality, whose 2:1 converters would leave the
  half-rate block unwritten), and can run without the flush-to-zero scope (an evidence switch for
  the test that shows the float output tells the flush apart). `build.rs` compiles it twice: on the
  module's memory, and on the port's longest buffer with the bank size raised and the vendored
  namespace renamed (`-Dbeads=transit_texture_long`), so the two builds share no symbol.
  `cpp/texture_extras_oracle.cc` wraps the synth (`beads::Synth`, on its own reverb memory: on the
  module it borrows the chain's) and the input stage (`beads::GainControl`).
- `cpp/ftz_scope.h` holds the flush-to-zero scope both wrappers open around every call into the
  vendored code (moved out of `cpp/oracle.cc` for #198, unchanged).
- `cpp/granular_oracle.cc` wraps the granular processor (`clouds::GranularProcessor`, #199) in its
  granular and stretch playback modes. The processor and its two memory blocks are
  value-initialized (zeroed), as the module's globals: `Init` leaves the freeze low-pass, the
  silence flag, the grain player's rate phasor, the reverb's low-pass states and the grains' fields
  unwritten. It drives the processor as the firmware's desktop harness (`clouds/test/clouds_test.cc`)
  does: `Prepare` after `Init` and after every block. The memory sizes it accepts must leave room
  for the FX workspace in both channel layouts, the pitch shifter's 8192-byte clear included, and
  for the longest grain, and the large block may not exceed 131072 bytes: every layout then
  records fewer than 131072 samples, below which a grain's 16.16 phase in an `int32_t` cannot
  overflow (above it the phase wraps negative and the grain reads before the buffer).
- `cpp/resonator_oracle.cc` wraps the resonator's `rings::Part` (#198). The part and its 32768-word
  reverb memory live in one value-initialized (zeroed) allocation, as the module's globals live in
  zeroed static storage: the part's `Init` leaves some members unwritten (the voice rotation
  counter, the strings' resampler phase, the FM voices' feedback sample, the reverb's low-pass
  states), so the oracle must start them at zero to be the module and to be deterministic.

## Updating

Upstream has not changed this code since the commits above. If it does: copy the same file set at
the new commit, update the commits here and in `LICENSE-THIRD-PARTY`, re-run the verification,
check the patches still apply (`build.rs` fails the build if a hunk does not match exactly
once), and re-measure the floor (README).

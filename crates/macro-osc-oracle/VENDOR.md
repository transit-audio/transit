# Vendored sources

`vendor/` holds Émilie Gillet's original C++ DSP (the macro oscillator, #194, and the resonator,
#198), copied unchanged from two upstream repositories.
It is the oracle: never edit it. Licenses are in [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

| Directory | Upstream | Commit |
|---|---|---|
| `vendor/plaits/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (2023-08-16, firmware 1.2: the eurorack commit the oracle pins; the vendored files last changed in `ebe6bbf`, 2023-05-30) |
| `vendor/rings/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4`, the same commit (the vendored files last changed in `c5f910f`, 2022-01-22, typo fixes, after `43b9c9f`, 2020-04-29, which initializes the resonator's `previous_position_`) |
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

From stmlib:

- `stmlib.h`, `dsp/*.h`, `dsp/units.cc`, `utils/random.cc`, and `LICENSE`
- from `utils/`, only the three headers the DSP includes: `buffer_allocator.h`, `dsp.h`, `random.h`.
  The other `utils/` headers are unused, and two of them (`crc32.h`, after Gary S. Brown, and
  `murmurhash3.h`, public domain) carry no MIT notice, so they are left out.
- `dsp/atan.cc` is not included: nothing in the file set references it

Nothing from `drivers/`, the UI, settings, the bootloader, `user_data_receiver`, `test/` or
`resources/` (the table generators) is vendored. 162 files in all (134 for the macro oscillator,
28 for the resonator), every source file carrying Emilie Gillet's MIT notice.

## Verification

Every vendored file is the upstream blob, byte for byte. The check compares committed blob ids, not
working-tree bytes; `.gitattributes` marks `vendor/` `-text`, so every checkout (Windows included)
also has the upstream bytes, with LF line endings. To check, clone both repositories and compare
blob ids (run from the repository root):

```sh
git clone --filter=blob:none https://github.com/pichenettes/eurorack /tmp/er
git clone --filter=blob:none https://github.com/pichenettes/stmlib /tmp/stm
( git -C /tmp/er ls-tree -r 08460a69a7e1 plaits rings/dsp rings/resources.cc rings/resources.h | awk '{print $3, $4}'
  git -C /tmp/stm ls-tree -r e3bd7c9c | awk '{print $3, "stmlib/"$4}' ) | sort -k2 > /tmp/upstream.txt
git ls-files -s crates/macro-osc-oracle/vendor \
  | awk '{sub("crates/macro-osc-oracle/vendor/", "", $4); print $2, $4}' | sort -k2 > /tmp/ours.txt
join -1 2 -2 2 /tmp/ours.txt /tmp/upstream.txt | awk '$2 != $3 {print "differs:", $1}'
```

No output means every file matches (checked 2026-10-29: 134 of 134; with the resonator, 2026-10-30:
162 of 162).

## Local changes (outside vendor/)

- `cpp/compat.h` is force-included into every file on every compiler. It defines
  `_USE_MATH_DEFINES` (MSVC and MinGW hide `M_PI` without it), defines `__attribute__(x)` away on
  MSVC (`stmlib/utils/dsp.h` uses a GCC attribute), and includes `<cstdio>` (the `-DTEST` branch of
  `plaits/user_data.h` calls `printf` without including it). Nothing in it changes arithmetic.
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
- `cpp/ftz_scope.h` holds the flush-to-zero scope both wrappers open around every call into the
  vendored code (moved out of `cpp/oracle.cc` for #198, unchanged).
- `cpp/resonator_oracle.cc` wraps the resonator's `rings::Part` (#198). The part and its 32768-word
  reverb memory live in one value-initialized (zeroed) allocation, as the module's globals live in
  zeroed static storage: the part's `Init` leaves some members unwritten (the voice rotation
  counter, the strings' resampler phase, the FM voices' feedback sample, the reverb's low-pass
  states), so the oracle must start them at zero to be the module and to be deterministic.

## Updating

Upstream has not changed this code since the commit above. If it does: copy the same file set at
the new commit, update the two commits here and in `LICENSE-THIRD-PARTY`, re-run the verification,
check the MSVC patches still apply (`build.rs` fails the build if a hunk does not match exactly
once), and re-measure the floor (README).

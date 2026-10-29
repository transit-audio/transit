# Vendored sources

`vendor/` holds Émilie Gillet's original C++ DSP, copied unchanged from two upstream repositories.
It is the oracle: never edit it. Licenses are in [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

| Directory | Upstream | Commit |
|---|---|---|
| `vendor/plaits/` | https://github.com/pichenettes/eurorack | `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4` (2023-08-16, firmware 1.2: the eurorack commit the oracle pins; the vendored files last changed in `ebe6bbf`, 2023-05-30) |
| `vendor/stmlib/` | https://github.com/pichenettes/stmlib | `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec` (the submodule commit the eurorack commit above pins) |

## File set

From eurorack, paths kept relative to the repository root:

- `plaits/dsp/**/*.cc` and `plaits/dsp/**/*.h` (all engines, oscillators, physical models, speech,
  FM, effects, and `plaits/dsp/voice.cc` and `voice.h`)
- `plaits/resources.cc` and `plaits/resources.h` (the generated lookup tables)
- `plaits/user_data.h`: `plaits/dsp/voice.cc` includes it; with `-DTEST` it returns no user data, so
  the six-operator engines load their built-in banks, as on a module with no uploaded patches

From stmlib:

- `stmlib.h`, `dsp/*.h`, `dsp/units.cc`, `utils/random.cc`, and `LICENSE`
- from `utils/`, only the three headers the DSP includes: `buffer_allocator.h`, `dsp.h`, `random.h`.
  The other `utils/` headers are unused, and two of them (`crc32.h`, after Gary S. Brown, and
  `murmurhash3.h`, public domain) carry no MIT notice, so they are left out.
- `dsp/atan.cc` is not included: nothing in the file set references it

Nothing from `drivers/`, the UI, settings, the bootloader, `user_data_receiver`, `test/` or
`resources/` (the table generators) is vendored. 134 files in all, every source file carrying
Emilie Gillet's MIT notice.

## Verification

Every vendored file is the upstream blob, byte for byte. The check compares committed blob ids, not
working-tree bytes; `.gitattributes` marks `vendor/` `-text`, so every checkout (Windows included)
also has the upstream bytes, with LF line endings. To check, clone both repositories and compare
blob ids (run from the repository root):

```sh
git clone --filter=blob:none https://github.com/pichenettes/eurorack /tmp/er
git clone --filter=blob:none https://github.com/pichenettes/stmlib /tmp/stm
( git -C /tmp/er ls-tree -r 08460a69a7e1 plaits | awk '{print $3, $4}'
  git -C /tmp/stm ls-tree -r e3bd7c9c | awk '{print $3, "stmlib/"$4}' ) | sort -k2 > /tmp/upstream.txt
git ls-files -s crates/macro-osc-oracle/vendor \
  | awk '{sub("crates/macro-osc-oracle/vendor/", "", $4); print $2, $4}' | sort -k2 > /tmp/ours.txt
join -1 2 -2 2 /tmp/ours.txt /tmp/upstream.txt | awk '$2 != $3 {print "differs:", $1}'
```

No output means every file matches (checked 2026-10-29: 134 of 134).

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

## Updating

Upstream has not changed this code since the commit above. If it does: copy the same file set at
the new commit, update the two commits here and in `LICENSE-THIRD-PARTY`, re-run the verification,
check the MSVC patches still apply (`build.rs` fails the build if a hunk does not match exactly
once), and re-measure the floor (README).

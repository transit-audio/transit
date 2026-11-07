# Vendored sources

`vendor/` holds the Opus reference implementation, **libopus 1.6.1**, the floating-point
encoder and decoder transit uses to write and read Ogg Opus files (#238, ADR 0048). It is copied
unchanged from the release tarball; never edit it. Its license is in
[`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) (BSD 3-clause, `vendor/COPYING`).

| Directory | Upstream | Release |
|---|---|---|
| `vendor/` | https://opus-codec.org/, https://gitlab.xiph.org/xiph/opus | `opus-1.6.1.tar.gz` from https://downloads.xiph.org/releases/opus/, SHA-256 `6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1`, as listed in that directory's `SHA256SUMS.txt` |

## File set

Paths kept relative to the tarball's root:

- `COPYING`, `AUTHORS`, `README`: the license, the authors, upstream's readme.
- `celt_sources.mk`, `silk_sources.mk`, `opus_sources.mk` and the matching `*_headers.mk`:
  upstream's own source lists. `build.rs` reads the source lists, so the compiled file set is
  exactly upstream's for this configuration.
- `include/*.h`: the public API.
- The `.c` files of `CELT_SOURCES`, `SILK_SOURCES`, `SILK_SOURCES_FLOAT`, `OPUS_SOURCES` and
  `OPUS_SOURCES_FLOAT` (137 files), and every header at the top of `celt/`, `silk/`, `silk/float/`
  and `src/` (64 headers with `include/`). 3.1 MB.

Not vendored: `dnn/` (deep PLC, DRED, OSCE and LACE/NoLACE: 23 MB of model weights, all off by
default), the x86, ARM, MIPS and Xtensa SIMD sources and headers (`celt/x86`, `celt/arm`,
`silk/x86`, ...), `silk/fixed/` (the fixed-point build), the tests, the demo programs
(`opus_demo.c`, `opus_compare.c`, `repacketizer_demo.c`, `qext_compare.c`), the documentation and
every build system (autotools, CMake, meson, the Visual Studio projects).

## Build configuration

`build.rs` compiles the file set with the `cc` crate: no cmake, autotools, meson or system
library, so every runner builds it from source with its own C compiler (MSVC on Windows, Apple
Clang on macOS, GCC on Linux). The defines are those upstream's CMake build sets for its default
floating-point configuration:

| Define | Why |
|---|---|
| `OPUS_BUILD` | Building the library itself (every upstream build). |
| `ENABLE_HARDENING` | Upstream's default (`OPUS_HARDENING ON`): cheap run-time checks. |
| `PACKAGE_VERSION="1.6.1"` | `opus_get_version_string()` returns `libopus 1.6.1`. |
| `HAVE_LRINT`, `HAVE_LRINTF` | The float-to-integer rounding of `celt/float_cast.h` (upstream defines them when the C library has them; every supported one does). |
| `VAR_ARRAYS` (GCC, Clang) or `USE_ALLOCA` (MSVC) | Stack arrays: C99 variable-length arrays, or `alloca` where MSVC has none (upstream's CMake makes the same choice). Never `NONTHREADSAFE_PSEUDOSTACK`, so separate codec states are thread-safe. |

Left at upstream's defaults: no `FIXED_POINT`, no `FLOAT_APPROX`, no `ENABLE_ASSERTIONS`, no
`OPUS_HAVE_RTCD` or SIMD (portable C: a bounce is offline, and the result is the same on every
runner), no `ENABLE_DEEP_PLC`, `ENABLE_DRED`, `ENABLE_OSCE` or `ENABLE_QEXT`, no custom modes.
Upstream's hardening compiler flags (`-fstack-protector-strong`, `_FORTIFY_SOURCE`) are not added:
they are toolchain options, not codec configuration, and transit's own build does not set them.
The C is compiled at `-O2` (`/O2`) whatever the cargo profile, with the compiler's warnings off
(upstream's warnings are not transit's to fix).

## Local changes

None. No file under `vendor/` is edited and no patch is applied.

## Updating

Download the new release and its `SHA256SUMS.txt` from https://downloads.xiph.org/releases/opus/,
check the hash, replace `vendor/` with the same file set (the source lists name it), update the
version in this file, in `build.rs` (`PACKAGE_VERSION`), in `LICENSE-THIRD-PARTY`, in the crate
description and in the test `the_version_is_the_vendored_release`, and rerun the crate's tests and
`transit-audio-io`'s Opus tests: their measured figures (pre-skip, spectrum floor, bitrates) are
pinned to this release and must be measured again.

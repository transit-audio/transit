# transit-macro-osc-oracle

The null-test oracle for transit's macro oscillator ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md), #194).

It builds Émilie Gillet's original C++ DSP (MIT) and lets Rust tests render it sample by sample.
The Rust port (`transit-macro-osc`, #195) is gated by comparing its output against this crate, engine
by engine, on the same runner.

## Why dev-only

The app is pure Rust; the C++ stays in the test oracle (ADR 0033). The crate is `publish = false`,
and `cargo xtask layers` fails if any workspace package, the apps included, takes it as a normal or
build dependency (`DEV_ONLY` in `xtask/src/main.rs`). Only a dev-dependency is allowed, so it never
reaches a release build.

## What is in it

- `vendor/`: the upstream C++, byte for byte. Sources, commits and the file set are in
  [`VENDOR.md`](VENDOR.md); licenses in [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).
- `build.rs`: builds it with the `cc` crate. C++11 (C++14 on MSVC, its minimum), `-DTEST` (the
  desktop branch of the firmware's library, as its own test makefile), `-O2` in every cargo profile,
  no fast-math, no FP contraction (`-ffp-contract=off` on GCC and Clang, `/fp:strict` on MSVC).
- `patches/msvc/`: two small patches MSVC needs, applied to copies at build time on MSVC only
  (VENDOR.md lists them). GCC and Clang build `vendor/` unchanged.
- `cpp/oracle.cc`: a C ABI over one voice (`Patch` and `Modulations` in, int16 out/aux frames, as
  the firmware) and over each engine on its own (float out/aux before the voice's gain, limiter and
  low-pass gate), each on its own 16 KB arena, plus seeding of the random state.
- `src/ffi.rs`: the only `unsafe` in the crate. `Session` holds a process-wide lock, because the
  C++ keeps its random state in one static; everything renders through a session.
- `src/sweep.rs`: the fixed sweeps (78 engine-level and 84 voice-level points per engine, 48 kHz,
  12-frame blocks, a fresh engine or voice and the same seed per point).
- `src/residual.rs`: peak and RMS residual in dBFS.
- `src/floor.rs`: the measured floor per runner family.

Upstream runs at 48 kHz only (its tables and filter coefficients assume it), so the oracle accepts
48 kHz only. Its block size is the firmware's `kBlockSize`, 12 frames; a render call takes up to
`kMaxBlockSize`, 24.

While a session is held, the thread runs with flush-to-zero on, as the firmware's desktop test
harness does (MXCSR FTZ on x86-64; FPCR.FZ on AArch64, which also zeroes denormal inputs). Render
the port inside the same session so both sides run in the same mode.

## The floor

The floor is the residual of the oracle against itself. `tests/self_floor.rs` renders the whole
sweep (24 engines, 3,888 renders) twice in one process with the same seed and compares every
sample, then renders it again in a second process and compares digests of every render. Measured:
bit-identical (`-inf` dBFS peak and RMS, no differing sample) on Windows x86-64 with MSVC, and on the
Linux and macOS CI runners each time the test passes there. Results are compared only within one
runner family, never across: each family has its own compiler and instruction set.

To re-measure (after a compiler, flag or vendor change):

```sh
cargo test --locked -p transit-macro-osc-oracle --test self_floor -- --nocapture
```

A pass means the floor is still `-inf` on that runner. A failure prints the engine, the point and the
residual; it means the reference is unstable (state leaking between renders, an uninitialized read,
a floating-point mode that differs between processes) and must be fixed before any port gate is
trusted. Never rerun it to pass. Per-engine output levels are printed by
`cargo test -p transit-macro-osc-oracle --test every_engine_renders -- --nocapture`.

## Credits

The DSP is by Émilie Gillet, released under the MIT License (see `LICENSE-THIRD-PARTY`). transit's
own code here is MIT OR Apache-2.0.

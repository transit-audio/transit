# transit-macro-osc-oracle

The null-test oracle for transit's ported instruments ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md)): the macro oscillator (#194), the resonator (#198) and the granular texture processor (#199).

It builds Émilie Gillet's original C++ DSP (MIT) and lets Rust tests render it sample by sample.
The Rust ports (`transit-macro-osc`, #195; `transit-resonator`, #198; `transit-granular`, #199) are gated by comparing their
output against this crate, engine by engine and model by model, on the same runner. One crate holds
every module family's oracle, a module per family, so they share the build, the session lock, the
residual and the floor.

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
- `patches/msvc/`: three small patches MSVC needs, and `patches/all/`, for every compiler: one
  upstream line restored (the stretch mode's `Window::Start`), and the granular processor's
  past-the-end reads and its correlator's shift by 32 made defined. They are applied to copies at
  build time (VENDOR.md lists them); every other file builds as fetched.
- `cpp/oracle.cc`: a C ABI over one voice (`Patch` and `Modulations` in, int16 out/aux frames, as
  the firmware) and over each engine on its own (float out/aux before the voice's gain, limiter and
  low-pass gate), each on its own 16 KB arena, plus seeding of the random state.
- `src/ffi.rs`: the only `unsafe` in the crate. `Session` holds a process-wide lock, because the
  C++ keeps its random state in one static; everything renders through a session.
- `cpp/resonator_oracle.cc`: a C ABI over the resonator's part (`rings::Part`: six models,
  polyphony 1 to 4, the note filter, the reverb and the limiter; float out/aux), on zeroed memory as
  the module's static storage, with the model, polyphony and bypass settings. `cpp/ftz_scope.h` is
  the flush-to-zero scope both wrappers share.
- `cpp/granular_oracle.cc`, `src/granular.rs` and `src/granular_sweep.rs`: the granular processor
  (#199) in its granular and its pitch-shifter / time-stretcher playback modes, at its four buffer
  qualities, stereo int16 in and out at 32 kHz in the firmware's 32-frame blocks, on the module's
  memory sizes or others, driven as the firmware's harness drives it (`Prepare` after every block);
  and its fixed sweep (each of nine parameters at five levels, three freeze and trigger patterns,
  both modes, every quality: 888 points, each the quality's whole recording plus 0.25 s long, a
  fresh processor and a fixed seed and input per point).
- `src/resonator.rs` and `src/resonator_sweep.rs`: the resonator's safe wrapper and its fixed sweep
  (159 points per model and polyphony, 6 models, polyphony 1 to 4, 48 kHz, 24-frame blocks, a fresh
  part and the same seed per point).
- `src/sweep.rs`: the fixed sweeps (78 engine-level and 84 voice-level points per engine, 48 kHz,
  12-frame blocks, a fresh engine or voice and the same seed per point).
- `src/residual.rs`: peak and RMS residual in dBFS.
- `src/floor.rs`: the measured floor per runner family.

The macro oscillator and the resonator run at 48 kHz only (their tables and filter coefficients
assume it), so their oracles accept 48 kHz only; the granular processor runs at 32 kHz only, its
module's codec rate. The macro oscillator's block size is the firmware's `kBlockSize`, 12 frames; a render
call takes up to `kMaxBlockSize`, 24. The resonator's firmware renders `kMaxBlockSize`, 24 frames,
per call, and one call is one control block.

The C++ wrapper turns flush-to-zero on around each call into the vendored code, as the firmware's
desktop test harness does (MXCSR FTZ on x86-64; FPCR.FZ on AArch64, which also zeroes denormal
inputs), and restores the previous mode before returning. Rust code, the port included, always runs
in Rust's default floating-point mode: Rust assumes it, and changing it under Rust code is undefined
behavior. Where the reference (the C++ as that harness builds it) relies on flush-to-zero, the port reproduces it explicitly in its own
code, never by changing the host's floating-point mode.

## The floor

The floor is the residual of the oracle against itself. `tests/self_floor.rs` renders the whole
sweep (24 engines, 3,888 renders) twice in one process with the same seed and compares every
sample, then renders it again in a second process and compares digests of every render. Measured:
bit-identical (`-inf` dBFS peak and RMS, no differing sample) on Windows x86-64 with MSVC, and on the
Linux and macOS CI runners each time the test passes there. Results are compared only within one
runner family, never across: each family has its own compiler and instruction set.

`tests/resonator_self_floor.rs` does the same for the resonator: the whole sweep (6 models × 4
polyphonies × 159 points, 3,816 renders of 100 ms) twice in one process and again in a second
process. Measured: bit-identical on Windows x86-64 with MSVC (2026-10-30); Linux and macOS prove it
on every CI run.

`tests/granular_self_floor.rs` does the same for the granular processor: the whole sweep (888
renders, 4 qualities, 2 modes) twice in one process and again in a second process. Measured:
bit-identical on Windows x86-64 with MSVC (2026-10-30); Linux and macOS prove it on every CI run.

To re-measure (after a compiler, flag or vendor change):

```sh
cargo test --locked -p transit-macro-osc-oracle --test self_floor -- --nocapture
cargo test --locked -p transit-macro-osc-oracle --test resonator_self_floor -- --nocapture
cargo test --locked -p transit-macro-osc-oracle --test granular_self_floor -- --nocapture
```

A pass means the floor is still `-inf` on that runner. A failure prints the engine, the point and the
residual; it means the reference is unstable (state leaking between renders, an uninitialized read,
a floating-point mode that differs between processes) and must be fixed before any port gate is
trusted. Never rerun it to pass. Per-engine output levels are printed by
`cargo test -p transit-macro-osc-oracle --test every_engine_renders -- --nocapture`.

## Credits

The DSP is by Émilie Gillet, released under the MIT License (see `LICENSE-THIRD-PARTY`). transit's
own code here is MIT OR Apache-2.0.

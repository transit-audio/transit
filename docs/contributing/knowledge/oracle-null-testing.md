# Oracle null tests

How we know a ported engine is right. Read it before you port DSP, touch `crates/macro-osc`,
`crates/resonator`, `crates/granular` or `crates/macro-osc-oracle`, or write a test that compares audio. The gate list
is in [`../evidence/gates.md`](../evidence/gates.md#oracle-null-tests); the decision is
[ADR 0033](../../adr/0033-built-in-instruments-mutable-instruments-port.md).

## The idea

"Sounds the same" is not a test. We port Émilie Gillet's open-source DSP (MIT, the
`pichenettes/eurorack` and `pichenettes/stmlib` repositories) to Rust. The original C++ is the
reference: we build it as a dev-only crate, render the same sweep through it and through the port,
and require the same bits.

## The oracle

`crates/macro-osc-oracle` compiles the C++ with the `cc` crate. It is never linked into the app:
`cargo xtask layers` rejects any normal or build dependency on it, and its FFI `unsafe` lives in one
file (`src/ffi.rs`).

- The reference is the original C++ as its own desktop test harness builds it: `-DTEST`, `-O2`,
  FP contraction off (`-ffp-contract=off`, `/fp:strict` on MSVC), no fast-math, and flush-to-zero on
  x86-64 around each call (`cpp/oracle.cc`; FPCR.FZ on AArch64). The module itself is not the oracle,
  because a module cannot be nulled against. Its arithmetic differs from the reference in the last
  bits (fused multiply-adds, no flush-to-zero).
- `vendor/` is the upstream tree, byte for byte. MSVC needs two small patches; `build.rs` applies
  them to copies in the build directory, so `vendor/` never changes.
- Provenance is checked by git blob id against the pinned upstream commits (`VENDOR.md` has the
  command): every vendored file must match.

## The floor

Before any port is compared, the oracle is compared with itself: the whole sweep rendered twice in
one process, then again in a second process, with identical bytes required (`tests/self_floor.rs`).
Measured: identical, `-inf` dBFS, on Windows x86-64 (MSVC). Linux (GCC) and macOS (Apple Clang)
prove theirs on every CI run. Results are compared within one runner family, never across. If the
floor test fails, the reference is unstable (uninitialized read, state leaking between renders) and
every port gate waits. Never rerun it to pass.

## The sweep

Fixed parameter points, a fixed seed for the random state, a fresh engine per point, 48 kHz, the
firmware's 12-frame blocks (`src/sweep.rs`). Per engine: 78 engine-level points (float out and aux)
and 84 voice-level points (int16 frames through the envelopes and the low-pass gate). The Rings port
uses a resonator sweep of 159 points in 24-frame blocks.

## The gate

The port's output must equal the oracle's bit for bit, out and aux, and the random state after each
point must match too (`crates/macro-osc/tests/null_vs_oracle.rs`, `null_vs_oracle_voice.rs`,
`voice_scenarios_vs_oracle.rs`). The assert compares exact bits and the margin is zero. A residual is
a defect to hunt against the C++, never a tolerance to widen.

## What it found

Applied to the MIT Rust port we started from, the gate found about twenty defects at the engine and
voice levels (the list is in `crates/macro-osc/README.md`, "Corrections against the firmware").
Examples:

- The six-operator FM engine rendered all its voices at once; the firmware staggers them.
- A sign error in a speech table (the k0 table).
- The ensemble and diffuser effects each had their own ring buffer; the firmware shares one.
- The trigger was read 8 blocks late; the firmware reads it 5 blocks late.
- The fifth chord swapped two notes (0, 7, 7.01, 12).
- The Speech model's clock did not follow the sample rate (E63, below).

The engine sweep never reached some of these. Scenarios (engine changes, block sizes 1 to 24, full
macro ramps, long silences) found the rest.

## E63: the pitch gate class

The oracle runs at 48 kHz only. The Speech model played an octave sharp at 96 kHz, and a pitch test
passed it: its autocorrelation search accepted a peak at twice the period. Rule
([E63](../../escapes/E63-the-speech-model-played-out-of-tune-away-from-48-khz.md)): a pitch test
compares against the expected absolute frequency, rejects octave and lag-multiple aliases, and
covers every engine with its own pitch path at every supported rate (`tests/voice_pitch.rs`).

## Flush-to-zero in Rust

The oracle runs with flush-to-zero; Rust runs in its default mode. We do not change the host's mode:
`core::arch` documents changing the rounding or denormal mode under Rust as undefined behavior. The
port reproduces the flush in its own code, where the reference's arithmetic produces subnormals
(`utils::ftz` in `crates/macro-osc`; an `f32` wrapper that flushes every result in
`crates/resonator/src/float.rs`). It is proven for the sweep, not in general. In the granular
port (`crates/granular`) it is not proven at all yet: the firmware's output is int16, a subnormal
never moves a sample by one LSB, and every granular test passes with the flush off. TODO (#199
slice 2): the float output path must bring a test that fails with the flush off.

## Deviations we keep

One is documented: the firmware's engines share one RAM arena, and the first 12-frame block after
returning to a six-operator bank from six of the other engines replays leftover memory. On the
module that can be a click. The port plays the six-operator engine's own tail instead, and the test
pins the difference to that single block (ADR 0033, "Oracle deviation note").

## Cost

The sweep runs in seconds. CI compiles the C++ on three runners. A C++ compiler is a build
prerequisite for the workspace tests, not for the app (`docs/building.md`).

## A second port

The Rings resonator (`crates/resonator`) was written from the firmware, not from a Rust port. Its six
models matched the oracle bit for bit on the first full run, at every polyphony, using the same
crate for the oracle.

## A third port

The texture processor's granular mode (`crates/granular`) was written from the firmware too, and
nulled at 0 LSB at its four buffer qualities on the first full run, at its own rate (32 kHz) and
block size (32 frames). Two lessons from it: a sweep must let the recording fill before it measures
(the first sweep rendered silence at most positions, and a check on the first block passed on the
dry signal alone), and vendoring an upstream commit is not the same as vendoring a working module:
a 2023 merge left the stretch mode silent at the pinned commit, which the oracle's sound check
found. The review added a third: "an index of exactly 1.0 reads one past the table" is not bounded
by bounding the parameter when the index is a ramp toward it (the dry/wet crossfade overshoots by
rounding), so the oracle defines the firmware's past-the-end reads by padding the arrays on a
build-time copy, and its memory sizes are capped where the firmware's fixed-point phase overflows.

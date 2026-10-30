# transit-macro-osc

transit's macro oscillator ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#195): 24 synthesis engines (virtual analog, phase distortion, six-operator FM with three banks,
wave terrain, string machine, chiptune, waveshaping, two-operator FM, granular formant, additive,
wavetable, chords, speech, swarm, filtered and particle noise, inharmonic string, modal resonator,
and three drum models). Pure Rust, `#![forbid(unsafe_code)]`, no allocation once an engine is
built. Slice 1 is the engine level at 48 kHz; the instrument wrapper, the voice-level comparison
and a sample-rate parameter come later.

## Where the code comes from

The DSP is Émilie Gillet's MIT firmware (firmware 1.2). `src/` started as Oliver Rockstedt's MIT
Rust port of it, vendored at `effe9864bb` ([`VENDOR.md`](VENDOR.md)), and is transit's own from
there: every engine was corrected against the original C++, built as a dev-only oracle
([`crates/macro-osc-oracle`](../macro-osc-oracle/README.md)). Licenses and notices:
[`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The gate

`tests/null_vs_oracle.rs` renders each engine over the oracle's engine-level sweep (78 points:
notes 36, 60 and 84, free-running and triggered, each macro at 0, 0.25, 0.5, 0.75 and 1; 48 kHz;
the firmware's 12-frame blocks; a fresh engine and the same seed per point), next to the oracle on
the same runner, and holds the peak and RMS residual of both outputs to the oracle's floor plus a
margin. The floor is `-inf` (the oracle is bit-identical to itself) and the margin is zero: all 24
engines null bit-identical (measured on Windows x86-64 MSVC on 2026-10-29; Linux and macOS prove it
on every CI run). Any residual is a port defect: hunt it against the C++, never widen the bound.

```sh
cargo test --locked -p transit-macro-osc --test null_vs_oracle -- --nocapture
MACRO_OSC_NULL_POINTS="n60 h0.50" cargo test -p transit-macro-osc --test null_vs_oracle -- --exact fm --nocapture
```

The second form prints, for every point whose label contains the filter, the residual and the first
differing sample.

## Flush-to-zero

The firmware runs with flush-to-zero (the oracle switches the FPU mode on around each C++ call).
Rust code always runs in the default mode, where subnormals are kept, and changing the mode under
Rust is undefined behavior. So the port reproduces flush-to-zero in its own code: `utils::ftz`
turns a subnormal result into a zero of the same sign, and the operations that can produce a
subnormal in the firmware's arithmetic are flushed one by one, as the FPU would: the one-pole
smoother (`ONE_POLE`), the SVF and one-pole filters, the resonator bank, the parameter
interpolators, the FX engine's low- and high-pass, and the sums of the swarm, particle, sine and saw
voices. The sweep proves each one (the swarm, modal, string and particle engines decay into the
subnormal range and null bit-identical only with the flush).

The copy is proven for the sweep, not in general: an operation the sweep never drives into the
subnormal range may still be unflushed. On AArch64 (the macOS runner) the oracle's FPCR.FZ also
flushes subnormal inputs, which x86 FTZ does not; the port reproduces the x86 behavior. So a residual
on macOS in the swarm, modal, string or particle engines (the ones that decay into the subnormal
range) is blamed on that difference first, then hunted like any other.

## Corrections against the firmware

Each is its own commit with the engine's null test as the proof.

- Trigger state is the firmware's bit set (a block can carry a rising edge and a high gate).
- Pitch is computed against the corrected sample rate (`kCorrectedSampleRate`, 47872.34 Hz), as are
  the LPC speech pitch, the six-operator voices and the wavetable's initial pitch.
- One random generator per engine, passed to every component in the firmware's order, instead of a
  process-wide atomic.
- Engines render the block they are given, not their whole scratch buffer (oversampled engines
  render twice the block).
- The variable shape oscillator advances its phasor when free-running.
- Filter tangent coefficients are evaluated as the firmware's double-precision macros.
- The FX engine (ensemble, diffuser) is the firmware's shared ring buffer.
- The interpolators do not clamp their index (the sine's quadrature lookup reads past 1.0 on
  purpose); SLOPE scales both coefficients.
- Drums: analog snare pulse decay over 4.8 samples; synthetic snare tail on alternate samples;
  hi-hat square noise offset.
- Filtered noise sweeps low-pass to high-pass; grain formant blep and carrier scale and the aux DC
  blocker; naive and SAM speech table blends, the SVF's additive processing, the k0 table sign.
- The six-operator engine renders its voices staggered, as the firmware.
- The inharmonic strings start at f0 = 0.01.
- Flush-to-zero, above.

Two items flagged before the port was vendored turned out to be firmware behavior, kept as is: the
virtual analog engine's auxiliary oscillator starts at phase 0.25 (`set_master_phase(0.25f)` in the
firmware), and the VOSIM oscillator advances its carrier with the un-interpolated frequency, as the
firmware does (no firmware 1.2 engine uses that oscillator).

## Credits

The DSP is by Émilie Gillet (MIT). The Rust port it started from is by Oliver Rockstedt (MIT).
transit's own changes are MIT OR Apache-2.0.

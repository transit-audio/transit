# transit-granular

transit's granular texture processor ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#199 slice 1): ported to Rust from Émilie Gillet's MIT firmware. `Granular` is the firmware's
processor in its granular playback mode: it records its stereo input (with feedback) into a buffer,
plays up to 64 overlapping grains from it, then runs a diffuser, a reverb and a dry/wet crossfade.
Pure Rust, `#![forbid(unsafe_code)]`, no allocation once a `Granular` is built, one random
generator per processor. Not played by the app: it is the nulled reference port of the module's
granular mode, and the base the pitch shifter and time stretcher (the stretch mode, slice 2) builds
on.

| Parameter | What it does |
|---|---|
| Position | where in the recording grains start, newest (0) to oldest (1) |
| Size | grain length, 1024 to 16384 samples (32 to 512 ms) |
| Pitch | grain transposition, -48 to +48 semitones |
| Density | below 0.5 grains are seeded at a regular rate, above it at random; the distance from 0.5 sets the overlap (none between 0.47 and 0.53) |
| Texture | grain window, square to triangle to raised cosine; above 0.75 the diffuser |
| Dry/wet, Spread, Feedback, Reverb | equal-power mix, random grain panning, recording feedback, reverb amount |
| Freeze, Trigger | stop the recording (it resumes with a 256-sample crossfade); seed one grain at once |

## Where the code comes from

Written by transit from her C++ (`eurorack/clouds/dsp` at `08460a69`, stmlib at `e3bd7c9c`); no
third-party Rust code is in it. Provenance, the file map and the choices the C++ leaves open:
[`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The render contract

`Granular::process(parameters, input, output)` renders one block as the firmware's
`GranularProcessor::Process` followed by `Prepare`, as the firmware's desktop harness drives it:
stereo int16 frames in and out (the codec's format; the output goes through the firmware's soft
clipper), 1 to 32 frames (`MAX_BLOCK_SIZE`; the module always renders 32), an even number at a
low-fidelity quality. Parameters are clamped to the module's ranges (a NaN to the bottom), which
leaves values inside them untouched. A quality change clears the recording and the next block is
silent, as on the module.

## Sample rate, buffer and quality

The firmware runs at 32 kHz, its codec's rate, and its grain sizes, filters, diffuser and reverb are
tuned for it, so the port runs at 32 kHz only (`SAMPLE_RATE`); the null tests are only meaningful
there. A host at another rate resamples to and from 32 kHz at the edges, as the module's codec
does; that converter, its latency and its pitch tests at every host rate belong to the effect.

`BufferSize` is the firmware's two memory blocks. `BufferSize::FIRMWARE` is the module's (16-bit
stereo records 1.02 s, mono 1.86 s, 8-bit mu-law stereo 4.09 s, 8-bit mono 7.42 s);
`BufferSize::from_seconds` sizes them for a 16-bit stereo length from 0.768 s to 1.214 s, with the
other qualities scaled as on the module. The top is the firmware's own limit: a grain's read
position is 16.16 fixed point in 32 bits and a transposed grain may read a quarter of the buffer
ahead, so no recording may exceed 131072 samples (the module's 8-bit mono buffer, 118776, is near
it). Longer buffers need a wider phase, a deliberate deviation outside what the oracle can null.

`Quality` is the module's quality button: 16-bit stereo (the default), 16-bit mono, and the 8-bit
mu-law buffers with the grains at 16 kHz behind the firmware's 2:1 converters, stereo or mono.

## The gate

`tests/null_vs_oracle.rs` renders every quality over the oracle's granular sweep
(`transit_macro_osc_oracle::granular_sweep`, the granular mode's 111 points per quality: position,
size, pitch, density, texture, dry/wet, spread, feedback and reverb each at five levels around a
center; free-running, frozen from the middle to three quarters, and triggered every 25 blocks;
32 kHz; 32-frame blocks; the module's memory; a fresh processor, the same seed and the same
integer-generated input per point, each point the quality's whole recording plus 0.25 s long) next
to the oracle on the same runner, 12 tests (quality by pattern). The int16 output must be identical
(the oracle's floor is `-inf`, the margin zero) and the random state after each point must equal the
oracle's. All 12 pass (Windows x86-64 MSVC, 2026-10-30).

`tests/long_run_vs_oracle.rs` runs 60 s per quality with every parameter moving, freeze toggled
every few seconds, triggers and 10 s of silence, at 0 LSB from the first frame to the last.
`tests/scenarios_vs_oracle.rs` holds the same rule for quality changes in one processor, every
block size, bypass and silence, the shortest and the longest buffer sizes with the longest
transposed grains, and full-range ramps. `tests/pitch.rs` measures one grain's transposition (-12,
0, +7, +12 semitones of a 440 Hz sine) at the expected absolute frequency within 1 cent, with
the alias rejection of the E63 gate class (measured: 0.00 cents).

```sh
cargo test --locked -p transit-granular -- --nocapture
GRANULAR_NULL_POINTS="pos0.50 size0.50" cargo test -p transit-granular --test null_vs_oracle -- --exact stereo_free --nocapture
```

The second form prints the residual and the first differing frame of every point whose label
contains the filter.

## Flush-to-zero

The oracle runs the C++ with flush-to-zero, as the firmware's desktop harness does
(`clouds/test/clouds_test.cc`); the module itself runs with FZ off. Rust code always runs in the
default floating-point mode, so the port's arithmetic is done in `float::F`, an `f32` whose `+`,
`-`, `*` and `/` flush a subnormal result to a zero of the same sign, as x86 FTZ does, everywhere in
the DSP. Unlike the resonator, no test here depends on it: the firmware's output is int16, and the
long run and the scenarios pass with the flush turned off as well (checked 2026-10-30), because a
subnormal difference never moves an output sample by one LSB. The flush is kept so the port's
internal state stays the oracle's to the bit, which a float output (a deviation the effect may
choose) would expose. So the flush is unverified by any oracle comparison today; a float output
path must bring a test that fails with the flush off (as the resonator's silences do).

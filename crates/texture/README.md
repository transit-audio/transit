# transit-texture

transit's texture processor ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#241 slice 1): ported to Rust from Émilie Gillet's MIT firmware. Pure Rust,
`#![forbid(unsafe_code)]`, no allocation once built, one random generator per instance. Not played
by the app yet: the insert effect (proposed name "Texture") is slice 2, the instrument form slice 3.

- `Texture` is the firmware's effect chain: it records its stereo input (with feedback, soft-clipped
  at the lower resolutions) into a buffer at one of four qualities (`Media`), stereo or mono, and
  plays it back as up to 32 overlapping grains, or, with SIZE at its top, as an eight-tap delay
  (echoes, or slices of the buffer while frozen); then the dry/wet crossfade, tape wow and flutter
  (dirty tape), a limiter, the reverb and the output limiter.
- `Synth` is what the module plays with nothing patched: a wavetable oscillator and grains of the
  same wavetables (12 banks of 16 waves), then the limiters and the reverb.
- `InputStage` is the module's input stage: raw 32-bit codec frames to floats with the automatic
  gain (a 1-second wait, a 5-second learning phase, then peak protection) or a manual one, the tape
  qualities' soft clipping, and the copy of a patched input to an unpatched one. On the module it
  shapes everything the chain hears; the device decides whether to use it.

| Control | What it does |
|---|---|
| TIME | where grains start, newest (0) to oldest (1); in the delay mode the echo multiplier (1 to 16 intervals), or while frozen the slice |
| SIZE | grain length, 4 s down nine octaves (reversed grains below a third); above 0.99 the delay mode (a 1.5 s crossing, 0.75 s fades) |
| SHAPE | grain envelope: square, decay, bell, reversed decay; tap envelopes in the delay mode |
| PITCH | -24 to +24 semitones on the knob, plus or minus 60 by CV |
| DENSITY | below noon a regular grain rate, above it a random one, at noon taps only; in the delay mode the clock ratio (clocked) or the interval (free) |
| attenuverters | without CV: internal randomness (positive smooth or per grain, negative occasional jumps); with CV: the CV's amount. SIZE's is also the delay's bandwidth |
| FEEDBACK, DRY/WET, REVERB | recording feedback (the synth: the wavetable bank), the mix, the reverb |
| FREEZE, SEED | stop the recording (it resumes through a crossfade from its 256-frame tail); seed grains or clock the delay (button held, latched, or the SEED input's gates) |

## Where the code comes from

Written by transit from her C++ (`eurorack/beads/dsp` and `beads/gain_control.cc` at `f4cfa769`,
stmlib at `e3bd7c9c`); no third-party Rust code is in it. Provenance, the file map and the choices
the C++ leaves open: [`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The render contract

`Texture::process(controls, freeze, seed, input, output)` renders as the firmware's
`Processor::Process`: float stereo frames in and out (the module's float path, between its codec
conversions, which belong to the device), 1 to 24 frames per firmware block (`MAX_BLOCK_SIZE`; the
module renders its codec rate / 4000: 12, 8 or 6), an even number at the clean tape quality; longer
buffers are rendered in 24-frame blocks. `freeze` has one flag per frame; `seed` one gate-flags
byte per frame (`stmlib::extract_gate_flags`), or `None` when nothing is patched. Controls are
clamped to the module's ranges (a NaN to the bottom), input samples to plus or minus 16.
`configure(media, mono)` is the module's reaction to a quality or mono change: the buffer is
cleared, the triggers, the delay controller and the limiters restart, the reverb keeps its memory.

## Sample rate, buffer and quality

The module's codec does not run at one rate: each quality sets it (`Media::sample_rate`, the
firmware built with `ENABLE_HARDWARE_24K`): digital 48 kHz, vintage digital 32 kHz, clean tape
48 kHz with the grains at 24 kHz behind the firmware's 2:1 converters, dirty tape 24 kHz. The
filters, grain sizes, delay times, reverb and wow are tuned to those rates, so the port runs each
quality at its own rate only; the null tests are meaningful there and nowhere else. The device
(slice 2) resamples at its edges, for every quality but digital and clean tape in a 48 kHz session.
The synth runs at 48 kHz.

`BufferSize` is the delay memory, in seconds of 16-bit stereo at 48 kHz (every other layout divides
the same bits: 12-bit holds 4/3 as many frames, 8-bit twice as many, mono twice again). The
firmware's is 4 s (768000 bytes), which is also the shortest: frozen, the delay mode cuts the
buffer into slices of a clock interval that can reach 4 s, and needs one slice to fit. The longest
is 8 s: every delay tap is 22.10 fixed point in 32 bits, and the longest (the clean tape quality's
mono buffer, 16/3 of the frames, plus the 68-sample minimum and the 1024-sample pitch-shifter
window) must stay below 2^21 samples, which holds up to 393011 frames (8.19 s). `BufferSize::new`
and `from_seconds` refuse anything outside 4 to 8 s.

## The gate

All against the oracle (`transit_macro_osc_oracle::texture`) on the same runner, bit for bit (the
oracle's floor is `-inf`, the margin zero):

- `tests/null_vs_oracle.rs`: every quality (four media, stereo and mono, 8 tests) over the oracle's
  sweep (`texture_sweep`: 285 points per quality, 2280 in all: the twelve controls at five levels
  around a granular and a delay-mode center, free, frozen and SEED-pulsed, each point long enough
  for its grains or taps to read recorded audio). Every point sounds after its first two blocks
  except an exact list asserted both ways (DENSITY at noon in the granular free and frozen
  patterns, where only taps seed grains).
- `tests/long_run_vs_oracle.rs`: 60 s per quality, every control moving, freeze toggling, the delay
  mode and back, SEED pulses held and latched, 10 s of silence, CVs everywhere, the monophonic mode
  and the gate output; the random generators end equal.
- `tests/scenarios_vs_oracle.rs`: quality and mono changes mid-stream, every block size, the dry
  path with trims and the gate output, the shortest and the longest buffers (the oracle's second
  build), the firmware's undefined slices (defined as the oracle's build defines them), and the
  flush to zero: the oracle rendered without it differs in its bits, and the port matches the
  flushed reference.
- `tests/synth_vs_oracle.rs`: the synth at its four resolutions over its 147-point sweep, and the
  last wave of the last bank. `tests/input_vs_oracle.rs`: the input stage at each codec rate and
  soft-clipping mode.

```sh
cargo test --locked -p transit-texture
```

## Flush-to-zero

The module's firmware turns flush-to-zero on (FPSCR.FZ in `beads/drivers/system.cc`), and the
oracle sets the x86 equivalent around every call. Rust code always runs in the default
floating-point mode, so the port does its arithmetic in `float::F`, an `f32` whose `+`, `-`, `*`
and `/` flush a subnormal result to a zero of the same sign. The output here is float, so the flush
is visible: `the_flush_to_zero_is_visible_and_reproduced` renders a decaying tail with and without
it (hundreds of samples differ, subnormals against zeros) and holds the port to the flushed bits.

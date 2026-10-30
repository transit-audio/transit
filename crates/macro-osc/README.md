# transit-macro-osc

transit's macro oscillator ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#195): 24 synthesis engines (virtual analog, phase distortion, six-operator FM with three banks,
wave terrain, string machine, chiptune, waveshaping, two-operator FM, granular formant, additive,
wavetable, chords, speech, swarm, filtered and particle noise, inharmonic string, modal resonator,
and three drum models), and the voice that plays them (`voice::Voice`: engine selection, the
trigger delay, the decay envelope, the low-pass gate, the limiter and the int16 output). Pure Rust,
`#![forbid(unsafe_code)]`, no allocation once a voice or engine is built. The voice runs at 44.1,
48, 88.2 or 96 kHz. The transit instrument wrapper (MIDI, parameters, panel) comes in a later
slice of #195.

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
differing sample. The engine tests also require the random state after each point to equal the
oracle's: a voice's engines share one generator, so a draw that never reaches an engine's own output
still moves the next engine's noise.

### Voice level

`tests/null_vs_oracle_voice.rs` renders each engine through a whole voice over the oracle's
voice-level sweep (84 points: the 78 above with decay 0.5, plus decay 0 and 1 at each note's
triggered center; 100 ms each, so the envelopes decay) and requires the int16 frames, out and aux,
to be identical: 0 LSB on every frame. There is no tolerance at all, not even 1 LSB: the port
reproduces the firmware's int16 conversion (`Clip16(1 + int32_t(x))` after the `-32767` scaling)
exactly. All 24 engines are identical (Windows x86-64 MSVC, 2026-10-29).

`tests/voice_scenarios_vs_oracle.rs` covers what the sweep does not reach, against the oracle's voice
with the same 0-LSB rule: engine changes within one voice by the patch (the six-op banks, every
engine forward, backward and in a scrambled order) and by the engine CV (free-running and sampled on
triggers), the level input (the low-pass gate following the level, the accent), the frequency,
timbre and morph inputs with their attenuverters patched and unpatched, the note input and its
smoothing, the speech and chiptune special cases, blocks of 1 to 24 frames, the three macros ramped
over their whole range on every engine, 20 to 100 s of silence (subnormal decays), and a voice
re-initialized after use. One documented difference is pinned there, below.

`tests/voice_sample_rates.rs` checks the other rates (the oracle runs at 48 kHz only): a sine voice at
notes 45, 69 and 93 plays within 1 cent of its 48 kHz pitch at 44.1, 48, 88.2 and 96 kHz, measured
over two seconds of zero crossings (measured: within 0.001 cent).

```sh
cargo test --locked -p transit-macro-osc --test null_vs_oracle_voice -- --nocapture
cargo test --locked -p transit-macro-osc --test voice_scenarios_vs_oracle -- --nocapture
```

### The one difference: the firmware's shared engine RAM

The firmware's engines share one 16 KB RAM arena (`Voice::Init` frees the allocator before each
engine's `Init`), and an engine's `Reset` on an engine change restores most of what it keeps
there. The six-operator engine's staggered-render accumulator is the exception: it is not cleared,
so the first 12-frame block after the voice returns to a six-op bank mixes in whatever the engines
rendered in between left at that address. Six engines write there: the string machine (ensemble
delay), speech, swarm, particle noise, the inharmonic string, and, on a 64-bit host only, the
wavetable engine (its pointer map is twice as large as on the module). The port's engines own their
memory, so the six-op engine resumes from its own accumulator, which is exactly what the firmware
does when nothing overwrote it (re-entry from another six-op bank, or after any of the other 15
engines). Reproducing the overlap would mean reproducing every engine's byte layout, the host's
pointer size included, to replay one block of leftover memory, so the port does not.
`six_op_reentry_after_a_ram_sharing_engine` pins the difference to that one block: every other frame
is identical, the block is identical after the other 15 engines and differs after those six.

## Engine changes

The firmware 1.2 voice does not crossfade or fade between engines. On a change (or a user-data
reload) it loads the new engine's data (the six-op bank, the wavetables), resets the new engine and
resets the out channel's limiter (not the aux's), and the next block is the new engine's. The port
does the same; an anti-click fade, if transit wants one, belongs to the instrument wrapper, outside
the nulled voice.

## Sample rate

`Voice::new` takes a `SampleRate`: 44.1, 48 (the default), 88.2 or 96 kHz, rendered in blocks of 1 to
24 frames, timed as the firmware's 12-frame blocks. At 48 kHz the voice is the firmware's arithmetic,
bit for bit. The firmware computes pitch against its corrected rate, 47872.34 Hz (the module's codec
clock), so on the module every note is in tune; rendered at a true 48 kHz the same arithmetic plays
4.6 cents sharp (48000 / 47872.34), and so does this voice. At the other rates the normalized pitch
is scaled by 48000 / rate and the voice's envelope coefficients (`(200 * 12) / rate` and
`(20 * 12) / rate`) are per block at that rate, so notes and decays keep their 48 kHz frequency and
duration; the engines' own sample-rate-dependent constants follow the rate given to their `init`.
Per-block coefficients inside the engines and the low-pass gate (the vactrol's 0.6 attack per block,
filter cutoffs normalized to the sample) are the firmware's 48 kHz values, so timbre can shift
slightly at the other rates; only 48 kHz is nulled. Whether the instrument retunes by the 4.6 cents
is the wrapper's decision.

## Flush-to-zero

The firmware runs with flush-to-zero (the oracle switches the FPU mode on around each C++ call).
Rust code always runs in the default mode, where subnormals are kept, and changing the mode under
Rust is undefined behavior. So the port reproduces flush-to-zero in its own code: `utils::ftz`
turns a subnormal result into a zero of the same sign, and the operations that can produce a
subnormal in the firmware's arithmetic are flushed one by one, as the FPU would: the one-pole
smoother (`ONE_POLE`), the SVF and one-pole filters, the resonator bank, the parameter
interpolators, the FX engine's low- and high-pass, and the sums of the swarm, particle, sine and saw
voices. The sweep proves each one (the swarm, modal, string and particle engines decay into the
subnormal range and null bit-identical only with the flush). The voice adds its own decaying states:
the decay envelope, the low-pass gate envelope (vactrol state and its powers), the low-pass gate's
gain and bleed, the limiter's peak follower, and the internal-envelope modulation; with the flush the
vactrol state freezes at a tiny normal value once its decrement flushes, as on the firmware. The
int16 output never showed those subnormals, so that part is state fidelity (unit test and the
long-silence scenario), not an output fix.

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
- One random generator per voice, instead of a process-wide atomic: the voice owns it and hands it to
  whichever engine renders (`Engine::render` takes it), as the firmware's engines share
  `stmlib::Random`; every component draws in the firmware's order.
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

Found at the voice level (the engine sweep never reached them), each its own commit:

- The six-op LFO of a voice with no patch loaded runs at the firmware's init rate (0.1 per sample),
  wrapping and drawing a random value every block (the port held it still).
- The inharmonic string's reset re-initializes its damping filter and DC blocker.
- The fifth chord is 0, 7, 7.01, 12 (the port's table swapped the middle notes).
- The chiptune engine initializes four of its five square voices, as the firmware: the fifth keeps
  its zeroed power-on state.
- LPC speech plays the frame it is asked for, unclamped (a consonant on the table's last frame).
- The voice: the trigger is read 5 blocks late (`kTriggerDelay`); no floor on the decay knob; the
  chiptune envelope shape is the timbre attenuverter as is (no 0.05 minimum); the envelope
  coefficients are per 12-frame block whatever a call's size; the output is the firmware's int16
  frame; `init` restores the power-on state and the first render loads its engine (even engine 0).

Two items flagged before the port was vendored turned out to be firmware behavior, kept as is: the
virtual analog engine's auxiliary oscillator starts at phase 0.25 (`set_master_phase(0.25f)` in the
firmware), and the VOSIM oscillator advances its carrier with the un-interpolated frequency, as the
firmware does (no firmware 1.2 engine uses that oscillator).

## Credits

The DSP is by Émilie Gillet (MIT). The Rust port it started from is by Oliver Rockstedt (MIT).
transit's own changes are MIT OR Apache-2.0.

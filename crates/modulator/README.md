# transit-modulator

transit's meta-modulator ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#200): ported to Rust from Émilie Gillet's MIT firmware. Pure Rust, `#![forbid(unsafe_code)]`, no
allocation once built, one random generator per instance. The app plays it as the Cross Modulator
insert effect (`transit-dsp`).

`Modulator` takes two signals, a carrier (input 1) and a modulator (input 2), each through a
saturating amplifier with a noise gate, and combines them:

| Algorithm (at) | What it does |
|---|---|
| Crossfade (0) | an equal-power crossfade; TIMBRE moves from the carrier to the modulator |
| Fold (1/8) | the sum and product of the two through a wavefolder; TIMBRE is the fold depth |
| Diode ring (2/8) | an analog ring modulator's diodes (Julian Parker's model); TIMBRE is the drive |
| Digital ring (3/8) | the product, soft-limited; TIMBRE is the gain |
| XOR (4/8) | the two as 16-bit words XORed bit by bit, from their sum; TIMBRE is the amount |
| Comparator (5/8) | minimum, threshold, largest magnitude and a rectified variant in turn; TIMBRE picks |
| Vocoder (from 0.725) | 20 bands; the carrier's spectrum shaped by the modulator's band envelopes; TIMBRE shifts the formants; the release grows to a freeze near 1 |

Between two algorithms the output morphs from one to the other; between the comparator and the
vocoder it crossfades through the dry modulator. The algorithms run at 6x the codec rate between
the firmware's polyphase converters. An internal oscillator can stand in for the carrier
(`CarrierShape`: a sine, triangle or sawtooth for the algorithms, which are a sawtooth, a pulse or
filtered noise for the vocoder), input 1 then modulating its phase (sine) or frequency (the others).
The aux output is the two inputs summed (or the internal oscillator).

## Where the code comes from

Written by transit from her C++ (`eurorack/warps/dsp` at `08460a69`, stmlib at `e3bd7c9c`); no
third-party Rust code is in it. Provenance, the file map and the choices the C++ leaves open:
[`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

## The render contract

- 96 kHz only (`SAMPLE_RATE`): the pitch table, the filter bank and the converters assume it.
- Blocks of whole 12-frame groups up to `BLOCK_SIZE` (60, the module's codec block); any other
  length renders silence and leaves the state alone.
- `Modulator::process` takes and returns the codec's int16 frames (`[input 1, input 2]` in,
  `[main, aux]` out): the firmware's `Process`, bit for bit. `Modulator::process_float` is the same
  between the codec's conversions: floats in the codec's scale (an int16 over 32768) in, the main
  and aux outputs before their int16 conversion out. The device uses it, so the session's floats
  are neither quantized nor clipped at the edges.
- `Parameters` are what the module's CV scaler hands the firmware: drives 0 to 1 (the level knobs
  squared), algorithm and timbre 0 to 1, a MIDI note for the internal oscillator.
- No allocation after `Modulator::new`; `Modulator::reset` returns to a fresh modulator.

## The gate

`tests/null_vs_oracle.rs` renders the oracle's fixed sweep (`modulator_sweep` in the oracle crate:
432 points, the external carrier and the three oscillator shapes, every algorithm and morph at
three timbres, drive and note axes, ramps and jumps across the whole range) through the port and
compares the main and aux outputs sample by sample: all 432 points are bit-identical (Windows x86-64
MSVC, 2026-10-31; without `TRANSIT_FULL_ORACLE=1` a strided tenth runs). Shorter blocks (12 to 48
frames) null too. The oracle's own floor is `-inf` (`tests/modulator_self_floor.rs` there).

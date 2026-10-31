# transit-random-sequencer

transit's random sequencer ([ADR 0033](../../docs/adr/0033-built-in-instruments-mutable-instruments-port.md),
#202): ported to Rust from Émilie Gillet's MIT firmware. Pure Rust, `#![forbid(unsafe_code)]`, no
allocation once built, one random source per instance. The app plays it as the Random Sequencer
MIDI effect (`transit-dsp`, `random_sequencer`).

## What it does

A master clock ticks. On each tick a random vector decides, by one of seven models, which of two
gate channels fire and how long their gates stay high; the clock itself is a third gate (high for
half a tick). Three X channels (one per gate) and a Y channel draw voltages: a beta distribution
shaped by spread (narrow to the bias at none, a coin toss at full) and bias, scaled to a range, and
quantized to a scale by steps (ever fewer, heavier degrees as steps grow; below the center the
voltage glides instead). Deja vu works on every random decision: at zero each is new, at the center
a loop of 1 to 16 decisions repeats, toward full the loop's decisions are replayed in random order.

| Model | Gates |
|---|---|
| Coin toss | one toss per tick: gate 1 or gate 2 (the bias leans it) |
| Clusters | bursts at multiples and divisions of the clock (the bias picks how dense) |
| Drums | eight-step kick and snare patterns (the bias picks how busy) |
| Independent | a toss per gate |
| Divider | fixed multiplications and divisions of the clock (from 8x to 1/8 by the bias) |
| Three states | gate 1, gate 2 or neither |
| Markov | a model of the last 16 ticks (the firmware has it; the module does not offer it) |

## Two layers

- `Generators`: the firmware's generators as its main loop drives them, at 32 kHz (`SAMPLE_RATE`)
  in blocks of 5 frames (`BLOCK_SIZE`), the internal clock or a patched clock's gate flags in,
  three gates and four voltages per frame out. This is the layer the null test compares.
- `Sequencer`: the same generators locked to a host's transport (`Block`: playing, sample
  position, beat position, tempo, sample rate, length), MIDI notes out with sample offsets.
  - **Clock.** The beat (a quarter note) is the clock; `RATES` lists the firmware's divisions of a
    patched clock (`p/q` from 1/4 to 4, times a quarter or four by the range: 21 rates from 4/1 to
    1/64 with the dotted and triplet ones). Ticks fall on the division's grid from beat 0, exactly,
    with no jitter; the jitter bends them by the firmware's multiplier, pulled back toward the grid
    as the firmware pulls its jittery clock toward the straight one. Times are kept in beats and
    turned into sample offsets only in the block that plays them, so a tempo change keeps the grid
    and the events are the same at any block size.
  - **Gates.** The firmware samples its gates frame by frame; here each gate's rise and fall are
    computed from its ramp's slope in beats, from the state the firmware's code leaves at the tick
    (a Bernoulli ramp's gate ends where its phase passes the pulse width; a divider's gates start
    and end at its ramp's crossings).
  - **Notes.** Gate 1 plays X1, the clock X2, gate 2 X3 (`Voices` picks which). A rise starts a
    note, a fall ends it, a pulse that starts while the gate is high retriggers. A voltage is a key
    at a semitone per twelfth of a volt above the base note (the quantized voltage, or the free one
    rounded to the nearest semitone below the steps' center), folded into 0 to 127; microtonal
    scale degrees round to the nearest semitone. The velocity is the Y voltage (0 to 5 V across 1
    to 127, its bias the center, its spread the variation), drawn at every tick.
  - **Starts.** A play start, a locate or a loop restarts the generators at the grid point at or
    after the position with the firmware's reset; with the seed latched they restart from the seed,
    so the same transport path plays the same notes.

## The gate

`tests/null_vs_oracle.rs` renders the oracle's fixed sweep (`sequencer_sweep` in the oracle crate:
173 points of 3 or 6 s over every model, jitter, gate lengths, rates and ranges, deja vu across its
dead band and loop lengths, the X distribution, steps and scales, the control modes and ranges, the
input-sampling mode, the Y channel, every clock routing including a patched clock steady, swung,
paused and restarted and at audio rate, explicit resets, a loaded scale and motion) through the port
and compares every frame's three gates and four voltages exactly: all 173 points are bit-identical
(Windows x86-64 MSVC, 2026-10-31; without `TRANSIT_FULL_ORACLE=1` a strided tenth runs). Blocks of
1 to 4 frames null too, and one-minute runs with deja vu shuffling and looping. The oracle's own
floor is exact (`tests/sequencer_self_floor.rs` there). A deliberate one-rounding change to the beta
sample's interpolation makes 9 of the 173 points differ.

`tests/sequencer.rs` covers the transport layer: the same events at 64, 512 and 37-frame blocks for
five models, ticks on the grid, a tempo change keeping it, jitter moving ticks but keeping their
count, a latched seed replaying a locate, a locate starting on the next grid point, every note
ended and none doubled, and a frozen loop repeating.

## Where the code comes from

Written by transit from her C++ (`eurorack/marbles` at `08460a69`, stmlib at `e3bd7c9c`); no
third-party Rust code is in it. Provenance, the file map and the choices the C++ leaves open:
[`VENDOR.md`](VENDOR.md). Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY).

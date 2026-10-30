# ADR 0033: Built-in instruments: a port of the Mutable Instruments family

- **Status:** proposed 2026-10-29 (maintainer decision)
- **Date:** 2026-10-29
- **Reopen when:** the crates.io evaluation finds no acceptable starting point and the Rust port proves too costly to carry; the upstream license terms change; the null-test residual floor cannot be measured stably on CI hardware; or a product question asks for engines the family does not cover.

## Context

transit has no built-in instruments beyond what it inherited. The maintainer wants wavetable and granular synthesis. Writing generic engines from scratch would mean months of sound design with no reference to judge them against.

Émilie Gillet's open-source Mutable Instruments DSP (the `eurorack` repository on GitHub, code under the MIT license) is a coherent, proven family:

- Plaits: a macro oscillator with virtual analog, wavetable, FM, additive, physical-model and drum engines. Firmware 1.2 adds six-operator FM, wave terrain, string machine and chiptune.
- Rings and Elements: physical modeling.
- Clouds: granular texture synthesis.
- Warps: a meta-modulator, which is an effect.
- Marbles and Grids: random and drum sequencers, which are MIDI effects.

The modules were built to run on a Cortex-M4, so CPU cost is low, and they share one design language.

## Decision

Port the family as small, focused built-in instruments and effects. They are mono by design where the originals are. Users build bigger instruments by composition: a track chain (an instrument plus inserts) and a group of tracks, the rack of [ADR 0034](0034-racks-layering-instruments-through-track-groups.md).

Order: Plaits first, with a voice allocator and a small editor, as the first slice of v0.6.0. Then Rings, Clouds, Warps, then Marbles and Grids.

Rules, each agreed by the maintainer:

1. Port only from the original repository. Never port from VCV Rack's Audible Instruments, which is GPLv3 and would contaminate transit's MIT/Apache-2.0 licensing.
2. No Mutable Instruments branding on the instruments. They get descriptive names of transit's own (as VCV did with Macro Oscillator, Resonator and Texture Synthesizer). Credit Émilie Gillet explicitly, with her MIT notice, in the about box, in each instrument's help entry and in a third-party license file.
3. Polyphony, the editor window and the rack are transit's own work. Polyphony is a voice allocator around N mono engines with a note-to-macro mapping (pitch, harmonics, timbre, morph). The editor follows the ownership rules of [ADR 0030](0030-editor-window-cohesion.md).

### Implementation

A Rust port in a transit-owned crate with a descriptive, brand-free name (the Plaits port is `transit-macro-osc`; see the amendment below). The shipped app is pure Rust.

Before any port starts, evaluate the existing crates.io port (`mi-plaits-dsp`) for license and fidelity. The evaluation is a prerequisite, not a decision: it informs whether to adopt, fork or write the port, and this ADR does not pre-empt it.

### Gate

Null tests against her C++, wrapped as a dev-only oracle. The oracle is built with the `cc` crate for tests and CI only and is never linked into the app.

- Same model, seeded noise, fixed parameter sweeps, compared sample by sample at 48 kHz with her block size.
- The residual floor is measured first, with FP contraction and fast-math off. The gate is set at that floor with margin. A residual above the floor is an escape.
- The port is tested at the mono-voice level. The allocator, the editor and the rack get ordinary tests.

## Consequences

- A coherent instrument section with one design language and low CPU.
- The DSP risk is retired by the oracle; the design work moves to composition and UI.
- Every ported module adds its own oracle build to CI.
- Third-party license text and credits become a maintained deliverable of each release that ships a port.

## Alternatives considered

- Generic wavetable and granular engines from scratch: months of sound design and no reference to test against.
- The C++ via FFI inside the app: adds a toolchain and an unsafe surface. Rejected; the C++ stays in the test oracle.
- The VCV Rack ports: GPLv3, which conflicts with transit's licensing.
- Shipping under the original names: contrary to the branding request.

## Amendment 2026-10-29: crate naming and the Plaits source decision

Maintainer decision, 2026-10-29. This amendment supersedes the crate naming in Implementation above and records the outcome of the crates.io evaluation (#193).

**(a) Crate naming.** The rule `transit-mi-*` is replaced. The eurorack README asks derivative works not to keep the original module names, so crate names are descriptive and brand-free: each names what the crate does, not whose module it came from. The Plaits port is the crate `transit-macro-osc` (macro oscillator). Later modules take names that describe what they do, for example Rings is `transit-modal-resonator`, not `transit-rings`.

**(b) Source decision (#193, 2026-10-29): option (b), vendor the MIT Rust port.** The port is `mi-plaits-dsp-rs` by Oliver Rockstedt (GitHub `sourcebox`), vendored at commit `effe9864bb` into `transit-macro-osc`. Its terms:

- Rockstedt's MIT notice stays in the source and the crate.
- Émilie Gillet's full MIT notice from `eurorack` and `stmlib` is restored in the crate `LICENSE` and in the third-party license file.
- The source commit (`effe9864bb`) is recorded in the crate and in the third-party file.
- Before any engine ships, each one is corrected against the C++ oracle (#194): per-instance RNG; sample rate as a parameter, not a constant; an audit of the interpolate clamps; the VA auxiliary oscillator phase returned to firmware behavior; and the snare and VOSIM details.

**(c) Fallback.** If the vendored code needs more repair than it saves, port from the firmware instead (option c), behind the same oracle gate. The null tests compare output sample by sample, so the swap is invisible to users.

Related: #193 (the evaluation and this decision), #194 (the C++ oracle that gates each engine).

## Implementation note: oracle

The Gate oracle is `crates/macro-osc-oracle` (`transit-macro-osc-oracle`, #194): the upstream C++ vendored unchanged, dev-only (`cargo xtask layers` rejects any normal or build dependency on it), with its FFI `unsafe` confined to `src/ffi.rs`; the sweep, the residual and the measured floor are in [gates.md, Oracle null tests](../contributing/evidence/gates.md#oracle-null-tests).

## Oracle deviation note 2026-10-30: the six-op engine after shared RAM

The firmware's engines share one 16 KB RAM arena (`Voice::Init` frees the allocator before each engine's `Init`), and the six-op engine keeps its staggered-render accumulator there without clearing it in `Reset`. On the module, the first block after the voice returns to a six-op bank from the string machine, speech, swarm, particle noise or inharmonic string engine (or the wavetable engine, on a 64-bit host only) therefore adds that engine's leftover bytes, read as floats, to the output: arbitrary values up to a NaN, which the C++ cast turns into a full-scale -32768 click. The port's engines own their memory, so the six-op engine replays its own last partial sum, which is the module's output whenever nothing overwrote it; the port is never louder than the module there. Won't fix: on the module, the first 12-frame block (0.25 ms) after returning to a six-op bank from one of six engines adds that engine's leftover memory to the output, which can be a click; transit plays the six-op engine's own tail instead, as the module does after any other engine. Reproducing it would mean emulating every engine's byte layout and the host's pointer size to replay garbage. The voice gate pins the difference to that one block (`six_op_reentry_after_a_ram_sharing_engine` in `crates/macro-osc/tests/voice_scenarios_vs_oracle.rs`); every other frame of every voice scenario is compared at 0 LSB. This is an oracle deviation, not a transit known issue: no user can hit it, since nobody compares the DAW with the module (#195).

## Implementation note 2026-10-30: the Macro Oscillator instrument (tuning, names, MIDI)

The first instrument of the family ships as the built-in "Macro Oscillator" (`macro_oscillator`, in `transit-dsp` beside the other built-ins, over `transit-macro-osc`; `transit-dsp` takes the same-layer dependency and is no longer standalone, #195).

- **Tuning.** The firmware computes pitch against its corrected rate (47872.34 Hz, the module's codec clock), so the module plays in tune while the same arithmetic at a true 48 kHz plays 4.6 cents sharp. The instrument plays concert pitch by default (A4 = 440 Hz): `Voice::set_tuning(Tuning::Concert)` scales every frequency the firmware derives from its corrected rate (the voice's `a0`, the six-operator voices' reference, the speech formants) by 47872.34 / 48000. The port plays at concert pitch; the firmware's arithmetic is kept bit-exact in the crate (`Tuning::Firmware`, the crate default) for the null tests, and the module itself plays in tune because its audio clock runs at the corrected rate. So there is no switch back to the firmware's pitch: at a true 48 kHz it would only detune the instrument by 4.6 cents while claiming to match the hardware (a "Hardware Tuning" parameter existed briefly on the slice 3 branch and was removed before merge; a session that stored it loads, and the open logs the ignored id).
- **Names.** The 24 models are named by what they do, in the firmware's order: Filtered analog, Phase distortion, Six-op FM 1 to 3, Wave terrain, String machine, Chiptune, Virtual analog, Waveshaper, FM, Grain, Additive, Wavetable, Chord, Speech, Swarm, Filtered noise, Particle noise, Inharmonic string, Modal, Bass drum, Snare, Hi-hat. No module or brand name appears in a UI string or identifier.
- **MIDI.** The held note is the voice's V/OCT input, a note-on raises its trigger and sets its level input from the velocity, a note-off lowers both so the low-pass gate's decay is the release. Mono, last-note priority, legato as the module's trigger input behaves: a note under a held one changes pitch with no new trigger. A new phrase whose note-on shares a 12-frame block with the last note-off gets one low block first, so the voice sees the edge. Sustain holds released notes; pitch bend spans a parameter range (2 semitones by default); the mod wheel adds to a chosen control (Timbre by default).
- **Blocks and output.** The voice renders 12-frame blocks (its envelopes are timed per block) whatever the host block, carrying rendered frames across host blocks, so any host block size renders the same bits; events take effect at the next 12-frame boundary. The int16 frames become floats at full scale = 0 dBFS, polarity restored and the firmware's +1 truncation offset removed (silence is exactly 0); the default level is -6 dB.
- **Credits.** Émilie Gillet's MIT notices (eurorack, stmlib) and Oliver Rockstedt's are in ATTRIBUTION.md, NOTICE and Help > About transit; the panel and its help entries credit the original; the user guide says it is a port, not an emulation of the hardware's analog stages.
- **Rack-ready, not racked.** One mono voice, MIDI in, one audio output on every channel, parameters addressed by automation id (`plugin:instrument:<id>`). Polyphony and unison are ADR 0034's racks (#196 onward).

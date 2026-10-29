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

A Rust port in a transit-owned crate, named `transit-mi-*` per the crate naming rule. The shipped app is pure Rust.

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

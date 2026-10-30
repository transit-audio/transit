# ADR 0037: An adaptable core: the desktop DAW is the product, the engine must stay fit for small dedicated hardware

- **Status:** proposed 2026-10-30
- **Date:** 2026-10-30
- **Reopen when:** a transit product decision adds a second product shape (a dedicated device, a headless service) that the desktop app would have to carry, a core crate needs the UI or plug-in hosting to work, or the small-box benchmark ([#237](https://github.com/transit-audio/transit/issues/237)) shows the core cannot hold its deadlines on the reference board at the track count the desktop app promises.

## Context

transit is a desktop DAW for Windows, macOS and Linux ([ADR 0032](0032-transit-ships-a-desktop-app-only.md)). Its core is layered and the layers are enforced: `time`, `rt`, the audio and MIDI I/O crates, `midi` (L0); `dsp`, the native instruments `macro-osc` and `resonator`, and the plug-in hosts (L1); `model` (L2); `mix` (L3); `engine` and `playback` (L4); `automation` (L5); `ui-studio` (L6) ([`layering.md`](../contributing/knowledge/layering.md), `cargo xtask layers`). The native instruments are deterministic: `forbid(unsafe_code)`, allocation at construction, allocation-free render, and bit-exact against the original reference ([ADR 0033](0033-built-in-instruments-mutable-instruments-port.md), [oracle null testing](../contributing/knowledge/oracle-null-testing.md)). The render runs on a bounded pool with per-strip deadlines ([ADR 0029](0029-strip-job-slots.md)).

On 2026-10-30 the maintainer observed that this core is what a dedicated instrument (a groovebox-class device) needs. What such a device does not need is arbitrary plug-in hosting, or the desktop UI. The observation is about the core's shape, not about a new product.

## Decision

1. **The product target is unchanged.** The product is the desktop DAW. A groovebox or any other dedicated device is not a transit product, and this ADR adds no product scope.
2. **Design principle: performance is measured by the useful musical work accomplished within the hardware available, not by how much hardware the engine can consume.** A change that makes the engine use more cores, memory or threads to do the same work is not a performance gain. The measure is the work done on a stated board, with the deadlines held.
3. **The core builds and runs without plug-in hosting and without the UI.** The core is the engine, mix, model, MIDI, dsp and the native instruments. A headless build gate proves it, and the gate is tracked as [#236](https://github.com/transit-audio/transit/issues/236).
4. **Reference board: Raspberry Pi 5.** Four Cortex-A76 cores at 2.4 GHz, 64-bit Raspberry Pi OS with the PREEMPT_RT kernel, active cooling, the performance governor, class-compliant USB audio, PipeWire. The loaded-run benchmark is recorded on this board for every minor release from v0.7.0, as tracked in [#237](https://github.com/transit-audio/transit/issues/237).
5. **Small hardware means a small Linux computer with `std`.** It does not mean a microcontroller. This ADR makes no `no_std` commitment.
6. **A design choice that ties the core to the UI, to plug-in hosting, or to unbounded memory or threads is flagged against this ADR in review.** The review cites this ADR and asks for the core to keep its shape or the choice to move out of the core.

## Consequences

- The headless gate and the Pi 5 benchmark become v0.7.0 work, with the control surfaces the milestone already carries.
- The arm64 run of the Pi 5 doubles as a second architecture for the bit-exact instrument tests. Floating-point defaults differ on arm64 (flush-to-zero and fused multiply-add behavior), so the oracle floor and the gate must be measured there too, not assumed from x86-64 ([oracle null testing](../contributing/knowledge/oracle-null-testing.md)).
- The README performance text cites the Pi 5 number once it is recorded, under the evidence-visible rule ([ADR 0029](0029-strip-job-slots.md) and the README's "How we decide").
- No product scope is added. The desktop app stays the only shipped build ([ADR 0032](0032-transit-ships-a-desktop-app-only.md)).

## Conventions

Ableton and Logic are desktop-only, so the desktop convention ([ADR 0034](0034-racks-layering-instruments-through-track-groups.md), [ADR 0035](0035-disk-backed-audio-sources.md)) does not say how a core should behave on small hardware. This principle is where transit can be clearly better: a measured core that is fit for a board with four cores and no screen is a property the desktop convention does not offer.

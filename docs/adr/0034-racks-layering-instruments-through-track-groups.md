# ADR 0034: Racks: layering instruments through track groups

- **Status:** proposed 2026-10-29
- **Date:** 2026-10-29
- **Reopen when:** zones per member are needed, groups cannot carry the macro model, or keyed plans move off the audio thread (#174) and change the render path.

## Context

The instrument family of [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md) is small and mono by design and is composed by patching. transit needs the equivalent: a way to layer several instruments and play them as one.

Convention (transit follows what Ableton and Logic agree on): Ableton's Instrument Rack and Logic's Track Stack (summing stack) both give a group a summed output, let several instruments receive the same MIDI, and expose a few macro controls.

## Decision

This is a design to be refined, kept short.

- A rack is a group track that sums its member tracks and can route the group's MIDI input to every member.
- Macro parameters on the group map to member parameters, reusing the range model of the MIDI Learn mappings ([ADR 0028](0028-midi-learn-mappings.md)).
- A member's own chain stays ordinary inserts.
- A rack serializes as a group with its routing. Nothing new is stored per instrument.
- The render path is the existing group sum. Keyed plans stay on the audio thread until #174.

### Open questions

- Key and velocity zones per member. Ableton has them and Logic does not, so defer.
- Macro curve mapping.
- A rack preset format, which ties to #191 (session bundles).

## Consequences

- Layering needs no new engine concept; every member remains a normal track with its own mixer strip, automation and chain.
- Macros add a mapping layer on the group that must be saved, loaded and undone like other mappings.
- No rack presets until the preset format question is settled.

## Alternatives considered

A dedicated rack device, one container plug-in holding the members. Groups win because they are already a model concept and every member stays a normal track.

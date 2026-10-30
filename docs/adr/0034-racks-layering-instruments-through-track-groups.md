# ADR 0034: Racks: layering instruments through track groups

- **Status:** proposed 2026-10-29
- **Date:** 2026-10-29
- **Reopen when:** groups cannot carry the macro model or the zones, or keyed plans move off the audio thread (#174) and change the render path.

## Context

The instrument family of [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md) is small and composed by patching; its instruments play polyphonically inside one instrument (the Macro Oscillator's voices, #196) but do not layer. transit needs the equivalent: a way to layer several instruments and play them as one.

Convention (transit follows what Ableton and Logic agree on): Ableton's Instrument Rack and Logic's Track Stack (summing stack) both give a group a summed output, let several instruments receive the same MIDI, and expose a few macro controls.

## Decision

This is a design to be refined, kept short.

- A rack is a group track that sums its member tracks and can route the group's MIDI input to every member.
- Macro parameters on the group map to member parameters, reusing the range model of the MIDI Learn mappings ([ADR 0028](0028-midi-learn-mappings.md)).
- A member's own chain stays ordinary inserts.
- A rack serializes as a group with its routing. Nothing new is stored per instrument.
- The render path is the existing group sum. Keyed plans stay on the audio thread until #174.
- Each member has a key zone and a velocity zone, with a fade range at each edge, as in Ableton's Instrument Rack (the maintainer chose Ableton's way here, 2026-10-29); a note goes to every member whose zones contain it, scaled by the fades. Logic's Track Stack has no zones, so this is the one place the rack goes past what the two agree on.

### Open questions

- Chain select zones (Ableton's third zone type, a macro that picks members): not in the first version.
- Macro curve mapping.
- A rack preset format, which ties to #191 (session bundles).

## Consequences

- Layering needs no new engine concept; every member remains a normal track with its own mixer strip, automation and chain.
- Macros add a mapping layer on the group that must be saved, loaded and undone like other mappings.
- No rack presets until the preset format question is settled.

## Alternatives considered

A dedicated rack device, one container plug-in holding the members. Groups win because they are already a model concept and every member stays a normal track.

## Amendment: slice 1, the model, the distribution and fade rules, macro curves (2026-10-30, #197)

**Context.** The decision above says a note is "scaled by the fades" without saying what the fade scales, and leaves macro curves open. Ableton's manual (Live 12, *Instrument, Drum and Effect Racks*, Zones) is explicit: "Key zone fade ranges attenuate the velocities of notes entering a chain", and velocity zone fade ranges do the same; only the chain select zones of an Instrument or Audio Effect Rack "attenuate the volume level at each chain's output". Logic's Track Stack (a summing stack whose main track plays every subtrack) has no zones and agrees on everything else: a summed output, one MIDI input for every instrument, a few smart controls.

**Decision.**

1. **Model.** A rack is a routing folder (`TrackKind::Folder`, made like Group Tracks makes one, with its own bus) with `Track::rack: Option<Rack>`: `members: Vec<RackMember { track, key_zone, vel_zone }>` and `macros: [Macro; 8]`. `Zone { low, high, fade_low, fade_high }` holds note numbers or velocities, inclusive, kept in order `low <= fade_low <= fade_high <= high`: full level from `fade_low` to `fade_high`, a ramp in each fade range. Every field has a serde default and the rack is not written for other tracks, so a build that predates racks opens the file with the folder as a plain group. A MIDI or instrument track that moves into the folder becomes a member with full zones; one that leaves is no longer one. At most 16 members play (the fixed table below).
2. **Sum.** The render path is the group's: members output to the folder's bus, the folder's strip reads it, and each member keeps its own inserts and fader; the rack's inserts and fader act after the sum. Nothing new is rendered.
3. **Distribution.** The rack's MIDI is its own MIDI clips and, while it is monitored or armed, its live input. A note-on reaches every member whose key zone and velocity zone both contain it. A note-off reaches a member only when that member holds the note (its on was delivered), so a zone edited while a note sounds never strands it and a member that did not get the on never gets the off. Pitch bend, pressure, controllers (the sustain pedal included) and program changes reach every member. The rack's clip notes are a source of their own in each member's note table (the clip port, another generation), so they never collide with the member's own clips. Live input is fanned out by the engine on the audio thread, per event, into a fixed array of at most 16 member events on the stack: nothing allocates or locks.
4. **Fades (the rule, following Ableton).** Both zone fades scale the note's **velocity**, never the member's level: within a fade of `n` steps, the value `k` steps from the edge gets the factor `(k + 1) / (n + 1)` (so a fade from 60 to 63 gives 60 to 63 the factors 1/4, 1/2, 3/4 and 1, never 0 inside the zone); the key and velocity factors multiply; the scaled velocity is rounded and never below 1. A level fade per chain cannot serve a polyphonic member playing notes in and out of its fade at once; a velocity fade can, and it is what Ableton does for these two zone types. A chain select zone's level fade stays out of scope.
5. **Macros.** Eight knobs, each mapped to any number of member parameters by automation address (an insert slot or the instrument, and the stable parameter id), a parameter following one macro at a time (Ableton's rule). A mapping carries ADR 0028's range (`min` and `max` in 0..1 of the parameter's normalized travel; inverted when `min > max`) and a **curve** in -1..1: the travel is `min + (max - min) * x^(4^curve)`, so 0 is linear, 0.5 squares and -0.5 takes the square root (Logic's Smart Control mappings offer such a response curve; Ableton's macro mappings are linear). The parameter's plain range (and taper) is captured at map time so the audio thread converts without the parameter table. A macro move (`rack.macro`) writes every mapped parameter's stored value, the values `mix.insert_param` and `mix.instrument_param` write, in one undo step (a drag is one merged step). A macro lane on the rack (`AutoParam::Plugin { slot: 253, param: "macro1".."macro8" }`, a plain slot so older builds read and ignore it) drives the mapped parameters on the audio thread, at block rate, while it has points. A macro learns a controller through MIDI Learn (`track:<rack>:macro:<n>`); the source is stored on the rack, never in `Session::midi_mappings`, so older builds still open the file.
6. **Undo.** Making a rack, adding or removing a member, a zone edit, a macro mapping edit, a rename and a macro move are each one undo step.

**Consequences.** Racks need no new render node and no new track kind. Recording on a rack (the rack holding takes), drawing and editing the rack's own clips in the arrangement, a rack's notes to an external MIDI destination of a member, the controller chase of the rack's clips, Touch and Latch writing of mapped parameters from a macro move, and chain select zones are left for later slices.

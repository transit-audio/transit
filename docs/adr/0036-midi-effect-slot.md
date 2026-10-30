# ADR 0036: MIDI effect slot

- **Status:** proposed 2026-10-30 (#201)
- **Date:** 2026-10-30
- **Reopen when:** hosted MIDI effects (CLAP note effects) are wanted, a generator needs more than the fixed carry, or keyed plans move off the audio thread (#174) and change where an instrument's events are delivered.

## Context

The Marbles and Grids ports (#202) generate notes and triggers, so they need somewhere to sit
ahead of an instrument. Users want the small note tools every DAW has. ADR 0034 does not describe
such a slot: a rack routes its MIDI to its members, but nothing transforms MIDI on the way.

Convention (transit follows what Ableton and Logic agree on): Ableton's MIDI effects (Arpeggiator,
Chord, Note Length, Pitch, Random, Scale, Velocity) sit before the instrument in a track's device
chain; Logic's MIDI FX slot sits above the instrument slot on the channel strip (Arpeggiator, Chord
Trigger, Modifier, Modulator, Note Repeater, Randomizer, Scripter, Transposer, Velocity Processor).
Both run a chain of MIDI effects before the instrument, in order, each one bypassable.

## Decision

- **Model.** `Track::midi_effects: Vec<MidiEffectSlot { plugin, params, bypass, state }>`, ahead of
  the instrument, at most 8 (`MAX_MIDI_EFFECTS`). Not written while empty (serde default), so older
  sessions load and files without effects stay as older builds wrote them. Each slot has a
  process-unique `uid` (not saved) by which the mix knows its chain. Instrument tracks only: a MIDI
  track's external output (#61) does not go through the chain.
- **Where it runs.** Every event the mix gives an instrument already leaves through one call of its
  note and pedal table (`InstrumentEvents::send`): live input, clips, the controller chase, and the
  releases of a seek, a Stop or a failure. While a track has MIDI effects that call hands the event
  to the chain's host (`crates/mix/src/midi_fx.rs`), which travels with the instrument instance.
  The table keeps matching sources, voice ids and releases on the input side; the chain sees the
  table's output, with the mix's voice ids.
- **Order and offsets.** Within a block the host collects the events and, right before the
  instrument processes, runs them through the effects in chain order. Each effect gets its input
  sorted by sample offset (stable: events at one offset keep the mix's order) and may move, add,
  remove or rewrite events, keeping state across blocks. Between blocks (Stop, seek, failure) an
  event runs through at once, at offset 0.
- **Deferral.** An event an effect writes at or past the end of the block waits in that effect's
  carry (a fixed buffer) and joins the next effect's input, or the instrument's, in the block it is
  due, ahead of that block's own events at its offset; an overdue one (a block the strip did not
  render) lands at offset 0. This is the chain's own deferred path, after the note table: the mix's
  deferred MIDI (#174) is before it and unchanged.
- **Bypass is transparent.** A bypassed effect is skipped: the instrument gets exactly the events,
  offsets and order it would get without it, bit for bit.
- **Edits end the chain's notes.** Adding, removing, moving or bypassing an effect changes what a
  note's release would pass through. At the first block after such an edit the host sends a note-off
  for every note it started on the instrument, forgets carried events, and swallows the releases of
  notes given to the chain before the edit. A parameter change is not an edit: an effect pairs its
  own notes (Transpose remembers each note's shift). Stop and panic do the same before the mix
  releases its sources. Nothing is left hanging; the price is that an edit cuts a held note.
- **Capacity and diagnostics.** A block takes at most 512 events into the chain (`MIDI_FX_BLOCK`)
  and each effect writes at most as many; each effect carries 64 (`MIDI_FX_CARRY`); the host tracks
  256 notes (`MIDI_FX_NOTES`). Past those, events are refused and counted in `MidiFxCounts`
  (`EventStatus::midi_fx`); the mix retries a refused release at the next block, and a release a
  full carry cannot hold is delivered at the block's last offset. Nothing allocates, locks or logs
  on the audio thread: the storage is made with the instrument's event state, and a built-in effect
  is a value (`transit_dsp::midi_fx::Builtin`) made in place.
- **The effect trait.** `transit_dsp::midi_fx::MidiEffect`: `process(input, out, frames)` over
  `TimedEvent { offset, PluginEvent }` into a fixed `MidiOut`, plus parameters, `reset` and the
  transport. The contract: pair every note-on it lets through, pass an unknown release unchanged,
  never write a note-on of velocity 0. Hosted MIDI effects can implement it later, made off the
  audio thread like any hosted plug-in; until then the slot accepts built-ins only and an id this
  build does not know passes events through.
- **Built-ins.** Transpose (`midi_transpose`: semitones, ±48; a note shifted off the keyboard is
  dropped with its off) and Velocity (`midi_velocity`: scale 0..400 %, offset ±127, minimum and
  maximum 1..127, a minimum above the maximum read the other way round). Their parameters are
  automated as `AutoParam::Plugin { slot: MIDI_EFFECT_SLOT + index }` (slots 240..247, `plugin:midi1:…`
  in ids), read at the block start; a lane follows its effect when it moves and goes when it is
  removed.
- **Undo.** Add, remove, move, bypass and a parameter edit are one undo step each
  (`mix.midi_effect*`); a parameter drag in the UI is one step.
- **UI.** The device chain shows the MIDI effects in their own group card ahead of the instrument,
  listed top to bottom in processing order (Logic's placement above the instrument, in our left to
  right chain), with add, remove, reorder (the row's menu), bypass and a row per parameter. The
  mixer strip marks a track with MIDI effects ("MFX", its chain on hover).
- **Racks (ADR 0034).** A rack's own MIDI effects run before its distribution to the members, and
  a member's own MIDI effects after it, ahead of its instrument (Ableton's Instrument Rack: MIDI
  effects before the rack, and in each chain before its instrument). The member half is what this
  slot already does, since a member is an ordinary instrument track. The rack half needs the rack
  (#197) on `transit`: the rack's clip and live events must run through the rack's chain once per
  block, before the zones split them, rather than once per member, so an effect with state (an
  arpeggiator, a generator) runs once. It lands with or after #197.

## Consequences

- MIDI effects need no new plan cell: the chain lives in the instrument's event state, so adding
  one does not remake the instrument, and a plan swap keeps the chain running.
- A chain edit cuts held notes. Known and documented; a smarter rule (keep notes an unchanged prefix
  of the chain started) can come later.
- The legacy audition call (`MixEngine::live_note`, superseded by `queue_event`) and an External
  Instrument's MIDI out do not go through the chain.
- Generators (#202) get a slot whose carry and transport are ready; one that holds more than 64
  pending events per block needs a larger carry.

## Alternatives considered

- **A wrapper plug-in around the instrument.** Every event path would pass through it for free, but
  a chain edit would remake the instrument's slot, and the wrapper would have to forward every
  `Plugin` method forever.
- **MIDI effects as plan cells like inserts.** Matches hosted MIDI effects best, but needs the
  compiler to move effect instances and attach them on adoption for built-ins that cost nothing to
  make. Revisit with hosted MIDI effects.
- **Transforming events before the note table.** The table's source and identity matching would
  then see transformed keys, and every path that releases a source (seek, revoke, Stop) would need
  the chain's mapping.

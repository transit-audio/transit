<!-- transit-index
title: MIDI effect slot: `Track::midi_effects` ahead of the instrument (at most 8, instrument tracks), run by a host in the instrument's event state on every event the mix sends it, in chain order with stable offset sorting; events past the block wait in each effect's fixed carry; bypass is bit-transparent; an add, remove, move or bypass ends the chain's notes; fixed capacities with `EventStatus::midi_fx` counters; built-ins Transpose and Velocity, automation slots 240..247; a rack's own chain runs before its distribution (lands with #197)
-->
# ADR 0036: MIDI effect slot

- **Status:** proposed 2026-10-30 (#201); amendments 2026-11-01 (release priority) and 2026-11-05 (the carry's own-note eviction)
- **Date:** 2026-10-30
- **Reopen when:** hosted MIDI effects (CLAP note effects) are wanted, a generator needs more than the fixed carry, keyed plans move off the audio thread (#174) and change where an instrument's events are delivered.

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
  process-unique `uid` (not saved) by which the mix knows its chain. Instrument tracks and racks (below): a MIDI
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
- **Bypass is transparent.** A bypassed effect is skipped: the instrument gets the same events,
  offsets and order it would get without it (tested against no chain). The paths below that refuse
  or drop events apply to a chain whose effects are all bypassed as well.
- **Edits end the chain's notes.** Adding, removing, moving or bypassing an effect changes what a
  note's release would pass through. At the first block after such an edit the host sends a note-off
  for every note it started on the instrument, forgets carried events, and swallows the releases of
  notes given to the chain before the edit. A parameter change is not an edit: an effect pairs its
  own notes (Transpose remembers each note's shift). Stop and panic do the same before the mix
  releases its sources. Nothing is left hanging; the price is that an edit cuts a held note.
- **Capacity and diagnostics.** A block takes at most 512 events into the chain (`MIDI_FX_BLOCK`)
  and each effect writes at most as many; each effect carries 64 (`MIDI_FX_CARRY`); the host tracks
  256 notes (`MIDI_FX_NOTES`). Past those, events are refused and counted in `MidiFxCounts`
  (`EventStatus::midi_fx`). The release paths:
  - a release the chain's input cannot take is refused to the mix, which retries it next block;
  - a release the instrument's own queue refuses is held by the host (up to 64, `MIDI_FX_RETRY`;
    past that it is lost and counted) and given again at offset 0 of the next block;
  - a release a full carry cannot hold evicts its own carried note-on, else the newest carried
    note-on, else the first carried non-release; a carry of releases alone delivers it at the
    block's last offset (amendment 2026-11-05);
  - a full effect output or carry merge evicts pending note-ons for releases (the #278 rule below).

  Transpose and Velocity reach none of the last two: each writes one event per event it takes and
  carries nothing. Nothing allocates, locks or logs
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
  (#197) on `transit`, and a design of its own, because the rack's events reach members two ways
  there: its clips are merged into each member's clip events in `schedule_notes` (once per member),
  and its live input is fanned out per event in `MixEngine::queue_event` before the block. A rack
  chain therefore needs its own stage (a host owned by the rack strip, not by an instrument) fed by
  both sources once per block: the rack's clip events and its live events. The live fan-out moves
  after that stage, into the block, so zones and fades apply to what the chain put out, and the
  members read the stage's output instead of the rack's clips. The stage runs before any member
  renders, on the thread that publishes the strip jobs, so members on render workers see its
  output for the block; an effect with state (an arpeggiator, a generator) then runs once per rack,
  not once per member.
- **Rack chain, first form (landed with the rebase onto racks).** A track that is a rack may hold MIDI effects
  (the device chain shows their card ahead of the rack card). Until the rack stage above exists, each member's
  host runs the rack's chain ahead of its own (the rack's, up to eight, then the member's, up to eight), after the zones:
  Transpose and Velocity are per-event, so every member hears the same result, and a rack note-off is still
  gated by the raw key the member holds. Differences from the stage design, to be removed by it: zones and fades
  see the notes before the chain, not after, and a stateful effect would run once per member.

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

## Note 2026-10-31: the first generator (#202)

The Reopen-when item "before the generators of #202 land" is answered for the Random Sequencer
(`random_sequencer`): it keeps its own schedule in beats and writes only events due in the block
(at most a few per tick: two gate channels' starts and ends, the clock's), never one at or past the
block's end, so it uses no carry; it passes its input through one for one (note-ons dropped unless
Thru). Dense patterns, large host blocks, high tempos, and low sample rates can exceed the 512-event output buffer; #278 below defines its overflow policy.
It needs the session's sample rate to turn beats into offsets: `MidiEffect::set_sample_rate`, given
with the transport at the start of every block. A chain edit or a Stop resets it with the other
effects (its notes are ended by the host); its next playing block restarts it on the grid.


## Amendment 2026-11-01: release priority (#278)

The slot uses option (b): a full 512-event output, including the merge with due carried events,
admits a release by evicting the newest pending note-on, counted as `evicted_ons` and never emitted;
a new note-on is refused and counted as `refused_ons`. The existing `overflow` total counts both,
plus discarded non-note events. Releases include note-offs, CC 120/121/123 and pedal-up messages
(CC 64/66/67/69 below 64). After all pending note-ons are gone, a release evicts another non-release;
if all 512 entries are releases, they coalesce into All Sound Off, All Notes Off, Reset All
Controllers and all four pedal releases per affected channel, at the greatest release offset,
and that buffer refuses further non-releases. The host then emits explicit note-offs for every
accepted sounding voice and the safety controllers directly at the block's last offset, then resets
the effects and carries: CLAP note-only
ports and VST3 instruments without controller mappings may ignore the channel messages. An on
that the 256-entry sounding-note table cannot track is refused before delivery and counted.
This last resort is counted separately as
`coalesced_releases`, never as refused offs: at most 112 channel messages (16 channels times seven
controllers) represent any release flood in the slot's output buffer, with individual release timing
sacrificed. The recovery then adds its own direct events, so the worst case per instrument per block
is 64 retried releases + 512 delivered events + 256 explicit note-offs + 112 safety controllers = 944
events, which fits the instrument's 1,024-event queues. Within the slot's output buffer the invariant
is that every note-on it emits is followed by its note-off within the same or a later block, overflow
or not (the carry path holds the same ordering since the 2026-11-05 amendment, #281); on release-only
saturation the host's explicit offs supply that termination (`released_on_overflow`). This assumes
the effect keeps
its pairing contract and the instrument accepts its releases (the existing downstream retry limit
is unchanged). Two monotone victim cursors and index links make admission/eviction O(1) amortized
per event and retain write order at equal offsets; traversal is O(512) per stage. Storage is made
with the host, with no audio-thread allocation or lock. Non-overflowing event streams are unchanged.

Known limits: a stray release (one whose On was already evicted) still evicts an unrelated pending On,
and the 256-entry table admission guard turns any leak in the sounding-note table into refused
Ons (silence until an edit or Stop). No leak path is known: the mix converts CC 120 and 123 into
per-note offs, and offs are never refused.

## Amendment 2026-11-05: the carry's own-note eviction (#281, PR #337)

When a full carry cannot hold a release, the release evicts its own carried note-on when there is one (the newest carried on with the same channel and key, and with the same voice id when the release carries one; both go, since the note never sounded), otherwise the newest carried note-on of any channel or key (that note's later off then arrives without an on, which an instrument with no sounding note for it ignores), otherwise the first carried non-release; a carry of releases alone delivers the release at the block's last offset. The carry path now shares the output buffer's guarantee: no off reaches the instrument before its own on. The newest match is the own on because, within a stage, the carry keeps emission order, so a later on with the same key cannot sit before an earlier one. The rule is guarded by `a_release_in_a_full_carry_never_puts_its_off_before_its_on`, `the_fuzz_seed_71271_through_two_delays_never_delivers_an_off_before_its_on` and `a_same_key_release_without_a_voice_id_evicts_its_newest_on_not_the_oldest`. This was a community contribution (PR #337), with a one-word fix-up by the maintainer (the lookup takes the newest match, not the oldest).

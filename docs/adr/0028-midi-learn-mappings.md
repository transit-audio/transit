# ADR 0028: MIDI Learn mappings

- **Status:** accepted 2026-10-25 (decided by Derek for v0.3.0, #97)
- **Date:** 2026-10-25
- **Reopen when:** a controller needs 14-bit CC pairs, NRPN, relative (endless encoder) CCs or LED feedback; or mappings must follow a track across sessions by name rather than by position (slice 2 and later).

## Context

The only learn features today are the step-input bindings and the MIDI Monitor's Learn spotlight. Nothing maps a controller knob, fader or button to a mixer control (#97). Slice 1 covers the mixer and track controls: fader, pan, mute, solo, record arm and send level. Plug-in parameters (with automation write) are slice 2, transport and global actions are slice 3.

## Decision

1. **A mapping is `source -> target` plus how to apply it.** Source: input port name (`PortKey` display string), channel (0 to 15, or any), kind (`cc` or `note`) and number. Target: a control id, `track:<id>:volume|pan|mute|solo|arm|send<slot>` (session) or `strip:<n>:...` (profile, see 3). Mode: `pickup` (default) or `jump`. Range: `min` and `max` in 0..1 of the control's own travel (fader position, balance, send position); a knob at 0 means `min`. Buttons carry `toggle` (default true: note-on or CC rising through 64 flips; false: momentary, follows note on/off or CC >= 64).
2. **One source drives one target, per store.** `map` replaces any earlier mapping of the same target or of the same source in that store.
3. **Two stores.** The **controller profile** is per machine, saved in `ui.json` (`midi_profile`), keyed by port name; because track ids belong to one session, profile targets address a strip by position (`strip:3:volume` is the third track in session order). **Session mappings** are `Session::midi_mappings`, saved with the session, addressing tracks by id. Both load with none when the field is absent. At resolve time a session mapping wins over a profile mapping that shares its source or its target; the shadowed one stays stored and is shown as "overridden".
4. **Realtime contract.** Main resolves both stores into an immutable `Snapshot` (entries keyed by the ingress input id of the open port) and swaps an `Arc<Snapshot>` into the ingress worker through the existing update channel; the worker never builds, locks or frees one (Main keeps replaced snapshots until the worker's reference is gone). Per admitted message the worker scans the snapshot, with no allocation or lock, and publishes a `MapEvent` (`Copy`) to Main through a preallocated SPSC ring; it applies pickup and jump itself, from a per-entry atomic cell holding the control's last value (Main refreshes the cells once a frame). Main applies each event with the same setters a mouse drag uses. Matching runs on every open input port, routed or not, without popping unrouted ports (a cursor like the monitor's), so a controller no track listens to still maps. With no mappings and no learn armed the worker does nothing extra. **Learn state is Main-side only:** Main arms one atomic flag; the worker forwards exactly the next CC or note-on as a candidate (it clears the flag with a swap) and keeps no learn state.
5. **Undo.** A mapped move of a control is applied through `execute_merged`, keyed per control, and the gesture ends after 400 ms without a move (UI clock, injectable), so a sweep of a fader is one undo step like a mouse drag. Session `midi.map` and `midi.unmap` are ordinary undoable commands. Profile changes are per machine, like the I/O map: they neither dirty the session nor make an undo step.
6. **Commands.** `midi.learn {target, scope?}` arms (the next candidate becomes a `midi.map` with defaults), `midi.learn_cancel`, `midi.map {source, target, mode?, min?, max?, toggle?, scope?}`, `midi.unmap {target, scope?}`, `midi.mappings` (query). Learn arming itself is not an edit.
7. **Out of scope:** plug-in parameters and automation write (slice 2); transport, cycle, metronome and MMC (slice 3); relative encoders, 14-bit CC, NRPN, LED feedback.

## Consequences

- A hosted or built-in controller that wiggles a CC also reaches any route that admits it (routing is unchanged); mapping a CC that a track also plays is the user's choice.
- Ports are opened while any mapping or learn exists for them (as the monitor does), so mapping opens input ports that no route would.
- A profile mapping to a position follows whichever track sits there; moving tracks changes what the knob moves.
- Sessions saved with mappings load in older builds with the mappings ignored (no `needs` marker: nothing plays wrong).

## Alternatives rejected

- **Match on the UI thread.** Adds a frame of latency and drops fast sweeps at the tap ring's size.
- **Build the table in the worker.** Allocates on the worker and needs session access.
- **Target tracks by id in the profile.** Ids are meaningless in another session.

## Related

- Issue #97; ADR 0022; `docs/contributing/knowledge/midi-learn.md`, `midi-monitor.md`, `audio-thread.md`.

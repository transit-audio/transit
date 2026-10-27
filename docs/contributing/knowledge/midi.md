# MIDI input

Decision: [ADR 0012](../../adr/0012-midi-device-layer.md); clocks: `clocks.md`; sending to a
hardware port: `midi-output.md`.

## Path

1. **Device layer** (`crates/midi-io`): the WinMM backend (via midir), a ring buffer
   (`ring.rs`), and a fake backend (`fake.rs`) for tests. Callbacks stamp packets in backend time.
2. **Ingress** (`crates/playback/src/player/ingress.rs`): maps packets to host time and delivers
   them to the audio side through the action lane. It must never block.
3. **Delivery** (`crates/playback/src/player/audio/midi.rs`): an inbox holds future events in device
   time until their render block exists. `RELEASE_RESERVE` keeps room so note-offs and pedal-ups are
   admitted even when the inbox is nearly full: dropping a release causes a stuck note.
4. **Mix** (`crates/mix/src/events.rs`): events become instrument events with sample offsets.

## Conventions

- Sustain pedal is CC 64; pedal edges must not be reordered before the note events they follow
  (timing-mode switches are guarded in `midi.rs`).
- Tombstones: `crates/playback/src/player/sources.rs` validates a source (port plus generation) again
  just before dispatch. A local tombstone records a revoke so a stale read of the shared admission
  flag cannot resurrect a revoked source; a late event from it is discarded.
- Step entry taps the same stream from the UI side (`crates/ui-studio/src/keys/step.rs`). The tap
  is control-rate: the ingress worker feeds it whenever a route admits the message, before the
  action lane's credit check, so a message the lane sheds still reaches Step entry (#74). A shed
  pedal-up therefore cannot leave the pedal held. Of the messages a route admits, only a full tap
  ring drops any, and then Step entry gets a Reset. Shedding a release also invalidates the
  route's permit (Main rebuilds it and cancels what the old route held), so that case sends a
  Reset too. Messages from revoked or stale permits, muted ports, consumed MIDI Learn input and
  `consume_notes` never reach the tap.

## Invariants

Stuck notes and dropped note-offs are merge-blocking defects. Timing may be flagged approximate
(`clocks.md`) but must never be silently wrong. The device callback may allocate or lock inside the
OS library (known issue K4); our code on that path may not.

Tests: `crates/midi-io/src/tests.rs`, `crates/playback/src/player/ingress/tests.rs`,
`crates/playback/src/player/audio/midi/tests.rs`, `crates/engine/tests/midi_input.rs`.

## Step entry controls (#92)

The pending chord lives in `engine::step_input`, driven by monotonic UI milliseconds.
A Tie press while notes are held adds one grid unit to the current preview and freezes
its hold timer until the last note releases. A separate Tie with no held notes extends
the chord ending at the cursor. The combined Tie/Rest resolves on press: held notes
mean Tie, no notes mean Rest. Fresh preferences bind sustain (CC 64) to the combined
action; saved MIDI bindings retain their assignments. Separate Tie, Rest and Back
remain learnable in the Step settings. A pedal pressed
before replaying a chord retains the earlier matching-pitches tie behavior.

While the pedal is held (tie mode, #138) each strike adds exactly one grid unit and
advances the step by one, however long the key is held: the hold repeater is suspended
for that strike. The mode is fixed when the strike begins, so releasing the pedal
mid-hold neither adds a rest nor re-enables the repeater for that strike; the next
strike with the pedal up lengthens by the ramp as usual. A pedal press with nothing
held waits for its release, then inserts one rest of the grid value (a held pedal
repeats rests on the ramp); a key that arrives first turns the press into a tie, so
the release inserts nothing (#139). The release decides because a press may still
become a tie, and a tap of any length, even a down and up in one UI frame or the same
millisecond, yields exactly one rest.

Hold to lengthen defaults on. The timer reaches unit 2 at the threshold (500 ms), then
each further unit takes a shorter interval: 400 ms easing linearly down to the floor
over `ramp_units` units (default 6: 400, 367, 334, 300, 267, 234 ms), then the floor
(default 200 ms, five units a second) per unit. A prefs file with no `ramp_units` and a floor of exactly 80 (every earlier build saved that default whole) migrates on load to 200; any other floor is kept as the user's choice.
Pedal rests use the same `HoldPrefs::units` schedule. Turning hold to lengthen off leaves each new note at one unit until
Tie taps extend it; Rest's hold/repeat schedule is unchanged. Preferences are saved
in KeysPrefs, not the musical document. Enter is the default computer-key stand-in;
Space remains Play/Stop unless explicitly rebound to Tie/Rest, effective only in
the focused piano roll during step entry. Computer-key repeat is ignored so the
same hold timer drives MIDI and keyboard controls.

Each tap entry carries the `Instant` the ingress worker saw it, and holds, taps and pedal timing
are measured from that stamp rather than from the UI drain, so a late frame cannot lengthen a
hold or turn a tap into a ramp. The fast pedal-tap miss (#139) is not yet reproduced: the engine
and the UI drain insert one rest for a tap of any length in tests, so the issue stays open for a
hardware retest.

Repaint while Step is on (#144): the ingress worker cannot wake egui, so the UI polls the tap
ring. `step_repaint_delay` in `keys/step.rs` is the one rule. A pending chord or rest, a held key
or a held pedal repaints at full rate (16 ms, for the hold ramp display); otherwise Step on polls
every 30 ms instead of repainting at display rate. Step off, or the piano roll hidden, leaves the
idle policy (1 s) alone. Taps and holds are timed from the worker's stamp (#136), so the slower poll
delays pickup by at most 30 ms and skews no timing. egui deducts one predicted frame from a
requested delay, so a frame test reads about 13 ms at 60 Hz for the 30 ms poll.

A held pedal cannot be stranded (#134): the ingress worker sets `tap_lost` (the step editor
then gets a `Reset`) whenever a route stops admitting, whatever the cause: the port goes away,
the storm detector mutes it, Main drops a permit or rebuilds the routes without it (disarm,
monitor off), or a channel filter change rewrites a route. One edge check in `Pump::step`
(admitting, then not) covers every path; the route-update comparison covers a route that is
replaced while others stay. A rebuild that keeps the same routes sends nothing.

`event.step_input` rejects a `duration` above 64 whole notes (245,760 ticks) with a command error.
Opening a different clip in the piano roll clears Step state entirely: Step turns off and the
pending chord, pedal and learn state are discarded (the live keyboard keys are kept), so re-enable Step on the new clip.
A step longer than the cap, such as a held chord, is refused with a status message rather than clamped.

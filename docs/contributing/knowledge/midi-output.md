# MIDI output

A MIDI or instrument track can send its MIDI to a hardware or virtual port instead of its hosted
instrument (#61). Input side: `midi.md`; clocks: `clocks.md`; device layer:
[ADR 0012](../../adr/0012-midi-device-layer.md), decision 5.

## Model and command

- `Track::midi_output` (`crates/model/src/midi_output.rs`): `Internal` (the default, not written to
  the file) or `External { port, channel, offset_ms }`. The port is stored by backend, name and
  ordinal plus the backend's stable id when it has one (WinMM interface path). Channel is
  `as_recorded` or 1 to 16. The offset is clamped to ±500 ms when read, since files are hostile.
- `midi.output` (`crates/engine/src/cmd/midi_output.rs`): one undo step for any number of tracks;
  omitted fields keep their values. UI: the "Out" button at the end of a MIDI strip's input row
  (`crates/ui-studio/src/midi_output.rs`). The arranger track header shows the same destination as a compact chip (state dot, port and channel; red when the device is missing, label hidden at narrow widths) that opens the same picker (#69).

## Routing rule: instead of, not in addition to

An external destination replaces the hosted instrument for that track. Clip notes and controllers
go to the port; the instrument receives only note-offs, so switching destination mid-note cannot
leave a voice stuck. Live input on an armed or monitored track is echoed to the port and does not
reach the instrument, but it is still captured for a MIDI recording, so a MIDI track with no
instrument can record. Track mute and solo silence the port (note-offs still pass). Track
automation lanes do not generate MIDI CCs today, so there is nothing of theirs to forward; clip
controllers (CC, pitch bend, pressure, program) are sent.

## Threads

| Thread | Does | Never |
|---|---|---|
| Audio callback (`player/audio/external.rs`) | Per block, visits the clip events of external tracks (`soundcraft_mix::external_midi_events`) and pushes stamped copies into a preallocated SPSC ring | Calls into `transit-midi-io`, allocates, locks, blocks |
| MIDI ingress (`player/ingress.rs`) | Pushes live input of echo routes into a second ring as soon as it pops it | Waits on the output thread |
| MIDI output (`player/midi_out.rs`, `transit-midi-io` sends) | Owns every `OutputHandle`; sorts events by deadline; sends; tracks held notes; polls ports | Runs on an audio thread (counted in `audio_thread_sends`, tested) |
| Main | Sends the destination table on each `Player::service`, the panic on Stop and All Notes Off; reads the status | Sends MIDI |

Ring full: the event is dropped and counted; the last 256 slots are kept for releases.

## Timing model

For each block about to render, the audio thread takes the block's first render frame, maps it
through the converter phase to an output-device frame (`Phase::lead`), and maps that frame to host
time H through the device clock map (`ClockMap::frame_to_host`, ADR 0017). While no map is valid,
it falls back to the host time at callback entry plus the frames between, which is early by the
device's output latency. The mix holds every software path back by its delay compensation A
(the plan's device latency: Main's arrival plus the master inserts, `MixEngine::latency`), so a
bare MIDI-out track's `H(block)` also adds `A × ns_per_session_sample`: its MIDI for a sample goes
out when the mix plays that sample, and a synth heard outside the plan stays with the delayed mix
(ADR 0020 decision 2; before, a MIDI-out track was early by A whenever a plug-in added latency). A
track that holds an External Instrument does not add A (`external::Block::host_for`): its return
comes back through the plan, which compensates it. Each event gets
`H(block) + offset_in_block × ns_per_session_sample` at the current speed. The output thread
sends it at `H + destination offset`.

- **Deadlines never go backwards per track.** A is read on every block, so it can drop while a
  note sounds (a latent insert removed or bypassed, a manual latency lowered, another External
  Instrument's port lost). A note-off stamped with the new A could then fall due before its
  note-on stamped with the old one, and the synth would hold the note. The output thread clamps
  each playback event's deadline to the latest one already queued for that track
  (`Track::last`; equal deadlines keep arrival order), so for at most the drop in A events bunch
  at the old instant instead. Notes that fall inside that window right after A drops are held to
  the previous deadline and may be shortened. Stop, the panic and a destination change reset it; a
  wrap or locate release sets it to the release instant, so the next lap goes out on time. The
  clamp never holds an event more than 2 s past now (`CLAMP_CAP_NS`), so one bogus far-future
  stamp cannot stall a track.

- The audio is rendered ahead of presentation, so events usually arrive at the output thread
  before their instant. A negative offset larger than that lead cannot be honored: the event is
  sent at once and counted late.
- The output thread sleeps until 0.3 ms before the next deadline (high-resolution sleep, at most
  1 ms at a time, which also bounds echo polling) and yields for the rest. Expected thread jitter
  is well under 1 ms on an idle machine; the OS MIDI driver and USB add their own. A send more than
  2 ms late is counted (`late`, `max_late_ns`, in the Out button's tooltip). Hardware accuracy is
  not established by the fake-backend tests; it needs Derek's hardware check.
- Echo ignores the offset and goes out at once. Its latency is the ingress worker's wake plus the
  output thread's poll. The ingress worker parks without polling and the input callback unparks it
  after publishing (#62), so only the output thread's 1 ms loop remains. The worker records
  callback-to-pop latency (`Counters::latency_last_ns`, `latency_max_ns`).
- With no external destination the thread parks between port polls (4 per second, for the
  picker's list) instead of waking every millisecond.
- The output thread runs at normal priority; promoting it is a follow-up if jitter under load
  matters.

## Safety

- **Stop** (the audio thread's transition, plus a direct panic from `Player::stop`): scheduled
  playback is dropped; every held note gets a note-off, held pedals go up, then Sustain off
  (CC 64 = 0) and All Notes Off (CC 123) on every channel the port was used on.
- **Loop wrap and locate** (#47's rule): at the jump instant (plus A on a bare MIDI-out
  destination, as its notes carry it, plus each destination's offset) the notes playback holds end; events scheduled at or after it are dropped (a locate discards its
  rendered-ahead block); the new lap or position starts again every note that sounds across it
  (`chase`). The same chase runs at play start.
- **Destination change** (port or channel, or back to internal): the track's held notes end on the
  old port, then Sustain off and All Notes Off on the channels that track used.
- **Echo route revoked** (disarm, monitor off, a MIDI input or channel filter change, the input
  controller unplugged): the key-up can no longer be echoed, so the output thread ends every note
  and pedal the echo holds for that track (note-offs, Sustain off), as revoking the source does for
  an instrument. `Native::service` sends `release_echo` for each revoked echo route; the
  destination's `echo` flag (armed or monitored) also releases when it goes false, and while it is
  false a late echo may only end notes, never start them.
- **Device loss**: any `PortWatcher` poll (the output thread polls every 250 ms) flags the handle;
  the port is closed and marked missing, held state is forgotten and sends are counted as unsent.
  When the port appears again it reopens and first gets Sustain off and All Notes Off on its used
  channels. A used port that is missing or failed to open is retried on every poll, so a port an
  old output thread still held (stuck in a driver at a device reopen) opens once it lets go.
- **Exit**: dropping the Player stops the thread, which releases everything on every open port; the
  drop waits up to 500 ms for that and never joins a stuck driver.

## External Instrument (ADR 0020)

An instrument track's instrument slot can hold an External Instrument (`external.instrument`,
`Insert::external`): a hardware synth played over MIDI, its audio returned on an input path. The
MIDI side is this page's destination, unchanged: `Track::midi_output` holds the port and channel,
and every safety rule above applies. The device holds the return path, a gain on the return and
the latency `manual_ms` (slice 1: set by hand on the card; Measure and a per-machine table come in
slice 2).

- **One timing knob.** While the device is on the track, the destination offset is held at 0:
  `instrument.external` writes 0, `midi.output` refuses a nonzero offset there, and playback reads
  `Track::midi_out_offset_ms` (0 on such a track) even from a hand-edited file. The Out button's
  offset row reads "Latency is set on the External Instrument".
- **Compensation, not early sends.** The device track's notes go out at P(R(S)), the stamp
  above without A (ADR 0020 decision 2: offset 0, not early, not delayed). The strip reads the
  return from the live input and reports C = round(manual_ms × r / 1000) frames at its head, where
  a latent instrument's latency counts, so delay compensation aligns the return at Main like a
  hardware insert's round trip (`crates/mix/src/external.rs`). C is part of A, so the rest of the
  mix (and every bare MIDI-out track) waits for the synth. Were the device's notes delayed by A
  too, the synth would land late by A and raising `manual_ms` would only delay the rest of the
  mix.
- **Missing states.** No audio device, no live input (not duplex), a return path this interface
  does not have, no port chosen, a port the output thread reports missing or failed, or no MIDI
  runtime at all: the strip is silent and reports 0, so compensation does not move. The Player
  follows the port's state (`Player::service`, `follow_external_ports`) and prepares a new plan
  when it changes. The card
  says which: "No audio device", "MODEL D missing: notes are not sent", "Return In 1 is not on
  this interface", "Needs a duplex device", "Not measured: notes sound late by the round trip",
  else "Manual: 19.5 ms".
- **Renders.** The synth only sounds in real time. An offline bounce, stems or bounce by path
  with an unfrozen External Instrument is refused unless the caller passes `without_external`
  (the studio's Bounce dialog asks: "Render without it" or Cancel; the status names the tracks
  left out). Freeze, flatten and bounce in place of the device track itself are refused until
  real-time render lands (slice 3).
- **Live playing.** Echo goes out at once; the return is monitored through the strip with no
  added delay, transport running or not.

## Persistence

Saved by port name and id. A session opened where the port is absent shows "device missing" on
the strip's Out button (red, with a warning icon and tooltip) and sends nothing; it connects when
the port appears. A port of another backend (a session from another OS) stays missing. The track
header indicator is a follow-up (`TODO(#61)` in `midi_output.rs`): the header lives in
`arrange.rs`.

## Not in scope

MIDI clock and MTC out, transport messages, SysEx, program change management.

Tests: `crates/playback/src/player/midi_out/tests.rs` (fake backend: order and offset, thread
identity, Stop, wrap, destination change, loss and reconnect, missing at load, echo, exit),
`crates/playback/src/player/device/external_tests.rs` (through the Player: a looped clip to the
port, no allocation in the callback, live echo without an instrument), `crates/mix/src/external_midi.rs`,
`crates/engine/tests/midi_output.rs`, `crates/model/src/midi_output.rs`, `crates/ui-studio/src/midi_output.rs`.
External Instrument: `crates/mix/tests/external_instrument.rs` (alignment at Main, bit exact; the
missing states), `crates/mix/src/external.rs`, `crates/playback/src/player/audio/external.rs`
(the send chained to the plan: the stamp External pushes, read back as a frame, feeds the fake
return, so the onset lands on the click; a bare track behind a latent master insert),
`crates/playback/src/player/audio/tests.rs` (`external_midi_stamps_move_with_the_mixs_delay_compensation`,
`an_external_instruments_notes_are_not_stamped_with_the_mixs_delay`, fake clock), the
deadline-order and per-kind release tests in `crates/playback/src/player/midi_out/tests.rs`, the
missing-port, failed-port and no-MIDI tests in `crates/playback/src/player/device/external_tests.rs`, `crates/engine/src/cmd/external_instrument.rs`,
`crates/ui-studio/src/external_instrument.rs`.

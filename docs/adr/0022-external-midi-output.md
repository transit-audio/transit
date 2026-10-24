# ADR 0022: External MIDI output: a per-track destination, sent by a dedicated thread on the output presentation map

- **Status:** accepted 2026-10-24 (retroactive; decided 2026-10-23 in #61, ingress wake in #62; stamp amendment from ADR 0020, issue #71)
- **Date:** 2026-10-24
- **Reopen when:** a hardware measurement shows the output thread's jitter at normal priority gets in the way (promoting it is the noted follow-up); MIDI clock, MTC, transport messages or SysEx are taken up (all out of scope here); a destination needs more than one port per track; or the deadline clamp (decision 6) shortens notes often enough in practice to need another design.

## Context

A MIDI track existed and the device layer (`transit-midi-io`, [ADR 0012](0012-midi-device-layer.md) decision 5) could open output ports, but nothing in the app used them: a track's clip events only reached hosted instruments (#61). Playing a hardware or virtual synth needs three things at once: a per-track place to say where the MIDI goes, a send that lands on the audio the listener hears, and note safety, because a note-on without its note-off leaves a hardware voice sounding.

Two constraints came from the engine. The audio callback must never call into the MIDI layer (`docs/contributing/knowledge/audio-thread.md`). And the device clock maps of [ADR 0017](0017-clock-contract-annex.md) give the host time at which an output frame is presented, which is the instant a MIDI send has to match.

The input side had a related fault (#62): the ingress worker waited with `park_timeout(1 ms)`, which Windows rounds up to the system tick (about 15.6 ms), so live input, and with it the echo path of this feature, could arrive a tick late.

## Decision

1. **A destination is a field of the track, not a device.** `Track::midi_output` (`crates/model/src/midi_output.rs`) is `Internal` or `External { port, channel, offset_ms }`.
   - `Internal` is the default and is not written to the file.
   - The port is stored by backend, name, ordinal among same-name ports, and the backend's stable id when it has one (the id takes precedence when nonempty).
   - `channel` is `as_recorded` or a channel 1 to 16.
   - `offset_ms` is clamped to +-500 ms (`MAX_MIDI_OUT_OFFSET_MS`) when read, because session files are not trusted.
   - One command, `midi.output` (`crates/engine/src/cmd/midi_output.rs`), sets it for any number of tracks as one undo step; omitted fields keep their values.
2. **An external destination replaces the hosted instrument; it does not add to it.** Clip notes and controllers go to the port, and the instrument receives only note-offs, so switching destination mid-note cannot stick a voice. Live input on an armed or monitored track is echoed to the port and is still captured for a MIDI recording, so a MIDI track with no instrument can record. Track mute and solo silence the port; note-offs still pass.
3. **A dedicated output thread owns every open port; the audio thread never touches the MIDI layer.** `crates/playback/src/player/midi_out.rs`:
   - Two preallocated lock-free SPSC rings (`rtrb`, 4096 events each) feed it: *playback* (the audio thread) and *echo* (the MIDI ingress worker, which forwards live input as soon as it pops it, never through the audio thread).
   - The audio thread pushes stamped copies of clip events plus `Release` (wrap, locate) and `Stop` markers. A push is wait-free and allocates nothing. A full ring drops the event and counts it; the last 256 slots are reserved for releases.
   - Main sends the destination table on each `Player::service`, and the panic (Stop, All Notes Off), through a bounded control channel, and reads a status snapshot. Main never sends MIDI.
   - The thread sorts events by deadline, sends, tracks held notes and polls port health every 250 ms. With no external destination it parks between polls (four per second, for the picker's port list).
   - "Never on an audio thread" is a tested invariant: sends made while the thread is marked as an audio thread are counted (`audio_thread_sends`) and must stay 0.
4. **Events are scheduled on the output presentation map.** For each block the audio thread takes the block's first render frame, maps it through the converter phase to an output-device frame, and maps that to host time H with `ClockMap::frame_to_host` (ADR 0017). Each event gets `H(block) + offset_in_block x ns_per_session_sample` at the current speed, and the thread sends it at `H + offset_ms`. While no map is valid the stamp falls back to the host time at callback entry plus the frames between, which is early by the device's output latency.
   - Audio is rendered ahead of presentation, so events normally arrive before their instant. A negative offset larger than that lead cannot be honored: the event goes out at once and is counted late.
   - The thread sleeps until 0.3 ms before the next deadline (high-resolution sleep, at most 1 ms at a time) and yields for the rest. A send more than 2 ms late is counted (`late`, `max_late_ns`, shown in the Out button's tooltip). The thread runs at normal priority.
   - Echo ignores the offset and goes out at once.
5. **Note safety is the output thread's job and covers every way a note can lose its end.** Full rules in `docs/contributing/knowledge/midi-output.md`.
   - *Stop:* scheduled playback is dropped, every held note gets a note-off, pedals go up, then Sustain off (CC 64) and All Notes Off (CC 123) on each channel the port used.
   - *Loop wrap and locate (#47's rule):* at the jump instant plus the destination's offset, held notes end, later scheduled events are dropped, and the new lap starts again every note that sounds across it (`chase`). The same chase runs at play start.
   - *Destination change:* the track's held notes end on the old port, then Sustain off and All Notes Off on the channels it used.
   - *Disarm and echo route revoked* (monitor off, input or channel filter change, controller unplugged): the thread ends every echoed note and pedal for that track (`release_echo`); while the route is off, a late echo may only end notes.
   - *Device loss:* the port is closed and marked missing, held state is forgotten, sends are counted unsent. When the port returns it reopens and first gets Sustain off and All Notes Off. A used port that is missing is retried on every poll.
   - *Exit:* dropping the Player stops the thread, which releases every open port; the drop waits up to 500 ms and never joins a stuck driver.
   - A session opened where the port is absent shows "device missing" on the Out button and sends nothing; it connects when the port appears.
6. **The stamp carries the mix's delay compensation for a bare track, and each track's deadlines never go backwards** (amends [ADR 0020](0020-external-instrument-device.md) decision 2, per the External Instrument's first slice).
   - A bare MIDI-out track (no External Instrument) adds the plan's delay compensation A (`MixEngine::latency`: Main's arrival plus the master inserts) to H, so its MIDI for a sample goes out when the mix plays that sample. Before, such a track was early by A whenever a plug-in added latency. A track that holds an External Instrument does not add A, because its audio returns through the plan, which compensates it.
   - A is read every block, so it can drop while a note sounds (a latent insert removed or bypassed, a manual latency lowered). A note-off stamped with the new A could fall due before its note-on stamped with the old one. The output thread therefore clamps each playback event's deadline to the latest one already queued for its track (equal deadlines keep arrival order). For at most the drop in A, events bunch at the old instant, and notes inside that window may be shortened. Stop, the panic and a destination change reset the clamp; a wrap or locate release sets it to the release instant. The clamp never holds an event more than 2 s past now (`CLAMP_CAP_NS`), so one bogus stamp cannot stall a track.
7. **The ingress worker wakes on data (#62).** `InputConsumer::wake_on(thread)` (`crates/midi-io/src/ring.rs`) records the worker's thread once per open input; the input callback unparks it after publishing to the ring (`unpark` is lock- and allocation-free, and a token left by an early unpark makes the next park return at once).
   - The worker (`crates/playback/src/player/ingress.rs`) parks for at most 20 ms (`IDLE`) as a safety net only: input data, attachments, route edits and Stop all unpark it, and port health (unplug, close) is noticed within that period.
   - After work the worker keeps stepping every 250 us for 16 rounds (`LINGER_PERIOD`, `LINGER`) with `thread::sleep`, so the audio thread's receipts (credits, release bookkeeping) are reclaimed promptly. `sleep` uses a high-resolution timer on Windows where a park timeout would round up to the tick.
   - The worker records callback-to-pop latency (`latency_last_ns`, `latency_max_ns`) for a future readout. On the fake backend the median went from about 1 ms with 16 ms spikes to tens of microseconds, and the echo latency test asserts under 10 ms.

## Consequences

- A MIDI or instrument track can drive a USB or virtual synth; the destination survives save and reopen, and an absent device is a visible state, not an error.
- The output thread's timing is bounded by the OS scheduler and the MIDI driver and USB, not by the audio callback. Fake-backend tests establish order, offsets, thread identity and safety; hardware accuracy needs a hardware check (the Model D benchmarks cited by ADR 0020 are the first).
- A port of another backend (a session from another OS) stays missing.
- The deadline clamp trades exactness for a guarantee: a stuck note is worse than a briefly shortened one. K39 (editing a clip while one of its notes sounds can leave that note on at the port until the next wrap, locate or Stop) is the recorded gap.
- Automation lanes do not generate CCs today, so none are forwarded; clip controllers (CC, pitch bend, pressure, program) are sent.

## Alternatives rejected

- **Reuse the existing MIDI I/O thread for sends.** Considered in #61 beside a dedicated thread; the output thread keeps its own deadline-sorted queue, sleeps against a high-resolution deadline, and owns the safety state.
- **Keep the 1 ms park in the ingress worker, or poll with a timed wait.** A park timeout rounds to the system tick on Windows (#62). A timed wait that must stay uses `thread::sleep`, as the linger rounds do.
- **Send at callback entry, with no clock map.** Early by the device's output latency and unrelated to when the audio is heard; kept only as the fallback while no map is valid.

## Related

[ADR 0012](0012-midi-device-layer.md) (device layer), [ADR 0017](0017-clock-contract-annex.md) (clock maps), [ADR 0020](0020-external-instrument-device.md) (External Instrument, stamp), `docs/contributing/knowledge/midi-output.md`, `midi.md`, `clocks.md`, issues #61, #62, #69.

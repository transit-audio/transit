# Native JACK backend

Code: `crates/audio-jack` (`transit-audio-jack`). Decision:
[ADR 0021](../../adr/0021-native-linux-audio-over-the-jack-client-api.md); unsafe policy:
[`unsafe.md`](unsafe.md). Linux only (the crate is empty elsewhere, its tests build everywhere).
One code path for JACK 2 and for PipeWire's JACK library. Slices 1 and 2 of 4 are in: the skeleton
and the duplex process engine (audio plays and records).

## Building

- Feature `native-jack` on `transit`, `soundcraft-playback` and `audio-bench`; the crate's own
  feature `libjack` turns on the backend. Without it the crate is empty; its tests (the safe core over
  the fake server) build on any platform.
- `jack-sys` `=0.5.1` with `dynamic_loading`: `libjack.so.0` is dlopened at first use, nothing links
  against it. Its build script still asks pkg-config for `jack` on Linux, so the build needs
  `libjack-jackd2-dev` (CI installs it); the binary does not need libjack to start. 0.5.1's manifest
  misspells its default feature list, so `dynamic_loading` is named explicitly.
- Building, `pw-jack` and the quantum: [docs/building.md](../../building.md#native-jack-backend-linux).

## Shape

- `src/api.rs`: the `JackApi`/`JackClient` traits (the `ComApi` pattern). `src/ffi.rs` implements
  them over libjack, `src/fake.rs` over a scripted server (tests).
- `unsafe` only in `src/ffi.rs` (every libjack call) and `src/callback.rs` (the `extern "C"`
  trampolines, each under `catch_unwind`); `lib.rs` denies it elsewhere. Every block quotes jack.h.
  The callbacks' `arg` is a leaked `Box<CallbackState>`, freed after `jack_client_close` returns.
- `jack-sys` resolves all its functions at first use and panics on a missing one: `open_client`
  makes that first use under `catch_unwind` and reports it as a missing library.

## Devices and enumeration

- The server is the system "JACK"; a device is a port group, the client part of the physical ports'
  names (`system` under JACK 2, one entry per node under PipeWire); its ports are the channels, named
  by their short names. One format per endpoint: `f32` at the server's rate and quantum.
- `JackBackend::probe` opens a short-lived client `transit-probe` with `JackNoStartServer`. Three
  states: library missing ("JACK library not found: install pipewire-jack (or JACK), or start transit
  with pw-jack"), no server ("JACK/PipeWire server not running"), or running with its groups. The
  first two are the system's diagnostic in Audio Devices, with no device.

## Requested versus achieved

Audio Devices shows "Requested 128 · running at 256" under the Output line when the Player's
`device_buffer` (or `device_rate`) differs from `ui.audio`'s `buffer_size` (`sample_rate`), on every
backend; the check is `audio_devices::requested_note`, run with the existing refresh, no polling.
Under the cpal ALSA host PipeWire runs the stream at the server's quantum whatever period was
asked for; the native JACK backend always reports the server's quantum. Change PipeWire's quantum
with `pw-metadata -n settings 0 clock.force-quantum 128` or `PIPEWIRE_QUANTUM=128/48000`.
The rate half reflects the rate of the open attempt, not a value the device reported.

## Lifecycle

- One client "transit" per session. The first open registers `in_1..` or `out_1..` and keeps the
  user callback; the other direction joins before either starts (the ASIO open-order rule). An open
  after a start and a second open of one direction are refused.
- The first `start` builds the engine from both directions' callbacks, installs the JACK callbacks
  and activates; each direction's start connects its ports (`out_i` to the device's i-th playback
  port, its i-th capture port to `in_i`), unless `TRANSIT_JACK_AUTOCONNECT=0` (`no`, `off`, `false`)
  leaves them to a patchbay. A stop is for good and returns once no cycle is inside that direction;
  when no direction runs the client is deactivated; the last drop closes it and only then frees the
  engine (the user callbacks drop on that control thread).
- `Achieved` is the server's rate and quantum whatever was requested (the Player resamples).

## The process callback

- `src/engine.rs`, safe code over the `OutputPlanes`/`InputPlanes` traits. One cycle: counters and
  the position, the capture callback (input ports interleaved into scratch), silence, the render
  callback (scratch de-interleaved into the output ports). Blocks over `SCRATCH_FRAMES` are served in
  chunks. Capture runs first at the same device position, so `capture_precedes_render` is true and
  the live-input ring needs no slack (K22).
- `src/callback.rs` holds the engine in an `UnsafeCell`, taken by the process callback under a
  `busy` flag (a miss counts `busy_skips` and plays silence). Clippy denies `format!`, `to_string`
  and the log macros in both modules; a `CountingAlloc` test runs 10,000 duplex cycles with block
  sizes 16 to 16384 and counts no allocation.
- **Stamps:** host time is `Instant::now()` at entry in the request's `HostOrigin`; backend callback
  time is that minus `jack_get_time() - current_usecs` (`jack_get_cycle_times`), quality
  `BackendEstimate`, or the arrival with `Arrival` when the cycle times fail. Device position is
  `current_frames`, unwrapped from 32 bits. A position jump, an xrun or a new quantum makes the next
  stamp `Invalid`; stamps never go backward. Frame times equal the callback time until port
  latencies land (slice 3).
- A panicking user callback is caught: that direction plays silence from then on and the watcher
  reports `StreamError::Other` once.

## The watcher

`transit-jack-watch` (20 ms, while active) turns the callbacks' atomics into stream errors, called
with no lock held:

- Server shutdown: `DeviceNotAvailable` once; the dead client is closed there, never in the handler.
  Then a probe client every 2 s for 30 s; when a server answers, `ResetRequested`, and the Player's
  reopen path opens a new client. After 30 s the user reconnects.
- A rate or quantum other than the open ones: `ResetRequested` once per new pair, so the Player
  reopens at the server's settings (the transport stops; a take is kept).
- Xruns: `StreamStats::overloads` (and the next stamp is invalid); not an error.
- `StreamStats` per direction: wakes, overloads, resets, faults, busy skips, position jumps, service
  time and lateness histograms.

## Default selection

`Auto` opens Native JACK when a JACK or PipeWire-JACK server answers, on the JACK system and on the
untouched default (no system and no device named); otherwise it is cpal's ALSA host
(`playback::backend::native::auto_jack_candidate`, ADR 0021 amendment 2026-10-25). The Audio
Devices note under the chosen port group says "Native JACK (<server>) runs at <rate> Hz with a
<n>-sample buffer" and where to change it; the buffer list offers only the server's quantum.

## Tests

- Fake server (`src/tests.rs`, every platform): enumeration, lifecycle, conversion and order,
  chunking, stamps and the position wrap, invalidation, stop, panic, shutdown, reconnect, rate and
  quantum resets, allocation.
- Real server (`src/server_tests.rs`, Linux with `libjack`): `server_*` tests skip at run time when
  no server answers. Start a dummy one: `jackd -r -d dummy -r 48000 -p 128 &`, then
  `cargo test -p transit-audio-jack --features libjack`. They check the ports and their connections,
  a loopback (`out_i` to `in_i`) that carries a per-frame signal back with one constant lag (one
  quantum through JACK's graph), and an xrun from a stalled cycle. `owned_server_*` (ignored) kill
  and restart, or re-quantize, a dummy server of their own:
  `JACK_DEFAULT_SERVER=transit-test cargo test -p transit-audio-jack --features libjack -- --ignored owned_server_ --test-threads=1`.

## Not yet (later slices)

Slice 3: `audio-bench --backend jack` with loopback, port latencies in the frame times,
`ReopenCause::ServerStopped` ("JACK server stopped"), Measure with mode `jack`, `Stream::period`
if reopening on every quantum change proves too disruptive under PipeWire. Slice 4: release.

## Known gaps

- A partly failed port registration leaves the names registered until the session closes
  (`jack_client_close` frees them). A retry then fails on the duplicate `out_1`/`in_1`, and orphan
  out ports receive silence. Not a soundness issue.
- See the known-issues rows K69 to K72.

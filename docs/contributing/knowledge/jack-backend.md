# Native JACK backend

Code: `crates/audio-jack` (`transit-audio-jack`). Decision:
[ADR 0021](../../adr/0021-native-linux-audio-over-the-jack-client-api.md); unsafe policy:
[`unsafe.md`](unsafe.md). Linux only (the crate is empty elsewhere, its tests build everywhere).
One code path for JACK 2 and for PipeWire's JACK library. Slice 1 of 4 is in: the skeleton.

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

## Lifecycle

- One client "transit" per session. The first open registers `in_1..` or `out_1..`; the other
  direction joins before either starts (the ASIO open-order rule). Slice 1 refuses an open after a
  start and a second open of one direction.
- The first `start` installs the callbacks and activates; each direction's start connects its ports
  (`out_i` to the device's i-th playback port, its i-th capture port to `in_i`). A stop is for good;
  when no direction runs the client is deactivated; the last drop closes it.
- `Achieved` is the server's rate and quantum whatever was requested (the Player resamples).

## Callbacks and the watcher

- Every callback stores atomics only (`src/engine.rs`, clippy denies `format!`, `to_string` and the
  log macros there; a `CountingAlloc` test runs 10,000 cycles). Slice 1's process callback writes
  silence and runs no user callback.
- The watcher `transit-jack-watch` (20 ms, while active): server shutdown is
  `StreamError::DeviceNotAvailable` once and the dead client is closed there, never in the handler;
  a new sample rate is `StreamError::ResetRequested` once; xruns are `StreamStats::overloads`; a
  quantum change is counted, not an error. Error callbacks run with no lock held.

## Not yet (later slices)

Slice 2: the duplex engine (conversion, stamps from `jack_get_cycle_times`, the user callbacks),
`audio-bench --backend jack` with loopback. Slice 3: `ServerStopped` reopen, quantum changes with
`Stream::period`, port latencies, Measure with mode `jack`. Slice 4: release, `Auto` on Linux.

## Slice 1 state and known gaps

- Slice 1 plays no audio: the app labels the backend "Native JACK (no audio until slice 2)", and
  testers should only run `audio-bench --backend jack --list-devices`. On the JACK system `Auto` and
  cpal open the cpal ALSA host (cpal has no JACK host in a native-jack build).
- Known gap: a partly failed port registration leaves the names registered until the session
  closes (`jack_client_close` frees them). A retry then fails on the duplicate `out_1`/`in_1`, and
  orphan out ports receive silence. Not a soundness issue.
- Known gap (slice 3): the watcher compares the rate to the open rate, so a rate change A to B to A
  reports a reset once and never again.

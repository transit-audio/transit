<!-- transit-index
title: Native Linux audio over the JACK client API, for JACK and PipeWire alike (`transit-audio-jack`, runtime-loaded libjack, one code path; MIDI stays on the ALSA sequencer in v1)
-->
# ADR 0021: Native Linux audio over the JACK client API, for JACK and PipeWire alike

- **Status:** accepted 2026-10-24 (issue #55); amended 2026-10-25 (slice 2); amended 2026-11-10 (#430, decision 2: transit's own function table); amendment (#760, the backend under PipeWire's JACK library) proposed 2026-11-20
- **Date:** 2026-10-24
- **Reopen when:** PipeWire's JACK library on the tester's system gives cycle times or port latencies that the loopback Measure cannot reconcile (then the native PipeWire API is the next step); the slice 2 bench shows cpal's ALSA path within the gate margin of this backend at 64 and 128 frames; a contributor needs JACK MIDI ports (decision 6); the main distributions stop shipping `pipewire-jack`; or a tester's live session through the bound table (Amendment 2026-11-10) fails on PipeWire or jackd2.

## Context

On Windows the Focusrite Scarlett 2i2 measured 248 ms round trip over WASAPI shared and 223 ms exclusive, and over ASIO 23.3 ms at 256 frames and 294 to 300 frames (about 6.2 ms) at 64 (`docs/benchmarks/2026-10-22-ryzen-5900x-scarlett-2i2.md`, `2026-10-23-...-asio-reported-vs-measured.md`). Issue #55 quotes "23 ms over ASIO at 64 frames"; that figure is the 256-frame one, and this ADR uses the benchmark pages. The WDM latency is inside the vendor driver. On Linux the kernel's `snd-usb-audio` drives the same class-compliant interface, and a 64 to 128 frame quantum under JACK or PipeWire typically gives 6 to 10 ms, so Linux can match the ASIO numbers without a vendor driver. Today Linux goes through cpal's ALSA host, which has the loose callback timing and thin latency reporting that led to the native WASAPI and ASIO backends (ADR 0016). Verified against `soundcraft` head a606751 (paths relative to `crates/`):

- **The trait** (`audio-backend/src/lib.rs`) has separate `open_render` and `open_capture`, `capture_with_render` for one-callback duplex systems, `Achieved` (rate, period, latency, device position, buffer frames), `Stamp { host_callback_ns, backend_callback_ns, backend_frame_ns, quality, offset_uncertainty_ns }` relative to a `HostOrigin`, `StreamError::{DeviceNotAvailable, ResetRequested, Stalled}`, `StreamStats` counters, `set_calibration`, and a scripted `fake` backend.
- **Native ASIO is the model** (`audio-asio/src/engine.rs`). One driver callback serves both directions; the stamp domain is the driver's sample position anchored once to the QPC at the first valid callback; render frame time is callback plus the driver's output latency, capture frame time callback minus its input latency (or, under a calibration, output latency minus the measured loop); a position step that is not one buffer counts a `position_jump` and invalidates the next stamp; `Held` keeps stamps from going backward; user callbacks run under `catch_unwind`; the engine module denies `format!`, `to_string` and logging by clippy; allocation is tested with `transit_rt::testing::CountingAlloc`.
- **The clock fit** (`playback/src/device_clock.rs`) fits frames against backend time per `ClockKey` and yields the input map and the output presentation map. MIDI out (#61) and the External Instrument (ADR 0020) schedule on the output map (`ClockMap::frame_to_host`); `audio-bench` times in its own capture-position fit (`tools/audio-bench/src/fit.rs`).
- **Measure** (`playback/src/player/device/calibrate.rs`) is native-ASIO only: a click by sample position on a duplex driver, stored under `CalibrationKey (device, input, mode, rate, period)` with mode `asio` (`devices::calibration_mode`). Reopen after a driver reset or stall is `playback/src/reopen.rs` (`ReopenCause`, three attempts, then the user reconnects).
- **CI** (`.github/workflows/ci.yml`, job `linux`, ubuntu-22.04) installs `libasound2-dev` and runs clippy and tests for the whole workspace; there is no JACK server on the runner, nor in the WSL gate (`~/transit-gate.sh`), which has no audio hardware or RT scheduling. `dlib` 0.5 and `libloading` 0.8 are already in `Cargo.lock`. `unsafe` lives only in the crates ADR 0013 names.

## Decision

1. **The JACK client API, one code path for two servers.** A new crate, `crates/audio-jack` (`transit-audio-jack`, Linux only, empty elsewhere), implements `Backend` over `libjack`. The same binary runs against JACK 2 and against PipeWire through its JACK library (`pipewire-jack`, or `pw-jack transit`). System name in Audio Devices: "JACK"; backend: `devices::Backend::NativeJack`, label "Native JACK".
2. **Bindings: `jack-sys` with runtime loading, our own safe layer.** The crate uses the raw `jack-sys` declarations with its dynamic-loading feature (dlopen of `libjack.so.0` through `dlib`/`libloading`, no link-time dependency), not the `jack` wrapper crate, pinned with `=`. Every `unsafe` call site is ours:
   - `src/ffi.rs` holds all calls (library load, `jack_client_open` with `JackNoStartServer`, activate, deactivate, close, port register and connect, `jack_get_ports` with `jack_free`, `jack_port_get_buffer`, `jack_get_cycle_times`, `jack_frame_time`, `jack_get_time`, `jack_port_get_latency_range`, callback registration); `src/callback.rs` holds the `extern "C"` trampolines, each under `catch_unwind` so nothing unwinds into C. The rest is safe code over a `JackApi` trait (the `ComApi` pattern), so the stamp arithmetic and lifecycle run against a fake server anywhere.
   - The callback state is a `Box` whose pointer is the JACK `arg`; it is freed only after `jack_deactivate` and `jack_client_close` have returned. Port buffers are used only inside the process callback that fetched them ("The pointer is valid only during the process callback", jack.h).
   - **ADR 0013 amendment** in this ADR's acceptance commit: `crates/audio-jack` joins the `unsafe` allowlist, for `libjack` calls and their callbacks only. Each block carries a `// SAFETY:` comment quoting the jack.h rule; a fresh-context refutation review gets the blocks and the quotes and its verdict is recorded before slice 2's gate counts (ADR 0009). Registered in xtask `LAYERS` at layer 0 beside the other backends.
3. **Device model: the server is the system, a port group is the device, ports are channels.** JACK has no devices. A device is the owner of a set of physical ports, by the client part of their names: `system` under JACK 2; one entry per device node under PipeWire (for example "Scarlett 2i2 USB Pro" with the Pro Audio profile, which the panel recommends because it exposes unmixed `AUX` channels). `endpoints()` lists them with their physical capture and playback port counts and `channel_names` from the port short names (aliases when they are more readable). An input and an output pair when the names match, else the user picks both, as WASAPI pairing does; under PipeWire both still run in one graph and one clock.
   - **Enumeration** opens a short-lived probe client with `JackNoStartServer` on a control thread; it never starts a server. When `libjack` does not load: "JACK library not found: install pipewire-jack (or JACK), or start transit with pw-jack". When no server answers: "JACK/PipeWire server not running". In both cases the system is listed with that diagnostic and the app offers cpal's ALSA host, which stays the Linux default (decision 9).
   - **PipeWire notes** for the panel and the knowledge note: on Debian and Ubuntu, `pipewire-jack` puts `libjack` outside the loader path, so either `pw-jack transit` or the distribution's `ld.so.conf.d` example file is needed; Fedora and Arch replace `libjack` outright. The quantum is set by the user (`pw-metadata -n settings 0 clock.force-quantum 128`, or `PIPEWIRE_QUANTUM=128/48000 transit` at launch). transit does not set `PIPEWIRE_QUANTUM` itself: `std::env::set_var` is `unsafe` under edition 2024 and racy in a threaded process.
4. **Client lifecycle: one client per open, the server dictates rate and quantum.**
   - The first `open_capture` or `open_render` creates the client ("transit", made unique by the server) and registers one port per requested channel (`in_1..`, `out_1..`); `capture_with_render` is true, so the Player opens capture first and both directions share the client and its one process callback, as on ASIO. The ASIO open-order rule applies: open both before starting either; an open after a start rebuilds the session. The first `start` activates the client and connects `out_i` to the device's i-th playback port and its i-th capture port to `in_i`. Connections the user makes later in a patchbay are left alone.
   - `Achieved` reports the server's rate, `jack_get_buffer_size` as period and buffer frames, F32, and the port latency (item 5). A requested rate the server does not run is not an error: the open succeeds at the server's rate and the Player resamples, as for a WASAPI mix rate. A requested period is not applied (it would change every client of the server); the panel shows the achieved quantum and how to change it. A later option may call `jack_set_buffer_size` behind an explicit button.
   - **Callbacks that are not the process callback** (buffer size, sample rate, xrun, latency, graph order, freewheel, info shutdown) are treated as real-time context on every server: they only store atomics. A backend-owned watcher thread (`transit-jack-watch`, 20 ms poll) turns the flags into actions and calls the stream's error callback; user code never runs on a JACK thread except the render and capture callbacks.
   - **Buffer size change** (PipeWire changes the quantum whenever another client asks for a smaller one): not a reopen. Blocks of any size up to `SCRATCH_FRAMES` (8192) are served by chunking, as the trait already allows; the next stamp is `Invalid` so the fit restarts; `StreamStats` gains `period_changes` and the trait gains `Stream::period() -> Option<u32>` (default `None`), which the Player polls to update the shown period and to swap the calibration for the new key, or drop it. Recording continues; a gap the server reports is a discontinuity like any xrun.
   - **Sample rate change** away from the open rate: `StreamError::ResetRequested`, the existing reopen path (stop the transport, keep a take, reopen at the new rate).
   - **Server shutdown** (`jack_on_info_shutdown`, which jack.h requires to behave like an async-signal handler): the handler stores a flag; the watcher reports `StreamError::DeviceNotAvailable` once, the Player takes its Fault path, and `ReopenCause` gains `ServerStopped` ("JACK server stopped") with its own schedule (retry every 2 s for 30 s, then the user's Reconnect). The dead client is closed on the watcher, never in the handler.
   - **Xrun:** counted in `StreamStats.overloads` (documented as "reported by the driver or server") and the next stamp is `Invalid`. An xrun is not device loss and does not reopen. **Freewheel:** stamps are `Invalid` while it lasts and the status line says "JACK is freewheeling".
5. **Timing: JACK's cycle clock is the backend time; the frame counter is the device position.**
   - At process entry the callback reads `HostOrigin` (arrival, `host_callback_ns`), `jack_get_time()` and `jack_get_cycle_times(current_frames, current_usecs, next_usecs, period_usecs)`. **Backend callback time** is the cycle start in the host clock: `arrival - (jack_get_time() - current_usecs) * 1000`. Only a difference of two JACK readings taken in one callback is used, so no assumption is made about which monotonic clock `jack_get_time` reads, and the cycle start is the server's filtered time (JACK 2's DLL, PipeWire's driver position), not the wake-up jitter. Unlike the ASIO stamps, whose backend time is position at nominal rate, this carries the device's real rate, so the fit sees drift. Quality `BackendEstimate`, `Measured` under a calibration, `Arrival` if `jack_get_cycle_times` fails.
   - **Device position** is `current_frames`, unwrapped from 32 bits to `u64` (it wraps after about 24.8 hours at 48 kHz). A step that is not the previous block's frames counts a `position_jump` and invalidates the next stamp, as on ASIO. Both directions share it, so Measure finds a click by sample position exactly as on ASIO.
   - **Frame times:** render `backend_frame_ns = callback + playback_latency.max / rate`, capture `= callback - capture_latency.max / rate`, from `jack_port_get_latency_range` on our own ports in `JackPlaybackLatency` and `JackCaptureLatency`. The ranges are read on the watcher after each latency or graph-order callback and handed to the callback through atomics (ASIO's `Latencies`); `min != max` (a port connected to paths of different latency) is shown in the device report. Under a calibration the capture frame time is `callback + playback - round trip`, so a click's output and input frames carry the same time, as on ASIO; the same `usable_calibration` refusal applies to a loop shorter than the playback figure. `Held` keeps every stamp from going backward.
   - **What keeps its meaning:** `ClockFitter` gets stamps of the same contract; the output presentation map that #61 and ADR 0020 schedule on is `frame_to_host` over the render frame times above; `audio-bench`'s capture-position fit uses `device_position` and the arrival unchanged.
   - **Measure** extends to the JACK system: `start_loop_calibration` accepts a duplex JACK session; `calibration_mode` gains `jack` (the whole loop by sample position, like `asio`), keyed by device, input, rate and period. The ADR 0020 compensation table is keyed by rate and buffer already; under PipeWire a floating quantum makes a stored figure "measured at another buffer" (amber), so the External Instrument card and the knowledge note recommend a forced quantum.
6. **MIDI stays on `transit-midi-io` (ALSA sequencer through `midir`) in v1.** #61 and ADR 0020 send from a MIDI thread at host times of the output map; that holds here unchanged. JACK MIDI ports would put MIDI output in the process callback, sample-placed within the cycle: a different design for the output thread, its note-safety rules and ADR 0020's Measure, and it would need its own ADR. Under PipeWire, ALSA sequencer ports are already bridged into the graph, so hardware MIDI is reachable either way. JACK MIDI is a later issue, decided on the tester's onset numbers.
7. **Real-time rules.** The process callback does not allocate, lock, log or format: scratch, ports and the stamp state exist before activation; the work is a safe engine over plane traits (JACK ports are non-interleaved `f32`, so the engine interleaves into scratch, as ASIO's `PlanarSource` and `PlanarSink` do); the engine module denies `format!`, `to_string` and the log macros by clippy. The `jack-sys` logging hooks are not installed (`jack_set_error_function` is left at libjack's default). RT scheduling and priority are the server's: `Stream::thread_priority` reports `WorkerPriority::Unobserved`, and the tester's `ulimit -r` and group membership go in the benchmark page. The Player's audio-thread rules (`docs/contributing/knowledge/audio-thread.md`) apply in the render callback unchanged.
8. **Build and packaging.** Cargo feature `native-jack` on `transit`, `soundcraft-playback` and `audio-bench`, like `native-asio`. `libjack` is loaded at run time, so the release tarball starts on a system without JACK and the JACK system then says why it is unavailable. The crate needs no `libjack` headers at build time; if `jack-sys`'s build script turns out to want them, CI installs `libjack-jackd2-dev` for the build only and the runtime loading is unchanged. CI's linux job adds `cargo clippy` and `cargo test` with `--features native-jack` for `transit-audio-jack`, `soundcraft-playback` and `audio-bench`; the WSL gate script runs the same lines. Unit tests use the fake server. No asset is added, so `ATTRIBUTION.md` does not change; `libjack` is LGPL and is loaded, not bundled.
9. **Selection.** `Auto` stays cpal ALSA on Linux until slice 4's acceptance. A failure of the native open before `start()` falls back to cpal with "Native JACK failed: <reason>; using the cpal backend", the existing path. Under JACK 2 cpal's ALSA host usually cannot open the hardware the server holds; the fallback message then says to stop the server or use JACK.

## Tests and acceptance

- **Fake server** (`src/fake.rs`, runs in CI): scripted cycles of `nframes`, `current_frames`, `current_usecs`, `jack_get_time` and latency ranges. Check: the backend callback time equals the cycle start in host time for a constant clock offset; render and capture frame times; the 32-bit frame counter unwrapping across the wrap; a position jump, an xrun and a buffer size change each invalidate exactly the next stamp; stamps never go backward; a calibration gives equal times for a click's output and input frames, and one shorter than the playback figure is refused; shutdown reports `DeviceNotAvailable` once and closes the client off the handler; a rate change reports `ResetRequested`; enumeration states for missing library, no server, and two port groups under PipeWire naming; open order and duplex on one client; a panicking user callback is silenced and reported once.
- **Allocation:** a `CountingAlloc` test drives the engine over the fake for 10,000 cycles with variable `nframes` and counts zero allocations; an ignored `hardware_` test does the same over a real server.
- **Server tests without hardware** (ignored, not CI): against `jackd -d dummy` in WSL or on the tester's machine: open, run, kill the server, see the Fault, restart it, see the reopen.
- **Tester acceptance** (48 kHz, the tester's interface, a loopback cable):
  - (a) `audio-bench --backend jack --loopback` at quantum 64 and 128: every click pairs and the round trip is in the 6 to 10 ms class (#55); reported against the Scarlett ASIO rows of the 2026-10-23 page where the interface allows.
  - (b) The server-reported loop (playback plus capture latency) and the measured loop agree within a few frames, or the difference is recorded as for K31.
  - (c) Playback and recording at 128 under PipeWire for 10 minutes with zero server-reported xruns and zero bench drift steps; `jack_lsp` shows the transit ports.
  - (d) Stopping the server during playback leaves no crash, shows "JACK server stopped", and playback resumes after restart without user action within the reopen schedule.
  - (e) Later, with ADR 0020: a MIDI-to-audio onset check on a hardware synth, if the tester has one.
  - Results go in `docs/benchmarks/<date>-<machine>-jack.md` with distro, kernel, server and version, quantum, interface and round trip.

## Tester checklist

What we need from the Linux tester before slice 2:
- Distribution and version; `uname -a` (and whether the kernel is PREEMPT_RT: `uname -v`, `/sys/kernel/realtime`).
- Server: `pactl info | grep "Server Name"` and `pipewire --version`, or `jackd --version`; where `libjack.so.0` resolves (`ldconfig -p | grep libjack`) and whether `pw-jack` is installed.
- Interface model, `cat /proc/asound/cards`, the PipeWire profile in use (Pro Audio or not), and which output is cabled to which input.
- `pw-top` while audio plays (QUANT, RATE, ERR columns) and `pw-metadata -n settings`; under JACK 2 the `jackd` command line (`-p`, `-n`, `-r`).
- `pw-jack jack_lsp -c -l -p` (or `jack_lsp -c -l -p`): ports, connections, latencies.
- RT limits: `ulimit -r -l`, `groups`, whether `rtkit-daemon` runs, files in `/etc/security/limits.d/`; CPU model and governor.
- Whether they build from source (Rust stable) or use the release tarball.

## Slices

Each slice ships alone behind `native-jack`, gets a refutation review, and changes nothing for a build without the feature.

1. **Skeleton** (small): the crate, `jack-sys` loading, `JackApi` and the fake server, enumeration with the three states, open and close of a client with ports, `Achieved`, `NativeJack` in the selector, the ADR 0013 amendment, xtask `LAYERS`, CI and gate lines. No audio callback yet beyond silence. Tester: `audio-bench --backend jack --list-devices` output.
2. **Duplex callback and stamps:** the process engine, conversion, stamps and stats, the allocation tests, `audio-bench --backend jack` with loopback. Gate: the quoted-unsafe verdict, and acceptance (a) and (c) on the tester's machine.
3. **Loss, change and Measure:** shutdown and `ServerStopped` reopen, buffer size and rate changes, latency ranges, `Stream::period`, Measure with mode `jack`. Gate: acceptance (b) and (d).
4. **Release and docs:** the release built with the feature, `docs/contributing/knowledge/jack-backend.md`, the benchmarks page, known issues, the README Features line. `Auto` picks native JACK on Linux when a server answers, cpal otherwise, after a week of the tester's use without a fallback event.

### Amendment 2026-10-25: slice 2 (audio) and the Linux default

Slice 2 ships the duplex process engine. Where it departs from the decisions above:

- **Scope moved between slices.** Slice 2 also delivers the server-stop path of decision 4 and slice 3's
  rate and quantum handling, in a simpler form; `audio-bench --backend jack`, port latencies,
  `Stream::period` and Measure with mode `jack` stay in slice 3.
- **Process.** One callback per cycle: the capture callback, then the render callback, at one device
  position (`capture_precedes_render` is true, so the live-input ring aligns with no slack, K22). The
  engine sits in an `UnsafeCell` in `src/callback.rs`, taken by the process callback under a `busy`
  flag; nothing else touches it after activation, and it is freed after `jack_client_close`. A
  stop flags the direction and waits until no cycle is inside it. A user callback's panic is caught,
  the direction plays silence from then on, and the watcher reports `StreamError::Other` once.
- **Stamps** follow decision 5 except that the frame times equal the callback time (port latencies
  are slice 3). A position jump, an xrun and a new quantum each make the next stamp `Invalid`.
- **Buffer size and rate changes are a reopen.** Decision 4 served a new quantum by chunking, with
  `Stream::period` for the Player. Slice 2 still serves any block size by chunking (so the cycles
  before the reopen play), but the watcher then reports `ResetRequested` once per new rate and
  quantum pair. The Player's reopen path opens again at the server's settings, so the shown buffer,
  the calibration key and the ADR 0020 compensation key are always the running ones. A quantum
  change stops the transport (a take is kept). The chunked path without a reopen remains the slice 3
  option if PipeWire's floating quantum makes reopens too frequent in the tester's use.
- **Server shutdown and return.** The watcher reports `DeviceNotAvailable` once and closes the dead
  client. It then opens a probe client every 2 s for 30 s, and when a server answers it reports
  `ResetRequested`, which the Player's existing reopen path serves. There is no new `ReopenCause`:
  the status line says "driver reset" until slice 3 adds `ServerStopped` ("JACK server stopped").
- **Connections.** Ports connect to the chosen group's ports at start, as in decision 4, unless
  `TRANSIT_JACK_AUTOCONNECT=0` (or `no`, `off`, `false`) leaves them to a patchbay.
- **Naming.** The selector label is "Native JACK". The Audio Devices note under the chosen port
  group names the server from `jack_get_version_string` ("Native JACK (PipeWire 1.0.5)", "Native
  JACK (JACK 1.9.20)"), its rate and buffer, and where to change them (`pw-metadata -n settings 0
  clock.force-quantum N`, `jackd -p N`). The buffer list offers only the server's quantum.
- **Default selection on Linux (replaces decision 9's first sentence).** When a JACK or
  PipeWire-JACK server answers, `Auto` opens Native JACK on the JACK system and on the untouched
  default (no system and no device named); with no server, or with a saved ALSA device, `Auto` is
  cpal's ALSA host as before. Reason: under JACK 2 the server holds the hardware, so cpal's ALSA
  host usually cannot open it, and under PipeWire the JACK client API gives the server's cycle clock
  and xrun reports that cpal's ALSA path lacks. The probe never starts a server. The fallback of
  decision 9 still applies when the native open fails. Slice 4's week of tester use still gates
  the release notes' claim, not the default.
- **Server tests.** `server_*` tests in `src/server_tests.rs` run against a real server through
  libjack and skip at run time, with a line on stderr, when none answers (CI and the WSL gate run
  none). The two that kill or reconfigure a server (`owned_server_*`) are `#[ignore]` and start their
  own dummy server, named by `JACK_DEFAULT_SERVER`.

### Amendment 2026-10-25: decision 4, the buffer size is asked of the server

Decision 4 and the 2026-10-25 amendment left the quantum to the server and offered only the one it
ran at listing time. A Linux tester on PipeWire then saw a list of one entry, 64, which that card
cannot run, and no way to change it without leaving the app. The decision changes:

- **The list offers the standard sizes 64 to 2048**, plus the running quantum when it lies outside
  them, with the running size marked in the list.
- **Apply asks the server.** When the chosen buffer size differs from the server's, the backend calls
  `jack_set_buffer_size` once, as it opens its client, on the thread that opens the client (the
  Player's open or reopen path; never a JACK callback). Every client of the server gets the new
  quantum, so the panel says so in one line: "Changes the JACK server's buffer size for all
  clients."
- **The achieved figure follows the server.** `Achieved` reports `jack_get_buffer_size` after the
  call. A server that applies the change later (PipeWire) reaches the buffer size callback, which
  already requests a reopen at the new quantum. A refusal is not an open error: the stream opens at
  the server's quantum and the requested-versus-running line shows the mismatch.
- **Unchanged.** transit does not set `PIPEWIRE_QUANTUM`, and the panel keeps the `pw-metadata`
  hint for pinning a quantum.
- **Precedent.** Bitwig Studio and REAPER both offer a buffer size under JACK and request it from
  the server; users of a shared server expect the host that opens a session to set its size.

### Amendment 2026-11-10: decision 2, transit's own function table (#430)

Amended 2026-11-10, accepted (issue #430). It replaces decision 2's first sentence, the first clause of decision 3's PipeWire note (on
`pw-jack`), decision 3's message for a library that does not load, and decision 8's sentence on
JACK headers at build time (Decision 2, old and new); every other decision, and the 2026-10-25
amendments, stand.

**The defect.** Decision 2 bound libjack through `jack-sys`'s run-time loading. `jack-sys` 0.5.1
opens `libjack.so.0` once, by that bare soname, and resolves every function from that handle. The
dynamic loader first matches a name without a slash against the objects already loaded (by file
name or `DT_SONAME`), so that `dlopen` returns the first libjack mapped in the process, whichever it
is; jackd2's and PipeWire's libraries both carry the soname `libjack.so.0`. A tester with jackd2
1.9.22 and PipeWire running, started without `pw-jack`, got "connected, but another libjack was
already loaded in this process; start transit with pw-jack" and no JACK ports (#429). That text was
shown only after a probed library connected through its own handle and the soname `dlopen` then
returned a different object, so another libjack was already mapped. Which component mapped it was
not established: an earlier probe of the loader's `libjack.so.0` when no PipeWire socket was seen
(the #230 order), or another component of the process, plausibly ALSA's JACK plug-in during ALSA
device listing (plausible, not proven). No probe order can prevent the second; not resolving by
soname removes both.

**The rule.**

1. **Own declarations, own table.** `crates/audio-jack` declares the libjack types, constants and
   function prototypes it calls (`src/sys.rs`, quoted from jack.h and types.h) and drops `jack-sys`.
   Each candidate library is opened (`libloading`, pinned with `=`) and every function is resolved
   with `dlsym` on that handle into one table (`JackFns`); a probe calls only its own candidate's
   table, and every later call goes only through the table of the library whose probe connected,
   never through a lookup by soname. A candidate given as a soname (the loader's `libjack.so.0`, or
   a soname in `TRANSIT_JACK_LIBRARY`) is opened by that name, so its handle is whatever object the
   loader returns for it, possibly one already mapped, by another component or by an earlier probe (a candidate that did not
   connect stays loaded, rule 3); the table is then that
   object's, used only if that object's own probe connects. A library lacking a required function
   is not usable and the selection goes on to the next candidate; `jack_get_cycle_times` and
   `jack_get_version_string` (`JACK_OPTIONAL_WEAK_EXPORT` in jack.h) are optional. Resolving through
   the handle selects that library's exported functions (`dlsym` searches the object, then its own
   dependencies); how the library's own internal references bind is the loader's and unchanged
   (Open).
2. **The choice.** Candidates in order: `TRANSIT_JACK_LIBRARY` when set (a path or a soname);
   PipeWire's library, found the way `pw-jack` finds it: first the directories written in
   the first readable `pw-jack` on `PATH` (each absolute one, the loader's `$LIB` token replaced by
   each loader directory's path relative to `/usr` or `/` when that path starts with `lib`, a word
   holding any other `$` skipped) that hold a `libjack.so.0`, skipping a directory that is itself one
   of the loader's (its `libjack.so.0` is the system's); then
   `pipewire-0.3/jack/libjack.so.0` under each loader directory (`LD_LIBRARY_PATH`, the directories
   of the shared objects mapped in the process, `/etc/ld.so.conf` and its includes, `/lib64`,
   `/usr/lib64`, `/lib`, `/usr/lib`); only files that exist, each file once by its canonical path;
   and the loader's `libjack.so.0`. PipeWire's go before `libjack.so.0` when a PipeWire socket exists
   (`$PIPEWIRE_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR`, joined with `$PIPEWIRE_REMOTE`, else
   `pipewire-0`) and no library is named, after it otherwise. No distribution path is written into
   transit. A library is used only if it loads, has every required function, and its probe client
   (`JackNoStartServer`) connects through its own table without starting a server; the first such
   candidate is chosen and the ones after it are not loaded.
3. **Once per process, never unloaded.** The chosen table is bound once (`OnceLock`) and never
   replaced, so two libjacks are never both bound. After binding no other library is loaded and the
   candidates are not probed again; every later client, enumeration's short-lived probe clients
   included, opens through the bound table, so switching between jackd2 and PipeWire takes a
   restart of transit. No library the backend loads is ever unloaded, the chosen one or a probed
   one: a library whose code may still run (a callback, a thread a failed open left behind) must not
   be unmapped. A failed selection binds nothing, and the next open probes the candidates again.
4. **Real time.** The process callback reaches `jack_port_get_buffer`, `jack_get_cycle_times` and
   `jack_get_time` through a `&'static JackFns` held by its callback state: a load and an indirect
   call (plus a `None` test for the optional `jack_get_cycle_times`), with no lock, allocation or
   lazy-initialization check (the `jack-sys` table had a `Once` check per call).
5. **The fallback note.** The "another libjack was already loaded" refusal is removed. When no
   candidate connects, Audio Devices lists every library tried and why and, when no candidate other
   than the plain `libjack.so.0` has `pipewire` in its path (a named library counts), adds
   "PipeWire's JACK library was not found: install pipewire-jack, or start transit with pw-jack".
6. **Build.** No JACK package is needed to build (`jack-sys`'s build script asked pkg-config for
   `jack`); CI and the release job drop `libjack-jackd2-dev`. `libloading` stays at the 0.7.4 that
   #230 pinned beside `jack-sys` and that the `// SAFETY:` comments were written against; this crate
   is now its only user in the lock, and moving to the 0.8 already there is a reviewed bump of its
   own.
7. **Tests.** No test starts, stops or connects to a server.
   - Rule 1, rule 2's usability condition and rule 3's failed selection, on Linux (`src/ffi.rs`,
     through the backend's own loader; fake libjacks the test compiles with the system C compiler,
     sharing one soname; none exports `jack_get_cycle_times`, so the optional path is taken):
     `the_table_is_bound_from_the_library_that_connected_not_the_first_mapped_soname` (a fake
     jackd2 that finds no server, then a fake PipeWire that connects: calls through the table reach
     the second, while a soname `dlopen` returns the first, which reproduces the defect),
     `a_library_lacking_a_function_is_passed_over_for_the_next`,
     `with_no_library_connecting_nothing_is_bound`.
   - Rule 2, any platform (pure functions over a scripted machine): in `src/library.rs`
     `the_order_is_the_named_library_then_pipewires_first_when_its_server_runs`,
     `jack2s_status_without_a_server_reads_as_not_running`,
     `jack2s_library_without_jackd_falls_back_to_pipewires`,
     `the_first_library_that_connects_wins_and_the_rest_are_not_loaded`; in `src/search.rs`
     `the_mapped_objects_give_the_loaders_library_directory`,
     `ld_so_conf_includes_are_followed_in_glob_order`,
     `the_loader_dirs_are_ld_library_path_then_mapped_then_ld_so_conf_then_the_system`,
     `pw_jacks_directory_is_read_from_the_script_with_lib_expanded`,
     `pipewires_library_is_found_without_a_distribution_path_in_transit`.
   - Rule 5: `with_none_connected_the_error_and_the_line_say_what_each_library_did`
     (`src/library.rs`).
   - Rule 6: the CI linux job and the release job, which build with `native-jack` and no JACK
     package.
   - Not tested, by inspection: the process-wide binding (set once) and that nothing is unloaded
     (each handle is forgotten, never closed), because both are process-global and read the real
     machine; the socket check, which reads the process environment (the order it feeds is
     tested); rule 4 at the trampolines, which run only under a real server
     (`the_engine_and_the_handlers_allocate_nothing` covers the engine they call).

**Decision 2, old and new.** Decision 2's first sentence read: "The crate uses the raw `jack-sys`
declarations with its dynamic-loading feature (dlopen of `libjack.so.0` through `dlib`/`libloading`,
no link-time dependency), not the `jack` wrapper crate, pinned with `=`." It now reads: "The crate
declares the libjack functions it calls (`src/sys.rs`) and resolves them at run time from the one
library it chose (rules 1 to 3), through `libloading` pinned with `=`, with no link-time dependency;
not the `jack` wrapper crate." Decision 2's bullets stand, "library load" in the first meaning rule
2's selection; the calls listed there are the design's, and the table declares only those the code
makes (`jack_frame_time` and `jack_port_get_latency_range` are not called today). Decision 3's
PipeWire note, its first clause (that `pw-jack transit` or the distribution's `ld.so.conf.d` example
file is needed where `pipewire-jack` puts `libjack` outside the loader path), is replaced by rule 2:
neither is needed, and `pw-jack transit` still works. Decision 3's message for a library that does
not load is the code's: "JACK library not found: install pipewire-jack (PipeWire) or JACK". Decision
8's sentence "The crate needs no `libjack` headers at build time; if `jack-sys`'s build script turns
out to want them, CI installs `libjack-jackd2-dev` for the build only and the runtime loading is
unchanged." is replaced by rule 6: the build needs no JACK package, and `docs/building.md` lists
none; the runtime loading stands. Decision 7's sentence on the logging hooks still holds: the table
has no `jack_set_error_function`.

**What a user sees** (no `TRANSIT_JACK_LIBRARY`):
- jackd2 only: no PipeWire candidate; jackd2's library connects when `jackd` runs. Without `jackd`:
  "JACK/PipeWire server not running", and the note adds the fallback.
- PipeWire's JACK library only: found by the search (or by the soname where the distribution puts
  it on the loader path) and connects while PipeWire runs; with PipeWire stopped, "JACK/PipeWire
  server not running".
- Both, PipeWire running: PipeWire's library is probed first and connects; a jackd2 libjack mapped
  earlier by anything no longer decides which library serves transit. Both, no PipeWire socket,
  `jackd` running: jackd2's library goes first and connects.
- Both, a PipeWire socket present but PipeWire's library not connecting (for example a socket left
  by a crashed server), `jackd` running: PipeWire's library is probed first and stays loaded
  (rule 3), so the loader answers the next candidate, the bare `libjack.so.0`, with that same object
  (rule 1) and jackd2's library is never probed; the line lists `libjack.so.0` with PipeWire's
  result. A known limit of opening the soname candidate by name; `TRANSIT_JACK_LIBRARY` (below) is
  the way out.
- Neither: `libjack.so.0` does not load: "JACK library not found: install pipewire-jack (PipeWire)
  or JACK", and the note adds the fallback.

In every case the library bound first serves the process until it exits (rule 3). To use jackd2
while a PipeWire socket exists, start transit with `TRANSIT_JACK_LIBRARY=libjack.so.0`.

**Trust.** The search reads the first `pw-jack` file on `PATH` and loads a library from a directory
written in it, and transit loads the library `TRANSIT_JACK_LIBRARY` names; loading runs that
library's initializers in transit's process. That is the trust the user already places in `PATH`
(whose `pw-jack` they would run) and in `LD_LIBRARY_PATH` (which the loader already honors);
relative `PATH` and `LD_LIBRARY_PATH` entries are skipped, and no other guard is added. The `pw-jack`
script is read, never run.

ADR 0013's amendment of 2026-10-24 (ADR 0021) is unchanged: `unsafe` stays in `src/ffi.rs`
(opening each library, resolving the table, every call) and `src/callback.rs`; `src/sys.rs` holds
declarations only and stays under the crate's `#![deny(unsafe_code)]`.

**Open** (needs a tester or a first run).
- A live session through the bound table on PipeWire and on jackd2, both installed, started
  without `pw-jack`, after the ALSA system was listed first (the KC5 tester check).
- Whether the tester's earlier jackd2 mapping came from ALSA's JACK plug-in.
- A real `pw-jack` script as installed (the tests cover a `$LIB` form and a literal path).
- PipeWire's and JACK 2's libraries left loaded after a failed probe while no server answers them
  (no thread or callback left running).
- Interposition: a libjack in the process's global scope (linked into the executable, or opened
  with `RTLD_GLOBAL`) could still receive calls PipeWire's libjack makes to its own exported
  functions. transit does not link libjack and opens every candidate `RTLD_LOCAL`; no such
  component is known in transit's process.
- The first CI and release runs without `libjack-jackd2-dev`.

**Alternatives rejected.**
- Keep `jack-sys`'s binding and require `pw-jack` (the refusal #230 shipped): `pw-jack` puts
  PipeWire's library on `LD_LIBRARY_PATH` only for a launch through it, so a desktop launcher still
  fails; the tester's JACK system worked only when started through `pw-jack` (#429).
- Preload PipeWire's library so the soname resolves to it (`LD_PRELOAD`, or opening it at startup
  before anything else maps a libjack): the choice is made before transit knows which server
  answers, so PipeWire's library would be bound even where jackd2 is the server; `LD_PRELOAD` needs
  the environment set before start (a wrapper or a re-exec; transit does not set variables in its
  own threaded process, decision 3); and either keeps `jack-sys`'s per-call `Once` check and its
  build-time `jack.pc`.
- A separate helper process as the JACK client: an extra hop and buffer between the server's cycle
  and transit's engine on every cycle, a second real-time thread whose clock decision 5 would have
  to carry across processes, and a lifecycle of its own (start, crash, restart), for a defect one
  function table removes.

## Consequences

- Linux gets an audio path with the server's own cycle clock and port latencies, so MIDI timing, the clock fit and Measure work as on native ASIO.
- One more `unsafe`-bearing crate, bounded by its size, the quoted review and the fake-server tests.
- PipeWire users must pin the quantum for stable calibrations and compensation figures; the panel and the knowledge note say so.
- Hardware acceptance depends on one outside tester; until slice 2's numbers exist, the 6 to 10 ms figure is an expectation, not a claim.

## Alternatives considered

- **The native PipeWire API** (`libpipewire` through `pipewire-rs`): PipeWire only, a much larger API and `unsafe` surface (loops, proxies, SPA pods), and JACK 2 users left out. The reopen path if `pipewire-jack` timing fails acceptance.
- **ALSA direct** (`hw:` devices, mmap): the lowest latency on PREEMPT_RT, but the device is held exclusively, which fights PipeWire or JACK on a desktop, and we would own period negotiation and recovery per driver. A later option for a dedicated machine.
- **cpal's JACK host:** timing only through `StreamInstant` (the ADR 0016 complaint), no port latencies, no xrun or shutdown routing, and server autostart by default.
- **The `jack` wrapper crate:** safe to call, but its `unsafe` and its handler and lifetime rules would sit outside the quoted review, as `wasapi-rs` would have (ADR 0016).
- **Linking `libjack` at build time:** the release would not start on a system without it.
- **A JACK server in CI (`jackd -d dummy`):** a background server and package on every run for little over the fake; kept as an ignored local test, revisited after slice 2.
- **Ports named after the session's I/O paths** (#55's proposal): renames break patchbay connections and session managers that save them; v1 keeps stable `in_n`/`out_n` names, and path names as port aliases are a follow-up.

## Related

[ADR 0009](0009-refutation-review-before-every-push.md), [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md) (amended here), [ADR 0016](0016-native-wasapi-backend.md) (the trait, the bench, native ASIO), [ADR 0017](0017-clock-contract-annex.md) (clocks), [ADR 0020](0020-external-instrument-device.md) (output map and compensation key), `docs/contributing/knowledge/{asio-backend,clocks,audio-thread,unsafe}.md`, `docs/benchmarks/`, issues #55, #61 and #71.

## Amendment (#760, proposed 2026-11-20): the backend under PipeWire's JACK library

Proposed 2026-11-20; not accepted, not built. Issue #760, with #591 (item 3), #588, #589, #429 and #431. This amendment changes when the session takes its rate and buffer size, what a chosen buffer size means under PipeWire, what Audio Devices says about transit's own links, the process thread and the path to the hardware, and how often the backend opens a probe client. It corrects several sentences of this record. Every decision and amendment above stands except the sentences quoted under "Corrections of fact".

**Sources and conventions.** Statements about today's code cite the file and line read at `transit` f707ccdf (paths from the repository root). Statements about PipeWire's JACK library cite its public source at upstream commit 0e30a57 (MIT): `pipewire-jack/src/pipewire-jack.c:N` (shortened to `pj:N`), `src/pipewire/impl-node.c:N`, `src/pipewire/keys.h:N`, `src/modules/module-scheduler-v1.c:N` (shortened to `sched:N`) and `src/daemon/jack.conf.in:N`. jack.h is quoted from JACK 2's public header, as the binding's `// SAFETY:` comments quote it. Anything inferred is marked *hypothesis*; anything that needs a real PipeWire machine is an *open question* with its check in rule 9.

### Context

**When the session reads the rate and buffer size today.**
- The probe opens a short-lived client, reads `jack_get_sample_rate` and `jack_get_buffer_size` from it while it is inactive, and closes it (`crates/audio-jack/src/session.rs:129-145`). Those figures feed the Audio Devices note (`crates/playback/src/backend/native.rs:119-122`, `281`) and every port group's format (`crates/audio-jack/src/devices.rs:41-61`).
- The first open creates the session client, asks for a chosen buffer size only when it differs from the inactive client's `buffer_size()`, then reads the rate and buffer size again, still before activation, as the session's baseline (`session.rs:202-215`). `Achieved` reports that baseline (`session.rs:236-243`).
- The first start activates the client (`session.rs:544-550`, `crates/audio-jack/src/ffi.rs:403-446`). The watcher compares the rate and quantum the callbacks stored with the baseline and requests a reset once per new pair (`session.rs:633-637`), logged as "the server now runs ... reopening" (`session.rs:716-720`). The Player reopens at most `REOPEN_ATTEMPTS = 3` times within `REOPEN_SETTLE = 10` seconds and then asks the user to reconnect (`crates/playback/src/reopen.rs:34-39`).

**What PipeWire's JACK library answers.**
- *An inactive client.* `jack_get_sample_rate` returns the client's own requested `node.latency` denominator when one is set, else a cached value, else the rate of the position the client holds (`pj:5475-5496`, the inactive branch at `pj:5482-5483`). `jack_get_buffer_size` does the same with the numerator (`pj:5499-5520`, inactive branch `pj:5506-5507`). Both cache what they return (`pj:5493`, `pj:5517`). `node.latency` is set by `PIPEWIRE_LATENCY` (`pj:4458-4459`, parsed at `pj:4465-4469`) and can be set by `jack.properties` (`src/daemon/jack.conf.in:72-73`). Before the server hands the client a driver's position (`pj:2474-2478`), the position is the client's own activation record (`pj:2317`), which the server initializes to the default `clock.quantum` and `clock.rate`, or to the global forced ones (`src/pipewire/impl-node.c:1644-1656`). So an inactive client reports a request or a default, not what the graph runs, whenever the two differ.
- *Activation.* `jack_activate` clears the cached buffer size (`pj:4920-4921`) and, when a buffer size callback is installed (transit installs one, `ffi.rs:423`), calls it synchronously from the data loop with the duration of the position the client then holds (`pj:4955-4958`, `pj:4893-4900`). The cached rate is not cleared. After activation, a quantum or rate other than the cached one queues a notification (`pj:2009-2035`, also when the driver changes, `pj:2365-2377`), and no process callback runs until the notify thread has delivered it (`pj:2081-2084`, `pj:1144-1160`). When the cached value was never set, the first position is taken silently, with no callback (`pj:2015-2018`, `pj:2029-2032`). The process callback's `nframes` is always the client's current buffer size (`pj:2103`, `pj:2239-2242`).
- *A buffer size request.* `jack_set_buffer_size` changes no server setting unless `jack.global-buffer-size` is set (default false, `pj:4533`). It sets `node.force-quantum` on the caller's own node (`pj:5413-5437`), which `src/pipewire/keys.h:173-174` defines as "force a quantum while the node is active". The scheduler lets the most recently stamped forcing follower decide the quantum unless a global `clock.force-quantum` is set (`sched:725-731`, `sched:371-377`), a force overrides every lock (`sched:783-784`), and when no follower forces any more, the driver restores its quantum, locks or not (`sched:776-781`, `sched:868-886`).
- *The quantum lock.* Every JACK client gets `node.lock-quantum=true` unless configured otherwise (`pj:4485-4486`; keys.h:171-172, "don't change quantum when this node is active"). The scheduler keeps the current quantum when any follower that was not just moved between drivers holds the lock, or while the driver resumes or is not running (`sched:717-723`, `sched:788-792`, `sched:868-876`), unless a force or the restore applies (`sched:776-786`). So, while transit runs, the quantum changes only through a force (another client's `jack_set_buffer_size`, a `PIPEWIRE_QUANTUM` launch, transit's own request, or `clock.force-quantum`), the restore after a forcing client leaves, a node newly moved between drivers, or a driver resume. The lock guarantees no fixed quantum and says nothing of the rate.

**What follows (the defects of #760).**
- *The reset after start* (read; the end result is an open question). With `PIPEWIRE_LATENCY`, a `jack.properties` latency, or a graph running at other than the default, the open's baseline is not the running figure, the first notification after activation differs, and the watcher requests a reset. A reopen reads the same kind of figure again. *Hypothesis:* this is the "Recovery: driver reset, reopened automatically (resets 2, stalls 0, reopened 2)" a tester saw in every sitting with nothing changed by hand.
- *The buffer size loop* (read; the loop itself is an open question). With a size chosen, the open sets the force, reads the old figure back as its baseline (`session.rs:208-215`), and the forced quantum arrives after activation as a change. The reopen closes the client (`session.rs:599-611`, `814-830`), which ends the force, so the scheduler restores the default; the next client asks again and the cycle repeats until the three attempts run out. This record assumed the request outlives the client ("A server that applies the change later (PipeWire) reaches the buffer size callback, which already requests a reopen at the new quantum", 2026-10-25 amendment), which holds for JACK 2's global setting only.
- *Links the server did not make* (read). `jack.self-connect-mode` (default `allow`, `pj:4541-4551`; `jack.conf.in:85-90`) can refuse a link with one end on the client's own port: the `ignore-*` modes return 0 with no link made, the `fail-*` modes return 1 (`pj:6505-6534`, `pj:6569-6570`, `pj:6614`). An unknown port, wrong direction or type returns 22 (`pj:6562-6568`). transit treats 0 and 17 as success and only logs anything else (`ffi.rs:448-459`, `session.rs:556-563`) and never reads its own links back in shipping code (`crates/audio-jack/src/api.rs:79-81`, `cfg(test)`).
- *Probe clients* (read; the count per open is read from the call sites). Each `jack_client_open` is a full PipeWire client with a node (`pj:4350-4573`). One open probes in `plan_render` and once or twice in `duplex_capture_endpoint` (`session.rs:147-169`, `306-336`), and `Auto` probes once more before choosing the backend (`native.rs:50`).
- *The process thread* (read). Under PipeWire the process thread is the library's data-loop thread inside transit's process (`pj:4398-4419`, `pj:4411-4413`), boosted by `libpipewire-module-rt` as `jack.conf` loads it ("Boost the data thread priority", `jack.conf.in:43-56`) and capped by `rt.prio` (`pj:4560`, `pj:7613-7621`). `jack_client_thread_id` returns it (`pj:5040-5047`). transit reports `WorkerPriority::Unobserved` (`session.rs:802-805`).
- *Port latency* (read). `jack_port_get_latency_range` is implemented (`pj:6750-6789`): a port's PipeWire `Latency` param, quantum multiples plus frames plus nanoseconds, evaluated at the client's current quantum and rate when read (`pj:6773-6785`). For transit's own ports, the library combines the latencies of the connected ports itself when no latency callback is installed, and leaves it to the client when one is (`pj:2888-2907`, `pj:2944-2947`). What the figure of a device's port contains (the ALSA period and headroom, a resampler's delay) is decided in server plug-in code outside this library and is not known here. transit does not bind the call (`crates/audio-jack/src/sys.rs:66-119`; `JackStream::latency` returns `None`, `session.rs:797-800`).

Convention: the 2026-10-25 amendment's precedent stands (the host that opens a session requests the server's buffer size). This amendment adds labels by kind: the requested and the running size are both shown, a figure the server reports says "reported", and the one measurement transit has is its loopback Measure.

### Decision

1. **The session's rate and buffer size are the active client's (P-1).** Activation stays at the first start. The baseline moves from the pre-activation read to a read right after `jack_activate` returns.
   - *Why not activate at open.* The Player opens capture, then render, then starts both, all inside one open call (`crates/playback/src/player/device/open.rs:521-598`), and reads the opened figures only after both starts (`open.rs:602-641`). The second direction registers its ports after the first open (`session.rs:482-514`), and the callback state takes the port arrays once, at activation (`ffi.rs:403-408`). Activating at the first open would need ports published into a running callback, or a deactivation and reactivation, which under PipeWire ends the force and applies it again (`sched:776-781`). Reading at the first start is as early as anything the Player uses.
   - *Open (first direction).* The session client is opened. Under PipeWire it makes no `jack_get_sample_rate` call before activation (a cached rate survives activation, `pj:4920-4921` clears only the buffer size), and makes no `jack_get_buffer_size` call either. Under JACK 2 the buffer size comparison of today stays (rule 2). The provisional figures in `Achieved` are the listing's: the rate and buffer of the port group the plan resolved (`devices.rs:22-24`, `session.rs:174`). The server kind is the library's version string: "PipeWire" in it means PipeWire's library (`pj:1045-1052`; `session.rs:87-97`).
   - *Start (first direction).* The engine is built and the callbacks installed and activated as today. Then, on the same control thread and under the session lock: (a) read the rate and buffer size from the active client; (b) store both as the session's rate, buffer and `reset_for`, and store them into `Shared` without counting a buffer change; (c) rule 2's wait, under PipeWire with a size chosen; (d) the connections, as today; the watcher is spawned as today. Between activation and the end of the start, the engine runs and plays silence: a direction whose state is not `STARTED` is skipped and the output ports get silence (`crates/audio-jack/src/engine.rs:63-66`, `553-570`), as today.
   - *What the Player learns.* `Stream` gains `period() -> Option<u32>` (default `None`; decision 4 named it and the 2026-10-25 amendment left it to slice 3): on JACK, the session's buffer after the start. The open path reads it once, after both starts, and uses it over `achieved.period` for `device_buffer` (`open.rs:629`) and for the held input's shown buffer (`crates/ui-studio/src/audio_devices.rs:1277`). `period()` is not polled; polling belongs to the deferred uninterrupted quantum change.
   - *A rate other than the opened one.* When the rate read after activation differs from the rate the streams were opened with, the first watcher tick requests a reset, as today (`session.rs:633-637`). Under PipeWire the backend keeps the last rate an active session read (in the process-wide backend, `crates/audio-jack/src/lib.rs:99-108`), and the next plan uses it instead of the probe's inactive figure, so that reset happens at most once per rate the graph moves to. A server shutdown clears it.
   - *The calibration key.* On JACK the loopback calibration is keyed today by the requested period, 0 when no size was chosen (`open.rs:505-507`, `565-568` rekeys ASIO only, `658`; `crates/playback/src/devices.rs:322-329`). It is keyed by the period `Stream::period()` returns, as ASIO's is keyed by the period the driver runs. A figure stored under period 0 is not applied after the change and shows "measure again" (alpha: no migration).
   - *Classic JACK 2.* jack.h describes `jack_get_buffer_size` as "the current maximum size that will ever be passed to the @a process_callback" (quoted at `ffi.rs:368-369`), with no exception for an inactive client, and `jack_set_buffer_size` as stopping the cycle and calling every buffer size callback before restarting. *Hypothesis:* under JACK 2 the read after activation equals the read before it, so the change adds one call and no reset. The fake's JACK 2 mode and one `server_*` run against a dummy JACK 2 server check it (slice 1).
   - *Fallback, decided now.* If rule 9 shows a reset right after start with the fix in place (the position at activation was the client's own, not the driver's: `pj:4897` reads whatever `rt.position` is then, *hypothesis* that it is the driver's), the baseline moves to the first process cycle instead: the start waits, bounded as in rule 2, for the first cycle and takes its `nframes` and the active client's rate after it. That is a slice 1 follow-up, not a new decision.

2. **A chosen buffer size under PipeWire is a request that lives with transit's client (P-2).**
   - *What the setting means.* Under JACK 2 the Buffer size changes the server's buffer size for every client, as today. Under PipeWire it asks the graph to run that size while transit's device is open; when transit closes the device, PipeWire goes back to its own choice (`keys.h:173-174`; `sched:776-781`). A global `clock.force-quantum` beats it (`sched:375-377`, `sched:725`). Another client that forces a quantum can take over (`sched:725-731`; *hypothesis:* the most recent request wins, from the stamp comparison, whose contents were not read).
   - *Applied by every client.* Under PipeWire each session client calls `jack_set_buffer_size` with the chosen size before activation, unconditionally: no comparison with an inactive read. Under JACK 2 the comparison of today stays (`session.rs:208-213`). A refusal is not an open error, as today.
   - *Not a change.* Under PipeWire, with a size chosen and different from the figure read after activation, the start waits at most `REQUEST_WAIT` (500 ms, in steps of the watcher period, a test-injected step count in tests) for the buffer size callback to bring the chosen size. If it arrives, it is the baseline. If not, the baseline stays what runs and the page shows the mismatch. Either way the arrival is never a reset. A later change of the quantum is a reset, as today.
   - *What the page shows.* The existing line "Requested 256 · running at 1024" (`crates/ui-studio/src/audio_devices.rs:1245-1258`) now compares with the running figure (rule 1). Under the buffer list, the note under PipeWire reads: "PipeWire runs this buffer size while transit's device is open, unless a forced quantum or another program's request overrides it; it ends when transit closes the device." Under JACK 2 it stays "Changes the JACK server's buffer size for all clients." (`audio_devices.rs:69`). With no session open, the JACK note's figures come from the probe and read "reports ... before transit's client is active"; with a session open, from the session (rule 4).
   - *Backstop.* The bounded reopen stays: three attempts, then Reconnect (`reopen.rs:34-39`). Two programs forcing different sizes can still make transit reopen; the attempts bound it (Consequences).

3. **transit's own links are read back once; a missing one is said (P-3).** At the first watcher tick after each direction's start, the backend reads the links of each of its ports it asked to connect (the pairs of `session.rs:502-509`), with the read-back slice B of ADR 0015's #431 amendment binds (`jack_port_get_all_connections`). A pair that is not linked becomes one Audio Devices line per port: "transit's link from <source> to <destination> is missing: the server did not make it (a PipeWire setting such as jack.self-connect-mode can refuse it)", with " (jack_connect returned <n>)" added when the call returned anything but 0 or 17. The read is made once per start, never repeated, and nothing is reconnected. With auto-connect off (`session.rs:44-46`) nothing is asked and nothing is read.
   - *Consistency with ADR 0015 rule 6.* Restored outside links are rule 6's: a restore that the server did not make is rule 6's failed restore, logged once and not retried. This rule covers only the links transit makes as its own. Both read links back the same way and neither retries.
   - *Hypothesis:* a link `jack_connect` made is visible to `jack_port_get_all_connections` one watcher tick later (the connect waits for one server round trip, `pj:6600`). The fake tests the logic; rule 9 step B1 checks that no false "missing" line appears.

4. **One probe per open; a running session lists through its own client (P-4).** The endpoint `plan_render` resolves carries the listing it came from (the probe's port groups, shared in the endpoint's handle), and `duplex_capture_endpoint` resolves the input against that listing, so an open probes once where it probes two or three times today (`session.rs:155-169`, `306-336`). `Auto`'s server check before the backend is chosen (`native.rs:50`) stays: two short-lived clients per open under `Auto`, one otherwise. While a session is active, enumeration (`endpoints`, the JACK system listing at `native.rs:274-309`) reads the ports and the rate and buffer through the session's client under the session lock, with no probe.
   - *What a user sees.* A graph tool (qpwgraph, pw-top) no longer shows "transit-probe" appearing and disappearing each time an open runs or Audio Devices lists, and the listed rate and buffer are the running ones while the device is open.
   - *Risk.* Listing holds the session lock for one `jack_get_ports` call, which delays a watcher tick, a stop or a start by that long. A reopen has no session open and still probes.

5. **The process thread's scheduling is observed (P-5).** After the first process cycle, the watcher reads `jack_client_thread_id` once (jack.h: "@return the pthread ID of the thread running the JACK client side real-time code"; `JACK_OPTIONAL_WEAK_EXPORT`, so an optional table entry) and the thread's policy and priority with `pthread_getschedparam`. That call lives in `transit-rt`, beside the existing reader of the current thread's level (`crates/rt/src/linux.rs:96-99`), so ADR 0013's allowlist does not change: `crates/audio-jack` gains a dependency on `transit-rt`, both at layer 0 (`xtask/src/main.rs:61`, `67`), as `transit-audio-wasapi` already has (`xtask/src/tests.rs:165`). `WorkerPriority` (`crates/audio-backend/src/lib.rs:303-330`) gains `Realtime { round_robin: bool, priority: i32 }` and `TimeShared`, shown at the end of the open lines as " · SCHED_FIFO 88" (or " · SCHED_RR 88") and " · not real-time". Until the read, and when the library lacks the function, it stays `Unobserved`.
   - *Under a classic JACK server.* jack.h says the thread runs "the JACK client side real-time code" and returns its pthread ID; a pthread ID names a thread of the calling process, so the thread is transit's under JACK 2 as well (inferred from that definition). What sets its priority under JACK 2 was not read: the line shows what is observed, under either server, and claims nothing about who set it.
   - The render workers' refused promotion stays #589's.

6. **What sits between transit and the hardware, as far as JACK can say (P-6).** Audio Devices gets, under the open lines on the JACK system, two lines and, under PipeWire, a third:
   - "Graph: <rate> Hz, buffer <n> (from transit's active client)".
   - "Latency the server reports: output <a> frames (<x> ms), input <b> frames (<y> ms); not measured, Measure measures the loop", with "<min>–<max> frames" where the range's ends differ, and "not reported" where the library lacks the call or the port is gone.
   - Under PipeWire, always: "Rate conversion and extra buffering, if any, happen inside PipeWire and are not reported to JACK clients."

   The latency is read with `jack_port_get_latency_range` (a new optional table entry: jack.h marks it `JACK_WEAK_EXPORT`) on the chosen output group's playback ports (`JackPlaybackLatency`) and the input group's capture ports (`JackCaptureLatency`), found with `jack_port_by_name` (slice B moves it out of `cfg(test)`), at the same first watcher tick as rule 3, once per session, so after every reopen. No latency callback is bound: under PipeWire installing one hands the propagation of transit's own port latencies to transit (`pj:2944-2947`), and jack.h says a client that registers one must set its ports' latency ranges in it. The cost is that a figure the server changes while the session runs shows only after the next reopen. This is ADR 0021 slice 3's latency read, pulled forward, read-only. **Display only:** `Achieved.latency` and `Stream::latency` stay `None` on JACK (`session.rs:236-243`, `797-800`), so "Path latency" (`crates/engine/src/path_latency.rs:33-44`, fed from `output_latency` at `crates/ui-studio/src/low_latency.rs:89`), Low Latency mode and the frame times of decision 5 are unchanged. Feeding the figure into compensation is its own decision, because what the figure contains is decided in server plug-in code not read here.

   | Statement the lines can make | Kind |
   | --- | --- |
   | The buffer size of the cycles transit runs | Observed: the process callback's `nframes` is the active client's buffer (`pj:2103`); the line shows the session's figure, which the engine's `last_frames` confirms once cycles run (`engine.rs:152-153`, `521`) |
   | The graph's rate | Reported by the server through the active client (the driver position's rate, `pj:2023-2035`, `pj:5484-5491`) |
   | The device ports' latency in frames | Reported by the server; contents unknown |
   | Whether PipeWire converts the rate or adds buffering | Unknown to a JACK client: never shown as a fact, only the fixed sentence |
   | The device's own rate, which node drives the graph, the ALSA period | Not available through the JACK API (the library reads no such property, `pj:3848-3926`, `pj:3927-4049`); not shown |
   | The round trip | Measured, only by Measure (decision 5) |

7. **Corrections of fact (P-7).** Each sentence below is replaced in meaning by this amendment; the old text stays above as the record of what was decided then.
   - Decision 4: "(PipeWire changes the quantum whenever another client asks for a smaller one)". *Correction:* every JACK client sets `node.lock-quantum=true` by default (`pj:4485-4486`), so while any JACK client runs, the quantum changes only through a force, the restore after a forcing client leaves, a node moved between drivers or a driver resume (`sched:717-731`, `sched:776-792`, `sched:868-886`). The lock does not fix the quantum or the rate. The same correction applies to known issue K69 ("under PipeWire a client that asks for a smaller quantum interrupts playback", `docs/known-issues/entries/K69.md:7`), the comment at `crates/playback/src/pipewire.rs:23-24` ("the graph runs at the smallest quantum its clients ask for"), and the fake's comment at `crates/audio-jack/src/fake.rs:205`.
   - 2026-10-25 amendment (decision 4, the buffer size): "Every client of the server gets the new quantum" and "A server that applies the change later (PipeWire) reaches the buffer size callback, which already requests a reopen at the new quantum." *Correction:* true of JACK 2; under PipeWire the request holds only while transit's node is active (rule 2). The code comment at `session.rs:204-207` and the knowledge note's "The server's buffer size changes for every client." (`docs/contributing/knowledge/jack-backend.md:148`) are corrected the same way, and the page note per rule 2.
   - 2026-10-25 amendment (slice 2): "so the shown buffer, the calibration key and the ADR 0020 compensation key are always the running ones". *Correction:* on JACK they were the pre-activation read, and the calibration key's period was the requested size (rule 1). After rule 1 they are the running ones.
   - Decision 4, xrun: "documented as 'reported by the driver or server'", and the help text "Overloads the driver reported to transit (ASIO overloads, JACK xruns)" (`docs/help/audio-devices.toml:126`). *Correction:* under PipeWire the xrun callback fires when the driver's xrun count changes, and not for the first xrun of a driver whose count was 0 (`pj:2094-2097`); the driver's count grows on the device's own report (`src/pipewire/impl-node.c:2313-2321`); transit's own late cycles are counted on transit's node (`impl-node.c:2240-2243`, `pj:2055-2060`) and show in transit as position jumps (`engine.rs:530-541`). The help text adds: "On PipeWire: xruns the device driver reported (the first one is not reported); transit's own late cycles show as position jumps."
   - Decision 7: "RT scheduling and priority are the server's: `Stream::thread_priority` reports `WorkerPriority::Unobserved`". *Correction:* the process thread is a thread of transit's process (rule 5); under PipeWire it is promoted by the client library's RT module (`jack.conf.in:43-56`). transit reports what it observes.
   - Decision 3 and `docs/building.md:111-112`: "`PIPEWIRE_QUANTUM=128/48000 transit` at launch" offered as a quantum setting. *Correction:* it sets `node.force-rate` as well as `node.force-quantum` (`pj:4447-4453`), so it pins the rate too. The building page says so or keeps only `pw-metadata`.
   - README.md:87: "and asks for `pw-jack` if JACK 2's library got loaded first". *Correction:* that refusal was removed by the 2026-11-10 amendment (rule 5); the note offers `pw-jack` only when no PipeWire library was found (`crates/audio-jack/src/library.rs:139-141`).
   - Decision 5: "the cycle start is the server's filtered time (JACK 2's DLL, PipeWire's driver position)". *Precision:* under PipeWire `current_usecs` is the predicted next wake-up minus one nominal period at the current rate correction (`pj:7142-7146`), not a measured start. Only a difference of two readings of one callback is used (decision 5), so nothing changes in code.

8. **Not in this amendment (P-8).**
   - *Uninterrupted quantum changes* (#591 item 3) stay deferred to their own timing-contract amendment, with the six points ADR 0015's #431 amendment lists. After rules 1 and 2, the reopens left are those of rule 2's override case and of the triggers in rule 7's first correction. *Hypothesis:* those are rare in ordinary use; rule 9 measures it. The deferral is revisited when the experiment or a tester shows resets after the fix.
   - *A native PipeWire backend* stays rejected for now (Alternatives considered, above).
   - *Reading PipeWire's own properties through `pw-dump`* is an alternative (below), not taken.

9. **Experiment on a real PipeWire machine (P-9; bounded; gates the release).** One sitting of about 30 minutes, merged with ADR 0015's #431 rule 12 check, on two builds: the current release (A) and a test build with this amendment's slice 1 and slice 2 and #431 slices A to C (B). The step list is under "Tester steps".
   - *A1, default settings, current build:* resets after start. Resets > 0 confirms the stale baseline (with the log's "the server now runs ..." pair differing from the open's "active at" pair, `session.rs:550`, `719`); 0 means the tester's earlier resets have another cause, and the log lines decide which (the design of rule 1 stands on the source).
   - *A2, buffer size loop:* default quantum, choose 256, Apply. Three "reopening" lines and "did not reopen after 3 attempts" confirm the loop. One reopen and then a steady 256 refute it (the force would outlive the client), and rule 2's unconditional request is still kept, being harmless then.
   - *A3, stale read:* `PIPEWIRE_LATENCY=256/48000` while the graph runs at another quantum: resets confirm the inactive read.
   - *B1 to B3, the same with the fix:* 0 resets in each; B2 shows "running at 256" or the requested-versus-running line with no reopen; no "link ... is missing" line; the Graph and Latency lines; a scheduling suffix. A reset in B1 or B3 triggers rule 1's fallback; a reopen in B2 means the forced quantum did not arrive within `REQUEST_WAIT` and the bound is revisited with the log's timing.
   - Results go into the #760 thread and the benchmarks page of decision 8's acceptance list. One run supports the tested configuration, not every PipeWire desktop.

### Tester steps (one sitting, #760 and #431 together)

Run each command in a terminal and paste the output, or attach the files named. About 30 minutes. A = the release you run now; B = the test build. Close any other audio program that sets a buffer size.

1. Before starting transit: `pw-metadata -n settings 0 > settings-before.txt`, `pw-top -b -n 3 > top-before.txt`, `pw-jack jack_lsp -c -p -l > ports-before.txt`.
2. **A1.** Start build A. Audio > Audio Devices: system JACK, backend Native JACK, buffer size left at its default, Apply. Play a session for 30 seconds. Press Copy diagnostics and paste it.
3. **A2.** Choose buffer size 256, Apply, wait 20 seconds. While waiting: `pw-top -b -n 3 > top-a2.txt`. Copy diagnostics and paste. Then: `grep -c "reopening" ~/.config/transit/logs/transit.log` and `grep -n "did not reopen\|reopening\|active at" ~/.config/transit/logs/transit.log | tail -20`. Set the buffer size back to its default, Apply, quit transit.
4. **A3.** `pw-top -b -n 1 | head -5` (note the QUANT of the driver). Start build A as `PIPEWIRE_LATENCY=256/48000 ./transit` (or the path you start it from). Play 30 seconds, Copy diagnostics, paste, and the same two `grep` lines. Quit.
5. **B1 to B3.** Repeat steps 2 to 4 with build B. Also paste the Audio Devices lines that start with "Graph:", "Latency the server reports:", "Output open:" and any line that says "link ... is missing".
6. **#431 (build B, still running from step 5 or started again normally).** Turn on Audio Devices > Input > **Show virtual devices**. Pick a virtual sink's monitor as the input; right after it opens run `pw-link -l > links-monitor-open.txt` and `pw-top -b -n 3 > top-monitor.txt`; record a 20-second take; run `pw-link -l > links-monitor-1min.txt` after a minute. Do the same with a virtual source (files `links-source-open.txt`, `top-source.txt`, `links-source-1min.txt`). Say whether each take has audio.
7. After quitting: `pw-metadata -n settings 0 > settings-after.txt`. Attach every file and both builds' `transit.log`.

### Slices

All slices land after ADR 0015's #431 slice B, which is being built in `crates/audio-jack/src/session.rs`, `ffi.rs` and `sys.rs`. They share with it: the start's connect loop (`session.rs:556-563`) and the close path (`session.rs:599-611`); the connection read (`ffi.rs:461-494`, `sys.rs:103-108`, `api.rs:79-81`), which slice B moves out of `cfg(test)` and which rule 3 then uses; and the fake's links and self-connect model. Each slice ships alone, gets a refutation review (ADR 0009), runs the repository gates and the Linux gate, and gets the quoted-unsafe review where `ffi.rs` changes. A new binding call is exercised through the real binding once: a `server_*` test, skipped where no server answers, run by the builder against a dummy JACK 2 server in WSL and recorded in the result note. Fake watcher ticks and waits are stepped, never wall-clock.

- **Slice 1: the baseline and the request (rules 1 and 2, rule 7's text corrections).** `audio-jack` (`session.rs`, `fake.rs`, tests), `audio-backend` (`Stream::period`), `playback` (`open.rs`: `period()` after the starts, the JACK calibration key; the remembered rate), `ui-studio` (the buffer note by server kind, in a new file beside `audio_devices.rs`, which has 1383 lines), docs (README.md:87, `docs/building.md`, K69, the knowledge note, `docs/help/audio-devices.toml`, the comments in `pipewire.rs`, `fake.rs` and `session.rs`). Small to medium. The fake gains a PipeWire mode: each client's `node.latency` request; an inactive client answers its request, else the server's default, distinct from the running figures; activation clears the cached buffer size and calls the buffer size callback synchronously with the running one, while the rate cache survives; `jack_set_buffer_size` forces the quantum only while that client is active and the server restores its default when it deactivates or closes; a forced quantum that arrives a given number of watcher steps after activation. Its JACK 2 mode is today's (one global quantum, applied at once, `fake.rs:289-295`, `357-364`). Gates:
  - PipeWire mode, graph at 1024, a client request of 256/48000: no reset after start; `Achieved` is provisional and `period()` returns 1024; no `sample_rate` call on the session client before activation.
  - PipeWire mode, default 1024, 256 chosen, the force arriving two steps after activation: the start returns with 256, no reset, `period()` 256, one `jack_set_buffer_size` per client, also on a reopen; with the force never arriving, `period()` 1024, no reset, and the page shows "Requested 256 · running at 1024".
  - A quantum change after the start is still one reset (today's behavior kept), in both modes.
  - A rate after activation other than the opened one: one reset, and the next open's plan uses the remembered rate (no second reset); a shutdown clears it.
  - JACK 2 mode: the requests and reads are today's, the baseline equals the pre-activation figure, no reset.
  - A loopback figure keyed by the running quantum is applied at that quantum and not at another; a figure under period 0 is not applied.
  - The page note per server kind; the JACK note's figures from the session while one is open.
  - `server_*` (dummy JACK 2): the read after activation equals the read before it.
- **Slice 2: what transit observes (rules 3, 5 and 6).** `audio-jack` (`sys.rs`: `jack_port_get_latency_range` and `jack_client_thread_id` as optional entries; `ffi.rs` with quoted `// SAFETY:` notes; `session.rs`), `rt` (the scheduling read of a given thread, Linux), `audio-backend` (`WorkerPriority` variants; a defaulted `Stream` accessor for the backend's lines), `playback` and `ui-studio` (the lines, in a new file; the help entries). Medium. The fake gains per-port latency ranges, a thread id with a scripted policy and priority, and uses slice B's self-connect model. Gates:
  - With `ignore`: no link is made, the connect answers 0, and the line names the port; with `fail`: the line adds "(jack_connect returned 1)"; with `allow`: no line. The read happens once per start and no connect is repeated; with auto-connect off nothing is read.
  - The Graph and Latency lines from the fake's ranges, with a min–max range, "not reported" for a missing function, and the PipeWire sentence only under PipeWire; `Achieved.latency`, `Stream::latency` and the Path latency text are unchanged.
  - The scheduling suffix for `SCHED_FIFO 88`, `SCHED_RR`, `SCHED_OTHER` and a missing function (`Unobserved`).
  - The allocation test still counts zero over the engine and handlers (nothing new runs in a callback).
  - `server_*` (dummy JACK 2): the latency read of the dummy's ports, the thread id and its scheduling read, and the link read-back.
- **Slice 3: one probe per open (rule 4).** `audio-jack` (`session.rs`, `devices.rs`), `playback` (`backend/native.rs`). Small. Gates: the fake counts `open_client` calls: one per open path with a chosen input present, absent and none; enumeration with an active session opens no client; listing reads the session's figures.

**Untested without a real PipeWire server:** what an inactive client and the read after activation report on a live graph; whether the forced quantum arrives within `REQUEST_WAIT`; whether a link is visible to the read-back one tick later; what the device ports' latency figures contain; what promotes the process thread. The WSL gate runs every fake-server test and the fake-library `ffi.rs` tests (decision 2, 2026-11-10 amendment, rule 7; the new entries are optional, so the fake libraries need not export them); it has no PipeWire and no audio hardware. Rule 9 covers the first three once.

### Alternatives considered

- **Activate at the first open, the process callback playing silence until start** (the issue's first proposal). Rejected for the reason in rule 1: the second direction's ports would have to reach a running callback, or the client would be deactivated and reactivated, which under PipeWire drops and reapplies the force.
- **Take the baseline from the first process cycle, without reading after activation.** It is the strongest fact (`pj:2081-2084`, `pj:2103`) but needs the start to wait for a cycle on every open. Kept as rule 1's fallback if the read after activation proves stale.
- **Stop resetting on a quantum change (#591 item 3).** Removes the loop too, but needs the timing contract ADR 0015's #431 amendment deferred.
- **Set `jack.global-buffer-size` for transit's client.** It would make the request a global `clock.force-quantum` that outlives transit (`pj:5422-5426`), a setting transit then leaves behind for every program; transit does not set properties for the user (decision 3's reason for not setting `PIPEWIRE_QUANTUM`).
- **Read PipeWire's own objects with an optional `pw-dump` run** (like the `pw-metadata` run at `crates/playback/src/pipewire.rs:176-203`). It would add the device node's negotiated rate against the graph rate, its `Latency` and `ProcessLatency` params, `api.alsa.period-size` and `api.alsa.headroom`, and the driving node, enough to say "the device runs at 44100 Hz; the graph at 48000 Hz". Not now: matching a JACK port group to its node is inexact (the client name is a description, a nick or a name, with prefixes and suffixes, `pj:3866-3893`), the output format of `pw-dump` was not read, and whether the server then resamples is an inference from its design. A later amendment, when someone needs the device rate shown.

### Consequences

- Under PipeWire, opening the device no longer reopens it right after start when the graph runs at another figure than an inactive client reports (pending rule 9), and choosing a buffer size no longer loops.
- The Buffer size note tells the truth for each server: under PipeWire, the request ends when transit closes the device.
- A link the server refused is named on the page instead of playing into nothing.
- Graph tools show one short-lived client per open (two under `Auto`) instead of two to four, and none while transit lists with its device open.
- Audio Devices shows the process thread's real scheduling and the latency the server reports, labeled as reported, with PipeWire's unreported layer said plainly.
- A loopback figure measured with no buffer size chosen is measured again once.
- Two programs that force different buffer sizes can still make transit reopen, bounded by the three attempts.

### Reopen when

- Rule 9 shows resets after start with the fix (rule 1's fallback), or the forced quantum not arriving within `REQUEST_WAIT`.
- Users ask for the device's own rate or ALSA buffering on the page (the `pw-dump` alternative).
- The reported latency is wanted in Path latency or Low Latency mode (a compensation decision).
- Two forcing programs make reopens common in a tester's use (the uninterrupted quantum change amendment).
- PipeWire's JACK library changes the inactive-client answers or the per-node force (the citations above, at a later commit).

### Open questions (each with the cheapest check)

- **Q1.** Is the position at activation the driver's, so the read after `jack_activate` is the running buffer size? Check: rule 9 step B1 and B3 (no reset).
- **Q2.** How long does a forced quantum take to arrive after activation, and is 500 ms enough? Check: rule 9 step B2, with the log's "active at" and buffer lines.
- **Q3.** Does the force end at once when transit's client closes, so a reopen races the restore? Check: rule 9 step A2 (`top-a2.txt` and the log).
- **Q4.** Is a link visible to `jack_port_get_all_connections` one watcher tick after `jack_connect` returned? Check: rule 9 step B1 shows no "missing" line with `allow`.
- **Q5.** What does the device ports' latency hold under PipeWire, against the loop Measure finds? Check: acceptance (b) of this record, a loopback Measure beside the Latency line.
- **Q6.** What sets the process thread's priority under JACK 2? Check: the slice 2 `server_*` run against a realtime dummy JACK 2 server, against `jackd -R -P`.
- **Q7.** Do distributions ship `jack.properties` with `node.latency` or a non-default `jack.self-connect-mode`? Check: `grep -r "node.latency\|self-connect" /usr/share/pipewire/jack.conf* /etc/pipewire 2>/dev/null` on the tester's machine, added to step 1 if cheap.

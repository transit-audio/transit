<!-- transit-index
title: Native Linux audio over the JACK client API, for JACK and PipeWire alike (`transit-audio-jack`, runtime-loaded libjack, one code path; MIDI stays on the ALSA sequencer in v1)
-->
# ADR 0021: Native Linux audio over the JACK client API, for JACK and PipeWire alike

- **Status:** accepted 2026-10-24 (issue #55); amended 2026-10-25 (slice 2); amended 2026-11-10 (#430, decision 2: transit's own function table)
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

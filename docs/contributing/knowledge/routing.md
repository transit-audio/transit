# Routing

Design: [ADR 0015](../../adr/0015-io-routing-abstraction.md). Read it before changing anything here.

Slice (f), [Low Latency mode](../../adr/0015-io-routing-abstraction.md#amendment-2026-11-05-low-latency-mode), stores low_latency and low_latency_limit_ms only in UiState/ui.json. The control-side selector in crates/mix/src/low_latency.rs walks the solved audio graph, excluding dropped and key edges, with active masters appended at Main and hardware output terminals resolved by plan_sinks. Longest prefixes and suffixes identify over-limit paths without enumerating them. It chooses the largest positive available audio-insert latency, then ascending numeric track/insert identity; instruments, MIDI effects and hardware remain enabled.

The compiler keeps raw slot latency separately from effective bypass and stores the override in plan-owned strip metadata. The existing render, note-reader, idle-state and key-alignment bypass branches consume it. Atomic raw latency cells survive retained-instance moves; fixed generation-tagged status requests a control-side refresh on shrinkage as well as growth, including bypassed slots. Publication and retirement use the ordinary drained plan boundary. The host's preference revision invalidates its session-identity shortcut. Refused and pending plans keep the adopted bypass display and figures.

query.path_latency reads the latest adopted plan/device status mirrored into Engine runtime state. engine is shared L, master is M, padding is L minus M, and total adds backend output latency once; each component has frames and ms. Pending replacement keeps running figures, unknown output latency leaves device/total null, and Delay Compensation off makes aligned engine figures unavailable. Capacity estimates are never readouts. Offline compilers have empty overrides; live-Player render jobs temporarily request the full chain. The independent live-cue padding rule and master hardware support are described below.

## Model

- **Per machine:** an `IoMap` describes the device's channels and the named input and output paths
  a user built (settings, not the session). Code: `crates/playback/src/devices.rs`,
  `crates/ui-studio/src/audio_devices.rs` and `io_settings.rs`.
- **Per session:** tracks, buses and sends name their destinations by string, such as
  `Route::Hardware(name)` (`crates/model/src/mixer.rs`). The session carries names only, so a
  session moves between machines and each machine resolves the names against its own map.
- **Unresolved routes:** a name the machine cannot resolve is kept in the file, renders silent,
  and is shown to the user as unresolved. Never rewrite or drop it on load.
- **Name collisions:** a path and a bus may not share a name; the command refuses.

## Route graph

`crates/model/src/route_graph.rs` models edges (output, send, input, key). It serves loop refusal
(L22), delay compensation, and the plan layering. A new kind of edge must be added to all three,
with a test that loads a loop from a file.

## Where to change what

- Resolving names to channels: `crates/engine/src/io.rs` and `crates/mix/src/sink.rs`.
- Commands that edit routes: `crates/engine/src/cmd/track.rs`, `setup_more.rs`.
- Input path resolution is shared by monitoring and recording; do not duplicate it.

## Hardware inserts

An insert may carry a hardware pair: a send path (output) and a return path (input), by name
(`crates/mix/src/hardware.rs`, ADR 0015 section 5). The slot writes the send and reads the return from
the live capture ring, which already delivers it one round trip late, so there is no FIFO in the slot.
Its latency is the measured round trip (Measure in Audio Devices > I/O > Hardware inserts plays a click
train and finds it on the return), stored per machine with device, rate and buffer; delay compensation
uses it. The send sums unaligned by design: the measured trip spans from the sink write. States
(`HardwareState`): Ready, Unmeasured (runs, no latency), NoInput, MissingSend, MissingReturn (dry) and
Disabled (dry, with a reason). Conflicts: an ordinary route to a path an insert sends on is refused, and
so is an insert whose send path is already a route target; opening a session runs the same check and
disables offenders. Offline paths print the insert dry. Gaps: K18 to K22.
Tests: `crates/mix/tests/hardware_insert.rs`, `crates/engine/tests/hardware_insert.rs`.

Hardware inserts also run on the master (#322). Sink planning and compiler metadata include its hardware sends; master processing calls the same hardware-slot branch as ordinary tracks and flushes sends without alignment. Its measured round trip contributes to the master-chain sum M in shared latency L. Missing paths, disabled or bypassed slots, and absent duplex input stay dry; offline deliverables stay dry too. The session keeps only the hardware path names and user insert settings.

A named non-Main output can enable **Live (no shared padding)** in Audio Devices > I/O paths, through `setup.io_map` (`set_live`, or `live` on `add_path`/`set`). `IoPath.live` defaults to false and lives only in the per-machine `ui.json` table; the compiler copies it into `SinkSpec` and `Sink`. A live cue retains its own maximum arrival, aligning its feeders without shared padding, while still contributing to L for other sinks. Upstream bus/key alignment and insert processing are unchanged. Main and input paths refuse the flag, and offline compiles ignore it.

Known gaps: `../../known-issues/README.md`.

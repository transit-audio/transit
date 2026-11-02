# Knowledge

What to know before touching the engine or the UI. Each file is short and points into the code and
the ADRs. A new topic gets a new file and a row here.

| File | Read it when you touch |
|---|---|
| [`layering.md`](layering.md) | Any crate or dependency. Always read. |
| [`audio-thread.md`](audio-thread.md) | Anything the audio callback runs. Always read. |
| [`clocks.md`](clocks.md) | Device timing, timestamps, MIDI timing, latency. |
| [`wasapi-modes.md`](wasapi-modes.md) | The native WASAPI backend's shared and exclusive opens. |
| [`asio-backend.md`](asio-backend.md) | The native ASIO backend: host thread, bindings, driver listing, streaming, stamps, the open-order rule. |
| [`jack-backend.md`](jack-backend.md) | The native JACK backend (Linux): runtime-loaded libjack, port groups, the client lifecycle, the watcher. |
| [`plan-renderer.md`](plan-renderer.md) | `crates/mix`: compile, render, PDC, sinks. |
| [`render-graph-measurement.md`](render-graph-measurement.md) | Render graph counters, snapshot accounting, and the tail default-flip measurement. |
| [`automation-modes.md`](automation-modes.md) | Automation modes, how the mix reads lanes, Touch holds and the return ramp. |
| [`routing.md`](routing.md) | Outputs, inputs, device channel maps, the route graph. |
| [`sidechain.md`](sidechain.md) | Insert keys and key taps. |
| [`racks.md`](racks.md) | Racks: a group that plays its instruments from one MIDI input, zones, macros. |
| [`midi.md`](midi.md) | Live MIDI input, step entry, note delivery. |
| [`midi-output.md`](midi-output.md) | External MIDI destinations: the output thread, send timing, note safety. |
| [`midi-monitor.md`](midi-monitor.md) | Window > MIDI Monitor and the input LED: the two tap rings, their thread contract, filters. |
| [`midi-learn.md`](midi-learn.md) | MIDI Learn: controller knobs and buttons mapped to mixer controls; the snapshot the ingress worker matches, pickup, the two stores. |
| [`recording.md`](recording.md) | Record start, takes, input capture. |
| [`session-format.md`](session-format.md) | Anything saved in a `.transit` file. |
| [`commands.md`](commands.md) | User actions, undo, the control channel. |
| [`ui.md`](ui.md) | `crates/ui-studio`: shortcuts, icons, theme, tests. |
| [`plugin-presets.md`](plugin-presets.md) | Hosted plug-in editor windows, their header strip, user presets. |
| [`oracle-null-testing.md`](oracle-null-testing.md) | A ported DSP engine, the oracle crate, a null test or a pitch test. |
| [`airwindows-catalog.md`](airwindows-catalog.md) | The stock-effect source selection, exact parameters, rate behavior, state and seeding behind ADR 0042, including deferred tape, saturation and plate, console initialization, slew stereo coupling and hall counter behavior. |
| [`unsafe.md`](unsafe.md) | Any `unsafe`, FFI or OS call. |

Decisions live in [`../../adr/README.md`](../../adr/README.md); the overview is
[`../../architecture.md`](../../architecture.md). Defects we already know are in
[`../../known-issues/README.md`](../../known-issues/README.md).

American English in code, comments, UI strings and docs.

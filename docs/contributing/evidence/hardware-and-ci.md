# Hardware evidence and CI

## CI

`.github/workflows/ci.yml` is the push gate: it runs on every pull request and on every push to
`main`, and cancels a superseded run. Jobs:

- `windows`: `cargo xtask layers`, `cargo xtask assets`, `cargo xtask timing`, then clippy (`-D warnings`) and the tests for
  the whole workspace with `--features transit/native-wasapi,transit/native-asio`.
- `linux`: `cargo fmt --check`, `cargo xtask timing`, clippy and the tests on default features.
- `macos`: clippy and the tests on default features (`crates/au-host` and the objc2 code compile only
  there).
- `wasm`: `cargo xtask wasm`.

`rt.yml` (the realtime-crate platform matrix) runs on pushes and pull requests that touch `crates/rt`,
`crates/mix` or the workspace manifest, and on tags. `release.yml` runs only on release tags and
manual dispatch. The gate does not call `cargo xtask ci` as a whole: `rt-targets` is covered by
`rt.yml`, and the bench step is not run. Cargo calls use `--locked`, so a stale `Cargo.lock` (which
also pins the patched cpal revision) fails the build.

Not gated: FreeBSD, `cargo xtask rt-targets` and the device-free `audio-bench` run, dependency
advisories (no `deny.toml`), and every hardware test. Hardware tests are `#[ignore]`d and never CI
gates: they need a real device and a person (see below).

The Windows release job builds one target, `windows-x86_64` (`transit/native-wasapi` and
`transit/native-asio`, no SDK); the `windows` job in `ci.yml` builds and tests both.

## Hardware evidence

### Running the hardware tests

From a clone, on the machine with the interface (Windows, 64-bit):

```
cargo test --release -p transit-playback --features native-asio,native-wasapi -- --ignored hardware_ --nocapture
```

`--test-threads=1` is no longer required (every hardware test holds one process-wide lock) but is
still fine. The loopback tests play a click out of one output and listen on one input; the pair is
`out:in` with 1-based channel numbers, read from the environment:

- `TRANSIT_LOOPBACK_SCARLETT` (default `2:2`): Focusrite USB ASIO, output 2 cabled to input 2.
- `TRANSIT_LOOPBACK_MODEL12` (default `1-2:1`): Model 12, the click on USB OUT 1 and OUT 2 together
  (the stereo path `1-2`, as the Audio Devices window sends it), MAIN L cabled to input 1. Mixer
  channel 2 is in PC mode, so OUT 2 reaches the main bus.

Either side takes one channel (`2`) or an ascending range (`1-2`). The tests run the same loop
measurement as Audio Devices > Calibration > Measure (`Player::start_loop_calibration`, by sample
position), not a separate probe, so they cannot disagree with the app.

Set them when the cable sits elsewhere, for example `TRANSIT_LOOPBACK_MODEL12=3-4:2`. Each loopback
test prints the measured round trip, the driver-reported output plus input latency and their
difference; it passes when the measurement is at least the driver figure and at most that plus eight
buffers. The interface's output level must be up (monitor knob and the output's fader) for the click
to return; a silent loop reads as "no click found", not as a failure of the device.

Needed when the claim is about real devices: latency, clock behavior, driver quirks, MIDI timing,
plug-in editors. A report includes:

- OS version, audio backend (WASAPI, ASIO, CoreAudio, ALSA or JACK), device and driver, sample rate,
  buffer size.
- The build identifier (version and commit) that was tested.
- The steps, the observed numbers (frames, milliseconds, xrun counts), and the app log from the
  settings folder (see `../../building.md`).
- A control: the same steps with the change reverted, when feasible.

Backend clock numbers carry unknown offset uncertainty; do not present them as certified latency
(see `../knowledge/clocks.md`). Fake-device results prove logic, not hardware.

Drivers can report a few frames more than the measured loop (Scarlett at 64: 326 measured against
328 reported; Model 12 at 256: 987 against 995), so the loopback tests accept a round trip down to
the driver figure minus one buffer.

### Render worker runs (#124 gate 6)

The gate 6 runs compare serial with the render workers (`TRANSIT_RENDER_POOL=1`) through the device
([2026-10-28 benchmark](../../benchmarks/2026-10-28-scarlett-render-workers-gate-6.md)). For #172
the pool runs are repeated with `TRANSIT_STRIP_DEADLINE_PERCENT=100` and `=150` (default 75): the
strips' deadline as a share of the period. The diagnostics report prints the share each run used
(`Strip deadline: N% of the period`), and the device-free bench (`crates/mix/tests/render_headroom.rs`)
takes the same variable and prints it per row.

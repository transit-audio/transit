# Layering

Crates form layers; a crate may depend only on its own layer or lower. The table is `LAYERS` in
`xtask/src/main.rs`, and `cargo xtask layers` enforces it.

| Layer | Crates |
|---|---|
| L0 | `time`, `rt`, `audio-io`, `audio-backend`, `audio-wasapi`, `audio-asio`, `audio-jack`, `midi-io`, `midi` |
| L1 | `dsp`, `instrument-core`, `sampler-core`, `macro-osc`, `resonator`, `modal-voice`, `texture`, `granular`, `modulator`, `random-sequencer`, `clap-host`, `vst3-host`, `au-host`, `plugin-host` |
| L2 | `model` (the session document) |
| L3 | `mix` (the render engine) |
| L4 | `engine` (commands, undo, I/O), `playback` (devices, Player) |
| L5 | `automation` (control channel, MCP) |
| L6 | `ui-studio` (transit's UI) |
| apps | `apps/transit`, plus the inherited `soundcraft-cli` (exempt from the table) |

## Rules the check enforces

- Every crate is registered in `LAYERS`; a new crate needs a row (and a layer you can justify).
- No dependency points upward (dev-dependencies excepted).
- The standalone crates (`STANDALONE`: `time`, `opus`, `audio-backend`, `midi-io`, `midi`,
  `macro-osc`, `instrument-core`, `resonator`, `texture`, `granular`, `modulator`, `random-sequencer`) have no
  production workspace dependencies at all. `audio-io` was standalone until it took `opus`, the
  vendored libopus behind its Ogg Opus writer and reader (#238, ADR 0048, a same-layer L0 edge). `modal-voice` depends on `resonator` at L1, and
  `sampler-core` (the portable Sampler core, ADR 0041) on `instrument-core` at L1, its only
  workspace dependency: never on `dsp`.
  `dsp` was standalone until it took
  `macro-osc` for the Macro Oscillator built-in (#195, a same-layer edge: the built-ins'
  registry, `Plugin` trait and parameter tables live in `dsp`). `dsp` also depends on
  `instrument-core`, the shared instrument primitives (shaper, matrix, SVF, LFO, ADSR) moved out of
  it by ADR 0041 so the sampler core can share them without an edge to `dsp`.
- Nothing below L6 depends on egui, eframe, winit, rfd or similar (`UI_CRATES`).

## Portability tiers

[ADR 0037, amendment 2026-11-03](../../adr/0037-adaptable-core-small-hardware.md#amendment-2026-11-03-two-tiers-of-portability)
adds a portability axis without changing the layer numbers:

| Tier | Crates or surface | Required boundary |
|---|---|---|
| A: Linux core with `std` | `engine`, `mix`, `rt`, `model`, `midi`, device-independent `playback` | Build and test without UI or plug-in hosting; no `no_std` commitment. |
| B: processing with `no_std` + `alloc` | L1 `dsp` pure processing, `instrument-core`, `sampler-core`, `macro-osc`, `resonator`, `modal-voice`, `texture`, `granular`, `modulator`, `random-sequencer`, every future DSP/effect port | Default `std` feature; bare-metal ARM library check with defaults off; no clocks, threads or allocation in process. |
| Desktop adapters, outside Tier B | dsp's desktop `Plugin`/`PluginInfo`, registry, state/editor transport and analysis; format hosts, `plugin-host`, device backends and UI | Adapt the portable cores; never make a processing crate depend on the desktop adapter. |

Tier B is the required destination for existing candidates, not a claim that they already compile
without `std`. `instrument-core` and `sampler-core` (ADR 0041) are the crates with the enforced
check: the linux job of `.github/workflows/ci.yml` runs their bare-metal library checks. The others remain
candidates until their retrofit slice enrolls them. The ADR lists blockers and the per-crate retrofit order. New DSP crates start with
the bare-metal check. Allocation belongs in constructors and `prepare`; caller-owned prepared
buffers are reused by processing, events, reset and state capture. The processing traits live
with their cores, and dsp adapts them to its desktop `Plugin` surface. A std-only dependency,
clock, thread or allocation in process is flagged against the amendment, including while a
candidate awaits conversion.

The headless feature contract introduces default-on `hosting` in `engine`, `mix` and `playback`,
plus default-on `devices` in playback. The relevant host dependencies become optional in their
direct owners; incoming dependency edges must not re-enable those defaults. Built-in slots and
their `Plugin` implementations stay available. Exact package lists, feature forwarding and
commands are in the ADR; both jobs are specified but not yet implemented by this docs amendment.

`cargo xtask layers` still enforces `LAYERS`, `STANDALONE`, UI exclusions and dev-only oracles.
It does not yet prove a feature-resolved `no_std` graph. The separate headless and bare-metal
[gates](../evidence/gates.md#portability-gates-adr-0037) supply that evidence. Tier B dependencies
must satisfy both the existing layering rules and the bare-metal check; do not add a reverse
edge to dsp from a leaf port or weaken `STANDALONE` to share a desktop trait.

## Notes

- `crates/mix` forbids `unsafe` (`unsafe_code = "forbid"`). See `unsafe.md`.
- Package prefixes: crates transit created (`rt`, `midi-io`, `audio-backend`, `audio-wasapi`, `ui-studio`) are `transit-*`, and so are the inherited crates, renamed from `soundcraft-*` in #37 (ADR 0014 amendment, trademark risk). The short names above drop either prefix.
- `ui-studio` is transit's UI. The legacy `ui-egui` crate and the web app are gone ([ADR 0032](../../adr/0032-transit-ships-a-desktop-app-only.md)).

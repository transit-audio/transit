# Layering

Crates form layers; a crate may depend only on its own layer or lower. The table is `LAYERS` in
`xtask/src/main.rs`, and `cargo xtask layers` enforces it.

| Layer | Crates |
|---|---|
| L0 | `time`, `rt`, `audio-io`, `midi-io`, `midi` |
| L1 | `dsp`, `clap-host`, `vst3-host`, `au-host` |
| L2 | `model` (the session document) |
| L3 | `mix` (the render engine) |
| L4 | `engine` (commands, undo, I/O), `playback` (devices, Player) |
| L5 | `automation` (control channel, MCP) |
| L6 | `ui-studio` (transit's UI) |
| apps | `apps/transit`, plus the inherited `soundcraft-cli` (exempt from the table) |

## Rules the check enforces

- Every crate is registered in `LAYERS`; a new crate needs a row (and a layer you can justify).
- No dependency points upward (dev-dependencies excepted).
- The standalone crates (`STANDALONE`: `time`, `audio-io`, `midi-io`, `midi`, `dsp`) have no
  workspace dependencies at all.
- Nothing below L6 depends on egui, eframe, winit, rfd or similar (`UI_CRATES`).

## Notes

- `crates/mix` forbids `unsafe` (`unsafe_code = "forbid"`). See `unsafe.md`.
- Package prefixes: crates transit created (`rt`, `midi-io`, `audio-backend`, `audio-wasapi`, `ui-studio`) are `transit-*`, and so are the inherited crates, renamed from `soundcraft-*` in #37 (ADR 0014 amendment, trademark risk). The short names above drop either prefix.
- `ui-studio` is transit's UI. The legacy `ui-egui` crate and the web app are gone ([ADR 0032](../../adr/0032-transit-ships-a-desktop-app-only.md)).

# ADR 0032: transit ships a desktop app only

- **Status:** accepted 2026-10-28 (decided by Derek, issue #177)
- **Date:** 2026-10-28
- **Reopen when:** a product question puts a browser UI in scope (a hosted demo, a collaboration feature). It would return as a new feature with its own ADR, not by reverting this removal.

## Context

transit inherited two things from SoundCraft that its shipped app never used. `crates/ui-egui` (package `transit-ui-egui`) is the legacy egui UI: 18,774 lines in `src`, about 5,000 lines of tests and examples, 273 tests. `apps/soundcraft-web` (package `transit-web`, wasm32) runs that legacy `SoundApp` in a browser; it was the only full consumer of the crate and the only reason for the `cargo xtask wasm` gate, the CI `wasm` job and the wasm32 branches in the engine crates.

A read-only survey on 2026-10-28 measured what the desktop app reached: `fonts::definitions()` (168 lines, egui only) and `mix_window::set_window_icon`, a `OnceLock` read only by the legacy separate Mix window, which the studio app does not have. About 18,600 of the 18,774 source lines were reachable only from the crate's own entry points, `ui_shot` or the web app. Nothing in `apps/transit` could launch the legacy UI. `crates/ui-studio` has not depended on it since the copy at 2af868e.

## Decision

transit is a desktop DAW and ships a desktop app only. One batch removes:

- `apps/soundcraft-web`, `crates/ui-egui` (tests, examples, the `ui_shot` example and the credits `build.rs`), `packaging/web`, their workspace entries and lockfile rows, and the unused `craft-ui` and `egui_kittest` workspace dependencies.
- The `cargo xtask wasm` gate, its row in `cargo xtask ci`, the `transit-ui-egui` layer row, the `transit-web` exemption, and the CI `wasm` job.
- Every `cfg(target_arch = "wasm32")` branch, stub and manifest table that existed only for the web app: the wasm strip pool in `transit-mix`, the stub plug-in host APIs, the `cpal` wasm-bindgen dependency, the browser delivery hook and the in-memory bounce target in the engine, and `BackendId::WebMidi`.
- Moved, not removed: `fonts.rs` (161 lines, with its tests) now lives in `crates/ui-studio`. The Mix window icon call in `apps/transit` is deleted; `transit_rt::set_window_icons_from_exe` still sets the window icons.

## Consequences

- The wasm gate and the CI `wasm` job are retired; `cargo xtask ci` and the push gate no longer compile for `wasm32-unknown-unknown`. `docs/contributing/evidence/gates.md` says so.
- The web deployment is retired. No release artifact shipped it; `packaging/web` is gone.
- About 24,000 lines and 273 tests leave the workspace. Shared engine behavior the legacy tests covered (autosave and recovery, the bounce-versus-session-replace refusal) has studio tests in `crates/ui-studio/src/recovery_tests.rs`. Rendering and layout tests of the legacy windows went with the windows.
- The contributor credits table (`contributors/*.json|toml` compiled into the legacy About window) is no longer built; the studio About window never showed it. The data files stay for a future studio build step.
- Left for follow-up, because they are not wasm cfgs: the session bundle API (`session_bundle`, `open_session_bytes`, `BUNDLE_EXTENSION`), which only the web app called.
- What a browser UI would need if it were ever wanted: a front end over the control channel, or a wasm build of `transit-ui-studio` with its own audio output (WebAudio through cpal's wasm-bindgen backend), a delivery path for saved files, a session bundle format, plug-in hosting that a browser cannot do (CLAP and VST3 are native binaries), and its own CI gate. That is a product, not a leftover to resurrect.

## Related

Issues #177, #176, #16. [ADR 0031](0031-transit-does-not-host-video.md), [ADR 0008](0008-fork-soundcraft-keep-the-engine-build-our-own-ux.md).

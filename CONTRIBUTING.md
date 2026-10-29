# Contributing to transit

Issues are welcome: bug reports (include the log from the settings folder, see the README), feature
ideas and questions.

Pull requests should be small, focused and written in the style of the code around them.

Before you open one, read the contributor docs and run the gates:

- Start at [`docs/contributing/README.md`](docs/contributing/README.md).
- How the engine and UI are built: [`docs/contributing/knowledge/README.md`](docs/contributing/knowledge/README.md).
- What counts as evidence, and the gate commands: [`docs/contributing/evidence/README.md`](docs/contributing/evidence/README.md).
- Known issues: [`docs/known-issues/README.md`](docs/known-issues/README.md). Build notes: [`docs/building.md`](docs/building.md).

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features transit/native-wasapi,transit/native-asio -- -D warnings
cargo clippy -p transit-mix -p transit-plugin-host -p transit-dsp -p transit-rt -- -D warnings   # lib targets only: the `testing` feature stays off (#215)
cargo test --locked --workspace --features transit/native-wasapi,transit/native-asio --no-fail-fast
cargo test -p transit-rt --release
cargo xtask layers
cargo xtask assets
cargo xtask timing
cargo xtask file-size
cargo xtask rt-targets   # only when crates/rt or crates/mix changed
```

The native-backend features need no SDK. The Linux job of `.github/workflows/ci.yml` is the Linux
compile proof. Details and the reasons are in [`gates.md`](docs/contributing/evidence/gates.md).

Rules:

- `unsafe` only in the crates on the xtask allowlist (`cargo xtask layers` enforces it): `rt`, the native audio backends `audio-wasapi` and `audio-asio` (COM, Win32 and ASIO driver calls) and `audio-jack` (libjack calls and their callbacks), and the plug-in host crates' SDK-interface code (`clap-host`, `vst3-host`, `au-host`). Every block carries a `// SAFETY:` comment, and the refutation review quotes each one (ADR 0013 and its amendments).
- No panics in non-test code; return errors.
- No Rust file over 1500 lines (`cargo xtask file-size`); split by concern before adding to a file near the budget.
- Tests do not assert scheduling or real-clock outcomes, and a failing test is never rerun to make it pass
  ([`gates.md`](docs/contributing/evidence/gates.md), [escapes](docs/escapes/)).
- Use American English in code, comments, UI strings and docs.
- Do not add assets without a row in `ATTRIBUTION.md`.
- Crates are named `transit-*`. The settings folder keeps its older name so existing installs keep their settings.

By contributing you agree that your work is licensed under MIT OR Apache-2.0, like the project.

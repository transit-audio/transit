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
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --locked -p transit-mix -p transit-plugin-host -p transit-dsp -p transit-rt -- -D warnings   # lib targets only: the `testing` feature stays off (#215)
cargo test --locked --workspace --no-fail-fast
cargo test -p transit-rt --release
cargo xtask layers
cargo xtask assets
cargo xtask timing
cargo xtask file-size
cargo xtask help        # the in-app help catalog matches docs/help (`cargo xtask help --write` refreshes the generated sections)
cargo xtask indexes --validate   # the known-issue and escape entry files are well formed (ADR 0050)
cargo xtask indexes --check      # the generated tables match the entry files; a pull request that only adds an entry file skips this (main regenerates)
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-instrument-core   # Tier B bare-metal check (ADR 0037); `rustup target add thumbv7em-none-eabihf` once
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-sampler-core      # Tier B bare-metal check (ADR 0041)
cargo xtask rt-targets   # only when crates/rt or crates/mix changed
```

The native-backend features need no SDK. The Linux job of `.github/workflows/ci.yml` is the Linux
compile proof. Details and the reasons are in [`gates.md`](docs/contributing/evidence/gates.md).
A pull request from outside merges only on green CI.

Tests take scratch folders from `transit_test_support::TestDir` (removed when the test ends); `cargo xtask temp-dirs` rejects a direct system temp directory call. `transit-*` folders left in the system temp directory by older runs are safe to delete.

## Adding a known issue or an escape record

The indexes in `docs/known-issues/README.md`, `docs/adr/README.md` and `docs/escapes/README.md` are
generated. Your pull request adds one file and nothing else: never edit or commit the table or list between
the `indexes:begin` and `indexes:end` markers. CI validates your file, and after the merge `main` regenerates
the table. You need no toolchain; the GitHub web editor works (add the file, open the pull request).

1. Name it. A known issue goes in `docs/known-issues/entries/`, an escape in `docs/escapes/`. The name is its
   id: `issue-<number>.md` for the GitHub issue it belongs to, `issue-<number>-b.md` for a second entry on
   the same issue (ids are unique across both folders, so a defect with a known issue and an escape needs the
   suffix on one), or a short lowercase kebab-case slug such as `midi-capture-ring.md` when there is no issue.
2. Write it. A known issue starts with a hidden block, then one paragraph (it may be wrapped; no blank line,
   no `|`):

   ```
   <!-- transit-index
   status: Open
   follow-up: What would fix it, or the gate that watches it.
   -->

   What is wrong, in one paragraph.
   ```

   Links to other files are written from the entry's own folder (`../../adr/0018-plugin-host-thread.md`), and
   every one must resolve. Do not add `order`, and do not copy it from an old entry such as `K42.md`: it only
   belongs to the entries converted from the old table, and the check refuses it elsewhere. An escape is the
   record format of [`escape-record.md`](docs/contributing/evidence/escape-record.md) with a `# <what escaped (#issue)>`
   heading; its list line is that heading.
3. Optional, with a toolchain: `cargo xtask indexes --validate` runs the same check CI does, and
   `cargo xtask indexes` shows you the table row it will produce (leave the README out of your commit).

Rules:

- `unsafe` only in the crates on the xtask allowlist (`cargo xtask layers` enforces it): `rt`, the native audio backends `audio-wasapi` and `audio-asio` (COM, Win32 and ASIO driver calls) and `audio-jack` (libjack calls and their callbacks), and the plug-in host crates' SDK-interface code (`clap-host`, `vst3-host`, `au-host`). Every block carries a `// SAFETY:` comment, and the refutation review quotes each one (ADR 0013 and its amendments).
- No panics in non-test code; return errors.
- No Rust file over 1500 lines (`cargo xtask file-size`); split by concern before adding to a file near the budget.
- Tests do not assert scheduling or real-clock outcomes, and a failing test is never rerun to make it pass
  ([`gates.md`](docs/contributing/evidence/gates.md), [escapes](docs/escapes/)).
- Use American English in code, comments, UI strings and docs.
- Do not add assets without a row in `ATTRIBUTION.md`.
- Crates are named `transit-*`. The settings folder is `transit` (`TRANSIT_CONFIG_DIR` overrides it).

By contributing you agree that your work is licensed under MIT OR Apache-2.0, like the project.

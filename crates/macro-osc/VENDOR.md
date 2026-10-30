# Vendored sources

`src/` started as a copy of Oliver Rockstedt's Rust port of Émilie Gillet's MIT firmware, and is
transit's own from there on: every engine is corrected against the C++ oracle
(`crates/macro-osc-oracle`, ADR 0033) and changes here are never sent back upstream.

| | |
|---|---|
| Upstream | https://github.com/sourcebox/mi-plaits-dsp-rs (MIT, Oliver Rockstedt) |
| Commit | `effe9864bb67d23aaf5724722ac1ce165d4e7c9f` (2026-09-21, "Merge pull request #14") |
| Copied | 2026-10-30, `src/**/*.rs` only (no tests, examples or manifest) |
| Derives from | https://github.com/pichenettes/eurorack `plaits/` and https://github.com/pichenettes/stmlib (MIT, Émilie Gillet), firmware 1.2: the same commits the oracle vendors (`../macro-osc-oracle/VENDOR.md`) |

Licenses: [`LICENSE-THIRD-PARTY`](LICENSE-THIRD-PARTY) (Rockstedt's MIT notice, and Gillet's MIT
notices for eurorack and stmlib, because the code derives from her firmware).

## Changes at import

- `std` instead of `no_std` + `alloc` (`alloc::` paths are `std::`); the crate is
  `#![forbid(unsafe_code)]`. Buffers are still allocated only when an engine is constructed.
- Dependencies dropped: `dyn-clone` (the `Engine` trait is no longer clonable), `spin` (the
  six-operator algorithm table is a `std::sync::OnceLock`), and `num-traits`' `libm` feature: the
  float methods are `std`'s, which call the platform's math library, as the C++ does. `num-traits`
  stays (default features) for the generic delay lines.
- Brand names removed from module doc comments; attribution comments in the files are kept.
- `cargo fmt` with the workspace settings; every file is inside the 1500-line budget.

## Corrections against the oracle

Each is its own commit, with the engine's null test (`tests/null_vs_oracle.rs`) as the proof.
The list is kept in the crate README.

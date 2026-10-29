# Working on transit with AI coding agents

transit is built largely by AI coding agents under human direction. This page describes the workflow
that has worked, so you can reproduce it with any agent or model. Nothing here is tied to a vendor.

## The shape

- **One coordinator.** It briefs workers, gates their results and merges. It writes no code itself.
- **Workers.** Each works in its own git worktree on its own branch, with a narrow scope.
- **A refutation reviewer** ([ADR 0009](../adr/0009-refutation-review-before-every-push.md)). A fresh
  agent with no context on the change tries to break it before it merges. The bar is critical-only:
  crash, data loss, hang, allocation or blocking on the audio thread, unsound `unsafe`, or a test that
  does not test what it claims. Anything else becomes a known issue or a follow-up issue.
- **A housekeeping pass after every push to `transit`.** Close issues with the commit hashes, update
  [known issues](../known-issues/README.md) and the README feature list.

## Model tiers by task class, not by name

- **Cheapest tier:** docs, issue housekeeping, periodic checks.
- **Mid tier:** scoped, well-briefed code tasks and merge chains.
- **Strongest tier:** engine, `unsafe`, FFI and threading work, and every refutation review of such work.
- **The coordinator itself** runs on the strongest model available, since it makes every judgment call:
  what to build, how to brief, what a review finding means, and when a result is good enough to merge.

As of October 2026 the maintainers use Haiku, Sonnet and Opus class models for the three tiers, with
Fable running the coordinator.

## Briefing a worker

A good brief is the biggest token saver. Include:

- the goal, and what is already ruled out;
- file and line pointers instead of pasted code;
- the exact gate commands to run;
- scope boundaries: the files it must not touch, and the worktree it works in;
- what the report must contain: commit SHAs, test counts, and every `// SAFETY:` block quoted when
  `unsafe` is involved.

## Token habits

- Run per-crate tests while iterating (`cargo test -p <crate>`) and one workspace gate before the merge.
- Never ask for verbose builds. Grep before reading whole crates.
- At most two heavy cargo jobs at once.
- Start a fresh worker for each new task; long-lived workers carry stale context.
- Have workers report conclusions, not file dumps.

## Repo rules agents must know

- Commit by pathspec (`git commit -- <paths>`), never `git add -A`. No attribution or co-author lines.
- `unsafe` only in the crates on the xtask allowlist (`cargo xtask layers` enforces it): `rt`, the native audio backends `audio-wasapi` and `audio-asio` (COM, Win32 and ASIO driver calls) and `audio-jack` (libjack calls and their callbacks), the plug-in host crates' SDK-interface code (`clap-host`, `vst3-host`, `au-host`), and the dev-only C++ oracle's `src/ffi.rs` (`macro-osc-oracle`, ADR 0033). Every block carries a `// SAFETY:` comment, and the refutation review quotes each one ([ADR 0013](../adr/0013-unsafe-only-in-soundcraft-rt.md) and its amendments).
- American English in code, comments, UI strings and docs.
- An escaped defect gets an escape record and a gate-class rule
  ([escape record](evidence/escape-record.md)); never patch around a defect, keep the repro.
- No Rust file over 1500 lines. `cargo xtask file-size` is a ratchet over the `OVER_BUDGET` table: a
  listed file may only shrink, an unlisted one may not cross the line. Never grow a file near the
  budget; split it by concern as a pure move first, then add the code ([`evidence/gates.md`](evidence/gates.md)).
- Tests do not assert scheduling or real-clock outcomes (E49 class). A freshness or rate comes from the
  message's own stamp (E59), and a test that compares file times sets them itself (E60). A playback
  or UI test that renders four or more strips calls `MixEngine::force_serial(true)` unless it tests the
  workers; the render workers are the default, and `TRANSIT_RENDER_POOL=0` renders serially
  ([ADR 0029](../adr/0029-strip-job-slots.md), [gates](evidence/gates.md)).
- A failing test is never rerun to make it pass, and a chain never reruns a failed test even to
  capture its message. Read the output, report it verbatim and bug-hunt the root cause before any test
  change. A starved or timing-bound failure on a loaded host is not a verdict: rerun on a quiet host,
  and a quiet failure is real (E61). See [`../escapes/`](../escapes/) and the [escape record](evidence/escape-record.md).
- One cargo target directory per checkout class. Agents working in different worktrees must not share
  a target directory at the same time: cargo then reuses another checkout's stale binaries or loses
  artifacts (E0463, #159). Give each concurrent worktree its own.
- A default changes only on a measurement on the reference hardware. Benchmarks live in
  [`../benchmarks/`](../benchmarks/README.md); the escapes behind the rules are in [`../escapes/`](../escapes/).
- Non-blocking review notes become known issues or follow-up issues. Do not drop them silently.
- Decisions live in ADRs, which the project owner maintains; do not add or change one unless asked.
- Crates form layers and a crate depends only on lower ones; nothing below the UI depends on egui.
  `cargo xtask layers` enforces it.
- Never crash: no `unwrap()`, `expect()`, `panic!`, `todo!` or `unreachable!` in non-test code; treat
  file, MIDI and plug-in data as hostile; the audio callback never allocates, locks or blocks. Every
  crash fix lands with a regression test.
- Session files: new fields use `serde(default)`, never `deny_unknown_fields`; old sessions must load.
- A test is evidence only if it fails without the fix.
- Every bundled asset needs a row in `ATTRIBUTION.md`.

"Green" means two things. The local gate, as in [CONTRIBUTING.md](../../CONTRIBUTING.md):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features transit/native-wasapi,transit/native-asio -- -D warnings
cargo test --locked --workspace --features transit/native-wasapi,transit/native-asio --no-fail-fast
cargo test -p transit-rt --release
cargo xtask layers
cargo xtask assets
cargo xtask timing
cargo xtask file-size
```

(plus `cargo xtask rt-targets` when `crates/rt` or `crates/mix` changed), and the Linux job in
`.github/workflows/ci.yml`, the Linux compile proof, which runs
`cargo clippy --locked --workspace --all-targets -- -D warnings` and
`cargo test --locked --workspace` on Ubuntu (it needs `libasound2-dev` and `pkg-config`).
Area gates are in [`evidence/gates.md`](evidence/gates.md).

## A worked example

The plug-in host thread ([ADR 0018](../adr/0018-plugin-host-thread.md), issue #36). The coordinator
briefed a strong-tier worker to write the ADR first, then implement on a branch. A fresh reviewer,
given only the diff and the authority documents, found one blocking finding: a "not responding" mark
that never cleared, which left stale plug-in state on save. The worker fixed it, and the reviewer
re-verified with its own probe. At the merge gate, three tests turned out to assume synchronous
plug-in loading; they were fixed and the branch merged. The housekeeping pass closed #36 with the
commit hashes and filed the non-blocking follow-ups as #56.

## If you only have a chat window

The same shape works with a single agent playing the roles in sequence: brief, implement, gate,
review, housekeeping. Start the review in a fresh conversation, give it the diff but not your
reasoning, and ask for counterexamples. The non-negotiable part is the refutation pass before a PR.

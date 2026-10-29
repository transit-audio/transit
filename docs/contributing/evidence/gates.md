# Gates

## Every change

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask layers      # crate layering and the no-UI-below-L6 rule (xtask/src/main.rs, LAYERS)
cargo xtask file-size   # no Rust file over 1500 lines; also part of `cargo xtask layers` (xtask/src/file_size.rs)
cargo xtask assets      # every bundled asset has a row in ATTRIBUTION.md
cargo xtask timing      # E49-class timing assertions in tests (xtask/src/timing.rs)
```

Other `xtask` commands: `cargo xtask rt-targets` (cross-target check of `crates/rt`; run it when you
touch that crate), `cargo xtask ci` (the CI sequence). The `wasm` gate is retired ([ADR 0032](../../adr/0032-transit-ships-a-desktop-app-only.md)):
transit has no web build.

## Which checkout a gate checks

A gate checks the checkout `cargo xtask` is run from, not the one the xtask binary was built in (#150): cargo reuses one binary across worktrees that share a `CARGO_TARGET_DIR`, so the root is resolved at run time. Every gate prints it as its first line, for example `layers: root E:/transit-wt-150`. Look for that line in a log before trusting a result.

## One target directory per checkout class

Cargo's fingerprint for a workspace member records the source paths of whichever checkout built it last. With a target directory shared between the clone and its worktrees, the other checkout's files are unchanged, so the unit counts as fresh and the old binary is reused (#159). Observed 2026-10-27: a test binary that lacked a new module's tests, and an `xtask` running another branch's ratchet table.

- The clone's target directory (`E:/transit/target`) is used only by builds run from the clone: the merge-chain gates and the dev build.
- Worktrees use one separate shared target directory for pre-hand-back checks. After the first run, check the test count of the crate you changed against what you expect.
- A Codex worker keeps its own target directory.
- The chain reports per-crate test counts for the crates a batch touched, so a stale binary shows up as a count that does not match the implementer's report.

## File-size budget

No Rust file under `crates/`, `apps/`, `tools/` or `xtask/` may exceed 1500 lines (counted as `wc -l` does). `cargo xtask layers` enforces it, and `cargo xtask file-size` runs the same check alone. Files over the budget today sit in the `OVER_BUDGET` table in `xtask/src/file_size.rs` with their current line count (#149):

- A listed file may not grow past its budget; it may only shrink.
- A file that is not listed may not cross 1500 lines.
- A listed file at or under 1500 lines, or one that no longer exists, fails the gate until its row is removed. The table only ever shrinks.

To split a file: cut it by concern (one module per responsibility, not per size), as a pure move that changes no code (`git diff --color-moved` shows no residue), with tests next to the code they test. Then lower or remove its row in the same commit. Do not raise a budget or add a row to make a change pass.

## By area

| You touch | Also run or extend | What it proves |
|---|---|---|
| Anything the audio callback runs (`crates/mix`, `crates/playback`) | `crates/mix/tests/plan_alloc_probe.rs`, `alloc_probe.rs`, `event_alloc_probe.rs`; `measure` in `crates/rt/src/testing.rs` | Zero allocations per block, counted by a counting allocator. |
| Rendering, routing, PDC, sinks | `crates/mix/tests/plan_parity.rs` against `crates/mix/tests/golden/plan_parity`; `path_pdc.rs`, `output_sinks.rs` | The plan renderer matches the goldens. Offline against realtime is checked only for one session in `output_sinks.rs` (`only_main_falls_back_and_a_path_table_without_routes_adds_nothing`, bit for bit) and one arrival in `path_pdc.rs`; a general parity test is planned. |
| Sidechain keys | `crates/mix/tests/sidechain.rs`, `sidechain_hosted.rs` | Key tap, alignment, hosted keyed plug-ins. |
| Plug-in hosts | Fixture plug-ins behind the `test-hooks` cargo feature (`crates/clap-host`, `crates/vst3-host`); `crates/mix/tests/strip_panics*.rs` | Host behavior (threads, faults, teardown) without third-party plug-ins. |
| Quit order, plug-in teardown, process exit, UI-thread panics, graphics device loss | `crates/ui-studio/src/emergency_tests.rs` (child processes with the VST3 fixture's unload mode, `VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER`); `apps/transit/src/shutdown.rs` tests | Every way out of the UI loop runs the bounded quit order and ends within its bound; a binary that was never exited ends in a termination, not a hang in its unload code ([E56](../../escapes/E56-a-lost-graphics-device-froze-the-app-in-a-plug-in-unload.md)). |
| Playback timing, clocks, MIDI ingress, record start | The fake-device harness: `crates/playback/src/device_clock/tests.rs`, `crates/playback/tests/*.rs`, `crates/midi-io/src/fake.rs` | Timing logic under scripted device behavior, no hardware. |
| MIDI recording capture, take construction or placement | `crates/engine/tests/cc_lanes.rs`, `crates/ui-studio/src/midi_record_tests.rs`; retain controller-only and note-bearing passes, preserve original channel/value/timing, and check undo and session round-trip | Controller data is a recorded performance even when no notes were played; empty-performance checks account for all retained event collections ([E53](../../escapes/E53-controller-only-recordings-were-discarded.md)). |
| MIDI echo, thru or output routing of live input | `crates/playback/src/player/native/tests/feedback.rs`: a fake device whose output is looped to its own input; the storm forms without the guards and stops with each | A device that returns what it is sent must not loop: no self-echo by default, no echoed channel-mode messages, and a storm mutes the input port ([E55](../../escapes/E55-midi-feedback-loop-ran-until-the-app-was-closed.md)). |
| Session format | Load tests with old fixtures; `crates/engine/tests/hostile_sessions.rs` | Old sessions load; hostile input does not panic. |
| `crates/rt` (unsafe) | `cargo xtask rt-targets`, the platform legs of `.github/workflows/rt.yml`, a refutation review quoting every `unsafe` block | See `../knowledge/unsafe.md`. |
| UI | `crates/ui-studio/src/*_tests.rs`, the shortcut and icon guards | See `../knowledge/ui.md`. |

## Rules

- A gate that is not run is not a gate. If one cannot run on your machine, say so in the pull request.
- Do not weaken or delete a gate to make a change pass. Fix the cause (see `escape-record.md`).
- Tests must not load third-party plug-ins. Use the fixtures.
- A playback or UI test that renders four or more strips on the audio thread takes the worker path by
  default now. Such a test calls `MixEngine::force_serial(true)` unless it tests the workers, so a loaded
  CI runner cannot silence a strip for a block and fail the test (E49 class).
- A test failure blocks a merge. A suspected flake needs a tracked issue and a green re-run on the
  failing platform before the change merges; "passes locally" or "passes on re-run elsewhere" is not
  enough. The CI conclusion after a push is itself a gate: check it, and a red run is a failure until
  it is explained and fixed (E47, `../../escapes/E47-header-state-jobs-raced-the-rendered-document.md`).
- No wall-clock latency bound under about 100 ms in a unit test. Assert a deterministic signal
  (a counter, a state, an ordering) or a stall bound with its rationale in a comment; a shared CI
  runner will exceed a tight bound sooner or later (E49, `../../escapes/E49-echo-latency-test-wall-clock-bound.md`).
  Enforced by `cargo xtask timing`, which flags in test code (a) a `<`/`<=` bound of `Duration::from_millis(N)`
  with N < 100 or any `Duration::from_micros`, inline or through a `const`/`static`/`let` name bound to one,
  (b) `thread::sleep` in a fn that asserts a rate, BPM, latency,
  `elapsed()` or a loop counter, and (c) a throughput floor `assert!(x >= N)` with N >= 3 on a
  frames/blocks/laps/iterations/polls count or a counter in a timed loop. `#[ignore]` fns are skipped. A
  deliberate case carries `// timing: <tag> <reason>` on the line or the line above, tag one of `stall-bound`
  (a ceiling far above jitter), `floor-ok` (a count not bound to the clock),
  `sleep-ok` (a sleep that does not pace a measurement) or `value` (a bound on a computed value); the reason
  is required. Prefer the deterministic signal, or "at least 2" progress plus a stall bound, over a marker.
  There is no `median` tag: a median over samples is a latency claim, so it lives in an `#[ignore]`
  measurement test, never a gated one (E49 amendment 2026-10-25, #110).
- The timing gate also checks sleep/park/yield polling loops (rule d, #120): a `for`, `while` or
  `loop` with `sleep`, `park_timeout` or `yield_now` must not exit against a short wall-clock
  duration (`Duration::from_millis(N)` with N < 250, or `from_micros`/`from_nanos`). The check
  follows same-file constants and local bindings into loop conditions, exit-controlling `if`
  branches, match guards and assertions, including multiline expressions. Assigning a variable
  read by the loop condition (such as `done = true` in `while !done`) counts as an exit path;
  its guarding expression receives the same duration check as a `break` or `return` branch.
  Numeric comparisons of `.elapsed().as_millis()`, `.as_micros()` or `.as_secs_f64()` also
  count when their literal bound represents less than 250 ms.
  Each sleep/park/yield call paces only its innermost enclosing loop: a nested `for`/`loop`
  pair with a sleep only in the inner loop produces one finding, on the inner loop. An outer
  loop with its own pacing call is checked separately.
  The function-scope heuristic also flags a paced loop when the enclosing function or closure
  passes a sub-250 ms Duration to another call, directly or through a same-file constant or
  local binding. This catches a short TTL configured by `set_limits(...)` even when the loop
  polls only an entry's state under a longer deadline. Calls in other functions or nested
  closures do not contribute. Sleep/park arguments alone do not count, and a function without
  a paced loop is unaffected. Test code means `#[cfg(test)]` modules, `tests/`
  directories, `tests.rs` and `*_tests.rs` files. This check also covers ignored tests.
  Prefer an injected clock (`clock.now()` or a `now` parameter) for expiry/window logic, or a
  generous stall budget while waiting for a state. `Instant::now()` and `.elapsed()` read real
  time; an unused clock parameter does not exempt them. Only `// timing: stall-bound <reason>`
  on the loop line or immediately above it exempts a deliberate short loop. A marker on a
  deadline binding or sleep does not. Keep the state, ordering or expiry assertion meaningful;
  shortening sleeps does not fix scheduler dependence. This is syntax and same-file binding
  analysis, not analysis of timing hidden in called functions, other files or macro expansions.
  The function-scope heuristic accepts an injected read in the loop (`clock.now()` or a `now`
  parameter), or an injected timestamp passed to a polled call. A helper call in the same
  function or closure also counts as injected-clock evidence for that scope's loops when its
  callee's final name contains the case-sensitive substring `clock` (#135). The intended
  allow-list includes `fake_clock`, `set_clock_for_testing` and `with_clock`; qualified calls
  and method calls follow the same substring rule. Calls in another function or nested
  closure do not contribute, and a name in a comment, string or uncalled function reference
  does not count. This is a function-scope naming heuristic, independent of call order; it
  does not prove that the helper installs a clock or executes on every path. Neither a helper
  call nor an injected read exempts a separate short wall-clock exit found by the direct
  duration check.
- A test never derives a rate, BPM or latency from `thread::sleep` pacing. It injects the
  timestamps (the fake input's `deliver_at`) and asserts the exact value (E49 recurrence,
  the MIDI Monitor Clock BPM test).
- Injected timestamps need an injected readout: stamp on the origin the code under test reads
  (not another module's default origin) and read at a time consistent with the stamps, never the
  real clock (E49 recurrence 2026-10-25).
- A file lock is released with an explicit unlock, never by closing the file alone: a child
  process spawned meanwhile holds a copy of the descriptor until its exec, and on Linux and macOS
  the lock goes with it (E57, `../../escapes/E57-an-autosave-lock-outlived-its-session-in-a-spawned-child.md`).
- A "skip what was already pending" cutoff on a worker applies only to the sources it held before
  the cutoff's trigger; a source attached in the same step is new, and its tests include the
  trigger and an attach landing in one step (E58, `../../escapes/E58-an-armed-learn-skipped-the-first-message-of-a-port-it-opened.md`).
- Tests must not read a real clock: a freshness or rate is judged from the message's own arrival
  stamp, never a consumer's clock read when it notices; tests inject the stamp and the readout,
  and a test waiting on another thread waits for every effect it asserts. A related hazard: two
  host origins in one process are two clocks, so stamps and readouts share one origin (E59, `../../escapes/E59-active-sensing-was-stamped-when-the-worker-looked-not-when-it-arrived.md`).
- A test that asserts on the order of two file modification times sets them itself
  (`File::set_modified`), never relying on the real clock between two writes: coarse file-time
  ticks (10 ms at HZ=100) make "later" equal, and a deliberate `>=` then keeps the older item
  (E60, `../../escapes/E60-a-recovered-autosave-test-left-the-save-order-to-the-clock.md`).
- A gate's starved run (WSL) is valid only on a quiet host: no `cargo` or `rustc` process on the
  Windows host when it starts or while it runs. A run with host builds is void and runs again;
  it is neither a pass nor a failure of the tree. A failure on a quiet host is a failure
  (E61, `../../escapes/E61-a-starved-gate-run-shared-the-host-with-cold-builds.md`).
- Tests create unique temp dirs: process id plus a per-process counter (or `TemporaryAutosave`),
  never a fixed name or a clock-only suffix; concurrent tests share a process and a clock tick
  (E51, `../../escapes/E51-test-temp-dirs-collided-in-one-process.md`).
- A test that pumps a fake device faster than real time reads another thread's output (the MIDI
  output thread, a worker) only after waiting for it, never at a fixed block count
  (E52, `../../escapes/E52-measure-cancel-test-assumed-the-output-threads-progress.md`).
- A guard that reads a quantity another thread writes (the input ring's backlog, a counter the
  capture callback bumps) is tested on a fake whose callbacks interleave: capture and render on
  their own schedules, a capture period that differs from the render block, and both orders on a
  tie, plus the real change it must still catch on the same fake. A lockstep fake alone does not
  pass it (E54, `../../escapes/E54-measure-guard-validated-only-on-a-lockstep-fake.md`).

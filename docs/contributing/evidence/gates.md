# Gates

## Every change

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask layers      # crate layering and the no-UI-below-L6 rule (xtask/src/main.rs, LAYERS)
cargo xtask assets      # every bundled asset has a row in ATTRIBUTION.md
cargo xtask timing      # E49-class timing assertions in tests (xtask/src/timing.rs)
```

Other `xtask` commands: `cargo xtask wasm` (web build), `cargo xtask rt-targets` (cross-target check
of `crates/rt`; run it when you touch that crate), `cargo xtask ci` (the CI sequence).

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
- A test never derives a rate, BPM or latency from `thread::sleep` pacing. It injects the
  timestamps (the fake input's `deliver_at`) and asserts the exact value (E49 recurrence,
  the MIDI Monitor Clock BPM test).
- Injected timestamps need an injected readout: stamp on the origin the code under test reads
  (not another module's default origin) and read at a time consistent with the stamps, never the
  real clock (E49 recurrence 2026-10-25).
- A file lock is released with an explicit unlock, never by closing the file alone: a child
  process spawned meanwhile holds a copy of the descriptor until its exec, and on Linux and macOS
  the lock goes with it (E57, `../../escapes/E57-an-autosave-lock-outlived-its-session-in-a-spawned-child.md`).
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

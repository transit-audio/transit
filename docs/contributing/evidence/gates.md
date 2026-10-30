# Gates

## Every change

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features transit/native-wasapi,transit/native-asio -- -D warnings
cargo clippy -p transit-mix -p transit-plugin-host -p transit-dsp -p transit-rt -- -D warnings   # lib targets only: the `testing` feature stays off (#215)
cargo test --locked --workspace --features transit/native-wasapi,transit/native-asio --no-fail-fast
cargo test -p transit-rt --release
cargo xtask layers      # crate layering and the no-UI-below-L6 rule (xtask/src/main.rs, LAYERS)
cargo xtask assets      # every bundled asset has a row in ATTRIBUTION.md
cargo xtask timing      # E49-class timing assertions in tests (xtask/src/timing.rs)
cargo xtask file-size   # no Rust file over 1500 lines; also part of `cargo xtask layers` (xtask/src/file_size.rs)
cargo xtask help        # the help catalog: every registered help id has an entry, every entry is used, the generated docs sections are current (xtask/src/help.rs)
```

Also `cargo xtask rt-targets` (cross-target check) when `crates/rt` or `crates/mix` changed. The
Linux job of `.github/workflows/ci.yml` is the Linux compile proof. transit ships a desktop app only
([ADR 0032](../../adr/0032-transit-ships-a-desktop-app-only.md)): there is no web or wasm32 build, gate or CI job.

## Which checkout a gate checks

A gate checks the checkout `cargo xtask` is run from, not the one the xtask binary was built in (#150): cargo reuses one binary across worktrees that share a `CARGO_TARGET_DIR`, so the root is resolved at run time. Every gate prints it as its first line, for example `layers: root E:/transit-wt-150`. Look for that line in a log before trusting a result.

## One target directory per checkout class

Cargo's fingerprint for a workspace member records the source paths of whichever checkout built it last. With a target directory shared between the clone and its worktrees, the other checkout's files are unchanged, so the unit counts as fresh and the old binary is reused (#159). Observed 2026-10-27: a test binary that lacked a new module's tests, and an `xtask` running another branch's ratchet table.

- The clone's target directory is used only by builds run from the clone: the merge-chain gates and the dev build.
- Worktrees use one separate target directory for pre-hand-back checks, and agents working in worktrees must not share a target directory concurrently (stale or missing artifacts, E0463). After the first run, check the test count of the crate you changed against what you expect.
- An agent that runs alongside others keeps its own target directory.
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

## Help catalog (#178)

`cargo xtask help` checks the in-app help against `docs/help/*.toml` (one file per area). It fails when

- an id in `crates/ui-studio/src/help/ids.rs` has no entry: a widget registered a help id and nobody wrote its text;
- an entry has no id, or its id is never used by a widget outside tests: a dead entry;
- an entry has no name or text, or an id appears twice;
- a generated section of a docs page (`<!-- help:telemetry -->` and `<!-- help:audio-devices -->` in `docs/troubleshooting.md`) differs from the catalog. `cargo xtask help --write` fills those sections, so the figure definitions of the troubleshooting page and the Info panel come from one source.

It runs in the Windows job of `.github/workflows/ci.yml` next to `layers`, `assets` and `timing`, and as a step of `cargo xtask ci`. Adding a control with a help id means adding its entry in the same commit ([`../../help/README.md`](../../help/README.md)).

## Scheduling-condition equivalence (#210)

`crates/mix/tests/sched_equivalence.rs` proves that the render workers' task graph changes nothing but which blocks a strip misses: for every keyed topology, the continuity session and a graph change under a held task, a seeded stall schedule is replayed through deterministic seams (no clock decides who misses, E49), the missed blocks are exactly the counted skips, the output equals a serial render with those blocks muted on every sample, and each strip's state fingerprint (plug-in state, key and delay lines, MIDI) matches at every block boundary. It runs 32 seeds per case by default (`TRANSIT_SCHED_SEEDS=<n>` raises it locally); a failure prints the case, the seed and a minimal schedule found by shrinking. Reproduce one with `TRANSIT_SCHED_SEED=<seed> cargo test -p transit-mix --test sched_equivalence -- --nocapture`; land the minimal schedule as a named `#[test]` that calls `regression(&topology, workers, &Schedule::new(&[...]))` with the printed literal (see `regression_a_chain_held_at_each_link`), named after what it found, in the same change as the fix.

The suite needs the `testing` cargo feature of `transit-mix` (which turns on `transit-dsp`'s): it compiles the seams the gate uses, `Plugin::fingerprint` and the strip state fingerprints, the muted serial blocks, the deterministic deadline and the per-block views (`crates/mix/src/fingerprint.rs`, `sched_seam.rs`, `strip_pool/seam.rs`), and `transit-rt`'s `testing` feature compiles the schedule and its shrinker (`worker_set::schedule`). Each is enabled only by a `[dev-dependencies]` entry (`transit-mix` names itself there), so tests and `--all-targets` lints get it and a release build (`cargo build --release`, `release.yml`) never compiles it; nothing has to be passed by hand. More seeds: `TRANSIT_SCHED_SEEDS=256 cargo test -p transit-mix --test sched_equivalence -- --nocapture`; one seed: `TRANSIT_SCHED_SEED=<n>` (the same command).

## Oracle null tests

How the technique works, start to finish: [`knowledge/oracle-null-testing.md`](../knowledge/oracle-null-testing.md).

A gate class for ported DSP ([ADR 0033](../../adr/0033-built-in-instruments-mutable-instruments-port.md)): the port is rendered next to the original C++ on the same runner, sample by sample, and a residual above the measured floor is an escape. The oracle is `crates/macro-osc-oracle` (`transit-macro-osc-oracle`): the macro oscillator (#194), the resonator (#198) and the granular texture processor (#199), a module per family in one crate. It is dev-only: `cargo xtask layers` fails if any package takes it as a normal or build dependency (`DEV_ONLY` in `xtask/src/main.rs`), and its FFI `unsafe` is confined to `src/ffi.rs`.

- **Build:** the vendored C++ (unchanged; commits and the blob check in the crate's `VENDOR.md`) with `-DTEST`, `-O2` in every profile, no fast-math and no FP contraction (`-ffp-contract=off` on GCC and Clang, `/fp:strict` on MSVC). MSVC needs three small patches; on every compiler, `patches/all/` restores one upstream line lost in a merge (the stretch mode's `Window::Start`) and makes the granular processor's past-the-end reads and one undefined shift defined (padded tables and crossfade tail, the correlator's shift by 32 pinned to 0); all are applied to copies at build time.
- **Sweep:** for each of the 24 engines, 78 engine-level points (float out/aux before post-processing) and 84 voice-level points (int16 frames through the envelopes and low-pass gate): notes 36, 60 and 84, free-running and triggered, harmonics, timbre and morph each at 0, 0.25, 0.5, 0.75 and 1, and the low-pass gate decay. 48 kHz, the firmware's 12-frame blocks, a fresh engine or voice per point, a fixed seed (`src/sweep.rs`).
- **Residual:** peak and RMS of the difference in dBFS (`src/residual.rs`); bit-identical is `-inf`.
- **The resonator** (#198): the oracle wraps the resonator's part (`cpp/resonator_oracle.cc`, on zeroed memory as the module's static storage; `src/resonator.rs`). Its sweep (`src/resonator_sweep.rs`) is 159 points of 100 ms per model and polyphony (6 models, polyphony 1 to 4): notes 36, 60 and 84; the internal exciter strummed, an external impulse unstrummed and strummed; structure, brightness, damping and position each at 0, 0.25, 0.5, 0.75 and 0.9995 (the oracle refuses structure above the module's 0.9995 ceiling and damping at 1, where `Interpolate` would read past a table); two FM offsets; 48 kHz, the firmware's 24-frame blocks, a fresh part and a fixed seed per point. Its floor (`tests/resonator_self_floor.rs`, the sweep twice in one process and again in a second process) is `-inf` on Windows x86-64 MSVC (2026-10-30), and CI proves Linux and macOS. The port gate is `crates/resonator/tests/null_vs_oracle.rs`, one test per model and polyphony (24), both outputs bit-identical and the random state equal after every point; all 24 pass (Windows x86-64 MSVC, 2026-10-30). `tests/scenarios_vs_oracle.rs` holds the same rule for model and polyphony changes, the note filter, macro ramps, every block size, bypass, a noise input, extreme notes and 20 s silences (the subnormal decays, which fail without the port's flush-to-zero). The resonator runs at 48 kHz only, so its pitch test (`tests/pitch.rs`, the E63 rule of the voice gate below) measures every model at 48 kHz.
- **The granular texture processor** (#199): the oracle wraps the processor (`cpp/granular_oracle.cc`, on zeroed memory as the module's globals; `src/granular.rs`), driven as the firmware's harness drives it (`Prepare` after every block), in its granular mode and its pitch-shifter / time-stretcher mode, at four buffer qualities, stereo int16 at 32 kHz (the module's codec rate; the oracle accepts nothing else). Its sweep (`src/granular_sweep.rs`) is 111 points per mode and quality: position, size, pitch, density, texture, dry/wet, spread, feedback and reverb each at five levels around a center (size and dry/wet stop one `f32` step below 1; the firmware still reads one past `lut_window` at a grain's peak, past `lut_xfade_in`/`lut_xfade_out` when the dry/wet ramp overshoots 1.0 by rounding, and past the 256-entry freeze tail on every resume, so the oracle's build gives each of those arrays one more entry, the last repeated or a zero read at zero gain, which is exactly what the port's clamp reads; the oracle also refuses memory over 131072 bytes, where a grain's 16.16 phase would overflow); free-running, frozen from the middle to three quarters, and triggered every 25 blocks; the module's memory; 32-frame blocks; each point the quality's whole recording plus 0.25 s; a fresh processor, a fixed seed and one integer-generated input. Its floor (`tests/granular_self_floor.rs`, all 888 points twice in one process and again in a second process) is `-inf` on Windows x86-64 MSVC (2026-10-30), and CI proves Linux and macOS. The port gate (granular mode) is `crates/granular/tests/null_vs_oracle.rs`, 12 tests (quality by pattern), int16 output at 0 LSB and the random state equal after every point; all 12 pass (Windows x86-64 MSVC, 2026-10-30). `tests/long_run_vs_oracle.rs` (60 s per quality, every parameter moving, freeze toggles, triggers, 10 s of silence) and `tests/scenarios_vs_oracle.rs` (quality changes, every block size, bypass and silence, other memory sizes with the longest transposed grains, full-range ramps) hold the same rule; `tests/pitch.rs` measures a grain's transposition at 32 kHz under the E63 rule. Each sweep point must also sound after its first two blocks, except the ones silent by design (density 0.50 unless triggered; position 1.00 when frozen, at every quality but 16-bit stereo), which the test lists. No test discriminates the port's flush-to-zero yet: the output is int16 and the suites pass with the flush off; a float output path (slice 2) must bring one. The stretch mode's sweep points are in the oracle's floor and wait for their port (#199 slice 2).
- **Floor:** the oracle against itself, the same sweep twice in one process and again in a second process (`tests/self_floor.rs`). Measured `-inf` (bit-identical) on Windows x86-64 MSVC on 2026-10-29; the Linux (GCC, x86-64) and macOS (Apple Clang, AArch64) runners prove theirs on every CI run (the crate is a workspace member, so the `cargo test --locked --workspace` step of each `ci.yml` job runs it; a C++ compiler is a build prerequisite, `docs/building.md`). The constants are in `src/floor.rs`. Results are compared within a runner family only, never across: each has its own compiler and instruction set.
- **Re-measure** after a compiler, flag or vendor change: `cargo test --locked -p transit-macro-osc-oracle --test self_floor -- --nocapture` on each runner. A failure names the engine, the point and the residual; it means the reference itself is unstable, and it blocks every port gate until fixed. Never rerun it to pass.
- **The port gate** (#195) renders each point through the same sweep functions and sets its bound at the floor with margin: `crates/macro-osc/tests/null_vs_oracle.rs`, one test per engine, both outputs, at the engine level. The margin is zero: all 24 engines null bit-identical (Windows x86-64 MSVC, 2026-10-29; the crate README lists the corrections that got them there). The engine tests also require the random state after each point to equal the oracle's (a voice's engines share one generator).
- **The voice gate** (#195): `crates/macro-osc/tests/null_vs_oracle_voice.rs`, one test per engine over the 84 voice-level points, compares the int16 frames, out and aux, exactly: 0 LSB, no tolerance. All 24 engines are identical (Windows x86-64 MSVC, 2026-10-30). `tests/voice_scenarios_vs_oracle.rs` holds the voice to the same rule where the sweep does not reach: engine changes by patch and CV, the level, frequency, timbre, morph and note inputs, the speech and chiptune cases, blocks of 1 to 24 frames, full-range macro ramps on every engine, long silences, re-initialization. One documented difference is pinned there to a single 12-frame block: the firmware's engines share one RAM arena and its six-op engine's accumulator is not reset, so returning to a six-op bank after one of six engines replays leftover memory for one block. Won't fix: on the module, the first 12-frame block (0.25 ms) after returning to a six-op bank from one of six engines adds that engine's leftover memory to the output, which can be a click; transit plays the six-op engine's own tail instead, as the module does after any other engine. Reproducing it would mean emulating every engine's byte layout and the host's pointer size to replay garbage. Recorded in [ADR 0033](../../adr/0033-built-in-instruments-mutable-instruments-port.md#oracle-deviation-note-2026-10-30-the-six-op-engine-after-shared-ram) and the crate README ("The one difference"). The oracle renders at 48 kHz only; `tests/voice_sample_rates.rs` checks that 44.1, 88.2 and 96 kHz keep the 48 kHz pitch within 1 cent on the two-operator FM engine, and `tests/voice_pitch.rs` checks the absolute pitch of every pitch path (the common `a0` path on eight engines, the six-op voices' reference, the LPC speech clock) at every rate under both tunings, within 1 cent. Rule ([E63](../../escapes/E63-the-speech-model-played-out-of-tune-away-from-48-khz.md)): a pitch test compares against the expected absolute frequency with a measurement that rejects octave and lag-multiple aliases (the peak searched near the expected period only, refused if the signal also repeats at a half or a third of it or if its spectrum near half the pitch is within 12 dB of the peak, the frequency read from the spectrum), and covers every engine with its own pitch path at every supported rate. The oracle switches flush-to-zero on only inside its C++ calls; the port runs in Rust's default floating-point mode and reproduces flush-to-zero explicitly in its own code where the reference (the C++ as the firmware's desktop test harness builds it; the hardware runs with flush-to-zero off) relies on it, never by changing the host's mode (Rust treats a changed mode as undefined behavior). Every ported module adds its own oracle crate.

## Rules

- A gate that is not run is not a gate. If one cannot run on your machine, say so in the pull request.
- Do not weaken or delete a gate to make a change pass. Fix the cause (see `escape-record.md`).
- Tests must not load third-party plug-ins. Use the fixtures.
- A playback or UI test that renders four or more strips on the audio thread takes the worker path by
  default now. Such a test calls `MixEngine::force_serial(true)` unless it tests the workers, so a loaded
  CI runner cannot silence a strip for a block and fail the test (E49 class).
- A test that takes the render-worker path and asserts what a strip produced (audio, a panic, an
  event) sets the strip deadline itself: far (`set_strip_deadline(Some(60 s))`) to wait for every
  strip, zero or a held worker to force an overrun. Inheriting the default share is a test defect
  (E49 recurrence 2026-10-29; enforced by `cargo xtask timing` rule (e), below). A test that needs a worker to
  hold one strip holds it (`Shared::hold` in `crates/mix/tests/common/workers.rs`) and waits for that
  strip's `holding` signal, never for "some strip is away"; a continuity count starts at the first
  block (`ContinuityInstrument::expect_from`), so overruns before an instrument's first render are
  holes too, and the closing blocks before a coverage check run at a far deadline (E49 recurrence
  2026-10-30).
- A test that marks the ASIO host stuck (`Host::set_stuck`) holds the host thread in a request
  meanwhile (`hold_host` in `crates/audio-asio/src/stream_tests.rs`): the flag alone leaves the
  thread free to tend and sweep before the test reads its state (E49 recurrence 2026-10-30).
- A failing test is never rerun to make it pass, and a chain never reruns a failed test even to
  capture its message: read the output, report it verbatim and bug-hunt the root cause before any test
  change. Three test defects were found this way on 2026-10-29, and no product defect. The escape
  records are in [`docs/escapes`](../../escapes/), and the process is in [`escape-record.md`](escape-record.md).
- A default changes only on a measurement on the reference hardware. Benchmarks and the decisions
  they made are in [`docs/benchmarks/README.md`](../../benchmarks/README.md).
- A test failure blocks a merge. A suspected flake needs a tracked issue and a green re-run on the
  failing platform before the change merges; "passes locally" or "passes on re-run elsewhere" is not
  enough. The CI conclusion after a push is itself a gate: check it, and a red run is a failure until
  it is explained and fixed (E47, `../../escapes/E47-header-state-jobs-raced-the-rendered-document.md`).
- Every test that starts a thread or takes the worker or host-thread path is audited, one row each, in [`e49-audit-2026-10-30.md`](e49-audit-2026-10-30.md) (#226); add a row for a new such test.
- `cargo xtask timing` rule (e) (#226, `xtask/src/timing_pool.rs`) holds the worker-path rules above for
  test code in `mix`, `rt`, `plugin-host`, `audio-asio`, `playback` and `ui-studio`. A `#[test]` is a pool
  test when it (or a same-file helper it calls, one level, with a helper's deadline parameter bound to the
  call's argument) starts render workers or uses the shared worker rig (`common::workers`) and renders a
  block; it is a host-thread test when it makes a plug-in host thread (`Host::new`, `host(`, `slow_host(`;
  not an inline host) or an ASIO host (`rig_watching(`, `hold_host(`). A bare `transit-rt` worker set is not a
  trigger: `finish` takes each block's deadline. The gate refuses: (1) a pool test with no strip deadline of
  its own, no held worker, no explicit instants (`set_block_deadline`, `finish`, `run_at`) and no serial
  switch (`force_serial(true)` before the first render, never set otherwise), or a host-thread test with no
  give-up of 5 s or more; (2) a short strip deadline (`from_micros`, `from_millis` under 100) with no held
  worker it waits on (`hold` plus a read of `holding`, `entered` or `held_tracks()`), zero deadline or
  explicit instants; (3) a `while ..render_workers_settled()` wait for some strip to be away in a test that
  injects `stall_ns` and asserts `Queued`, `StaleBinding`, `midi_dropped` or `held_tracks` without reading
  `holding`; (4) a quiet-period wait, a loop that exits on `v.elapsed()` for an `Instant` it resets when a
  value moves, in any test fn or helper (a `while` on it always; an `if` that breaks unless it is a
  no-progress guard of 5 s or more followed by an assertion its exit fails, E62; an `if` that panics or
  asserts is a failing guard); (5) a coverage assertion (`assert_time_covered`,
  `hole_samples`, `stuck_notes`) after the last stall or hold release with no far deadline
  (`from_secs(N)`, N at least 10, inline or a same-file name) or serial switch between them. A deliberate case
  carries `// timing: <tag> <reason>` on the fn header or in the attribute and comment lines directly above
  it, reason required: `far-deadline` (refusals 1, 2, 5: the far deadline is set where the scan cannot see
  it), `held-worker` (1, 2), `explicit-deadline` (1, 2: explicit instants, or injected stalls longer than an
  explicit short deadline with assertions that hold for any schedule), `serial` (1, 5), `host-timeout` (1:
  the test's subject is the host's own timeout) and `any-strip` (3). Refusal 4 has no marker: the exit names
  the state. The scan cannot see helpers in another file or a track count passed through one; a runtime
  guard is the follow-up (#226). The scan is keyword-based: review still checks order and targets (its known
  blind spots are listed in the audit doc). This rule closes #220.
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
- A test that waits on a background thread waits on its completion signal (a join, a ticket or
  counter, a channel, a synchronous flush), never on a wall-clock bound (E62, #189: the autosave
  test now calls `Autosaver::flush`).
- An X11 resource handed to another client (a plug-in, a test's observer) is made with a round
  trip first; one client's effect is checked from another only after the first client's round
  trip. The server orders requests per connection only (E64, #147: `EditorWindow::new`, resize
  and close end with `settle`).
- A test that hosts a plug-in through the player or the plug-in host ends with the app's quit
  order (`StudioApp::shut_down`, or the player dropped and then
  `transit_engine::shutdown_plugin_hosts`) and asserts it was clean before its process ends: the
  host thread releases instances in its own time, and a release that overlaps the exit handlers
  leaves a module entered and never exited (E65).
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

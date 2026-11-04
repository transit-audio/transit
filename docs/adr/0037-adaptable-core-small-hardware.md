# ADR 0037: An adaptable core: the desktop DAW is the product, the engine must stay fit for small dedicated hardware

- **Status:** accepted; amended 2026-11-03 (two tiers of portability)
- **Date:** 2026-10-30
- **Reopen when:** a transit product decision adds a second product shape (a dedicated device, a headless service) that the desktop app would have to carry, a core crate needs the UI or plug-in hosting to work, or the small-box benchmark ([#237](https://github.com/transit-audio/transit/issues/237)) shows the core cannot hold its deadlines on the reference board at the track count the desktop app promises.

## Context

transit is a desktop DAW for Windows, macOS and Linux ([ADR 0032](0032-transit-ships-a-desktop-app-only.md)). Its core is layered and the layers are enforced: `time`, `rt`, the audio and MIDI I/O crates, `midi` (L0); `dsp`, the native instruments `macro-osc` and `resonator`, and the plug-in hosts (L1); `model` (L2); `mix` (L3); `engine` and `playback` (L4); `automation` (L5); `ui-studio` (L6) ([`layering.md`](../contributing/knowledge/layering.md), `cargo xtask layers`). The native instruments are deterministic: `forbid(unsafe_code)`, allocation at construction, allocation-free render, and bit-exact against the original reference ([ADR 0033](0033-built-in-instruments-mutable-instruments-port.md), [oracle null testing](../contributing/knowledge/oracle-null-testing.md)). The render runs on a bounded pool with per-strip deadlines ([ADR 0029](0029-strip-job-slots.md)).

On 2026-10-30 the maintainer observed that this core is what a dedicated instrument (a groovebox-class device) needs. What such a device does not need is arbitrary plug-in hosting, or the desktop UI. The observation is about the core's shape, not about a new product.

## Decision

1. **The product target is unchanged.** The product is the desktop DAW. A groovebox or any other dedicated device is not a transit product, and this ADR adds no product scope.
2. **Design principle: performance is measured by the useful musical work accomplished within the hardware available, not by how much hardware the engine can consume.** A change that makes the engine use more cores, memory or threads to do the same work is not a performance gain. The measure is the work done on a stated board, with the deadlines held.
3. **The core builds and runs without plug-in hosting and without the UI.** The core is the engine, mix, model, MIDI, dsp and the native instruments. A headless build gate proves it, and the gate is tracked as [#236](https://github.com/transit-audio/transit/issues/236).
4. **Reference board: Raspberry Pi 5.** Four Cortex-A76 cores at 2.4 GHz, 64-bit Raspberry Pi OS with the PREEMPT_RT kernel, active cooling, the performance governor, class-compliant USB audio, PipeWire. The loaded-run benchmark is recorded on this board for every minor release from v0.7.0, as tracked in [#237](https://github.com/transit-audio/transit/issues/237).
5. **Small hardware means a small Linux computer with `std`.** It does not mean a microcontroller. This ADR makes no `no_std` commitment.
6. **A design choice that ties the core to the UI, to plug-in hosting, or to unbounded memory or threads is flagged against this ADR in review.** The review cites this ADR and asks for the core to keep its shape or the choice to move out of the core.

## Consequences

- The headless gate and the Pi 5 benchmark become v0.7.0 work, with the control surfaces the milestone already carries.
- The arm64 run of the Pi 5 doubles as a second architecture for the bit-exact instrument tests. Floating-point defaults differ on arm64 (flush-to-zero and fused multiply-add behavior), so the oracle floor and the gate must be measured there too, not assumed from x86-64 ([oracle null testing](../contributing/knowledge/oracle-null-testing.md)).
- The README performance text cites the Pi 5 number once it is recorded, under the evidence-visible rule ([ADR 0029](0029-strip-job-slots.md) and the README's "How we decide").
- No product scope is added. The desktop app stays the only shipped build ([ADR 0032](0032-transit-ships-a-desktop-app-only.md)).

## Conventions

The desktop convention ([ADR 0034](0034-racks-layering-instruments-through-track-groups.md), [ADR 0035](0035-disk-backed-audio-sources.md)) does not establish portability to small hardware. That needs its own build and measurement evidence.

## Amendment 2026-11-03: two tiers of portability

The maintainer approved this amendment on 2026-11-03 in response to the question of an embedded
reuse toggle. Decision 5 continues to govern the core, now called Tier A. Tier B adds a narrower
`no_std` commitment for reusable processing code. Decision 1 stands: no firmware, board support,
or promise of a groovebox is added. A Pi 5-class Linux device can use the core; a microcontroller
project can lift an instrument or effect without taking the DAW with it.

The feature names and CI jobs below are the required implementation contract, **not existing
gates** at survey base `de79525d`. This amendment changes documentation only. #236 owns the
headless implementation; the Tier B retrofits land one crate per slice. Until a slice lands with
its green job, that crate is a Tier B candidate, not a demonstrated bare-metal build.

### Tier A: the core with `std`

Tier A is `transit-engine`, `transit-mix`, `transit-rt`, `transit-model`, `transit-midi`, and the
device-independent parts of `transit-playback`. It retains `std`, bounded render workers and
the existing audio-thread contract. Its target remains the small Linux computer in decision 5;
there is no `no_std` commitment for these crates. Supporting dependencies such as `transit-time`,
`transit-audio-io` and `transit-audio-backend` are included transitively, not newly promised for
bare metal. The built-in DSP and instruments used by this core belong to Tier B below.

The escape hatch is the **headless build gate** (#236): build and test this package set plus all
eight Tier B packages, with neither the UI nor plug-in hosting in the resolved graph. Introduce
these additive features; absence of a feature, not a negative `headless` feature, removes it:

| Crate | Required feature contract | Direct host dependencies made optional |
|---|---|---|
| `transit-mix` | `default = ["hosting"]`; `hosting` enables the three optional format hosts | `transit-clap-host`, `transit-vst3-host`, `transit-au-host` |
| `transit-engine` | `default = ["hosting"]`; `hosting` enables its four optional hosts and `transit-mix/hosting` | The three format hosts and `transit-plugin-host` |
| `transit-playback` | `default = ["hosting", "devices"]`; `hosting` enables its three optional hosts and `transit-mix/hosting` | `transit-plugin-host`, `transit-clap-host`, `transit-vst3-host` |

`devices` owns playback's physical device opening, `cpal` (including its Windows dependency
entry), and physical MIDI I/O. Existing `native-wasapi`, `native-asio` and `native-jack` features
imply `devices`. With both defaults off, playback still exposes and tests publication, callback
and transport logic using device-free seams. Its current `test-hooks` remains a testing seam,
not a route to a physical device. Tier A does not require removing OS services from `rt`.

Set `default-features = false` on the engine-to-mix and playback-to-mix dependency edges and on
any other incoming edge that could re-enable `hosting` or `devices`. The package-root flag alone
does not disable dependencies' defaults. Desktop defaults explicitly forward the features so
the shipped app retains its current capabilities. `model`, `midi` and `rt` do not need a new
hosting feature. `transit-plugin-host` itself depends on CLAP and VST3 hosts, but is absent as a
whole in the headless graph; its internal format edges need no headless mode. `ui-studio` has
direct CLAP/VST3 edges and a dev-only `plugin-host` edge; the entire UI crate is outside this gate.

"Without hosting" means no discovery, SDK loading, host thread or hosted editor implementation.
It does **not** remove mix's built-in plug-in slots, the `Plugin` trait, instrument creation,
parameter automation, MIDI effects or built-in registry. A session's unavailable hosted slot
retains its identifier, parameters and saved state; it reports unavailable and follows the
existing missing-plug-in fallback (dry insert, silent instrument), without trying to load a host.
Tests must cover built-ins through these slots and round-trip a session containing an unavailable
hosted slot. No silently discarded state is an acceptable way to make the gate pass.

Host-dependent tests must be feature-gated too. In particular, the current mix `clap-host` and
playback `plugin-host` dev-dependencies cannot remain unconditional: even skipping their tests
would still compile those crates. Move their fixture support behind an opt-in `hosting-tests`
feature that implies `hosting` and the host testing hooks, and set self-dev dependency defaults
off. Run those tests in the existing desktop jobs with `hosting-tests` explicitly enabled. Inspect
normal, build **and dev** dependency graphs for the headless invocation to prevent feature
unification from bringing hosting back.

Add a separate `headless-core` Linux job alongside the existing jobs in `.github/workflows/ci.yml`.
It runs the following build and tests (shell: bash); this is a library gate, not a new app target.
The explicit `std` features keep Tier B's desktop arithmetic and oracle tests enabled:

```sh
packages=(transit-engine transit-mix transit-rt transit-model transit-midi transit-playback
          transit-dsp transit-macro-osc transit-resonator transit-modal-voice transit-texture
          transit-granular transit-modulator transit-random-sequencer)
args=()
for crate in "${packages[@]}"; do args+=(-p "$crate"); done
features=transit-dsp/std,transit-macro-osc/std,transit-resonator/std,transit-modal-voice/std,transit-texture/std,transit-granular/std,transit-modulator/std,transit-random-sequencer/std
cargo build --locked --no-default-features "${args[@]}" --features "$features"
cargo test --locked --no-default-features "${args[@]}" --features "$features"
cargo tree --locked --no-default-features "${args[@]}" --features "$features" -e normal,build,dev
```

The initial headless slice can introduce the eight default-on `std` feature declarations before
the no-default configurations are ready. That makes the feature spelling above usable without
claiming bare-metal support or making #236 wait for the entire retrofit queue.

The job must fail if that graph includes `transit-clap-host`, `transit-vst3-host`, `transit-au-host`,
`transit-plugin-host`, `transit-ui-studio`, or UI dependencies forbidden below L6. Printing the
graph without checking it is insufficient. Native backend and hosted editor tests stay in the
existing platform jobs; the headless job uses no audio device or display.

### Tier B: processing with `no_std` and `alloc`

Tier B comprises the pure processing in `transit-dsp`, `transit-macro-osc`, `transit-resonator`,
`transit-modal-voice`, `transit-texture`, `transit-granular`, `transit-modulator` and
`transit-random-sequencer`. Every future DSP or effect port, including every family selected by
[ADR 0042](0042-stock-effects-airwindows-ports.md), starts in Tier B.

Each has `default = ["std"]` and `#![cfg_attr(not(feature = "std"), no_std)]`, with `alloc` for
owned buffers. Its dependency edges disable defaults; its `std` feature explicitly forwards
dependency `std` features. In particular, modal-voice must not turn resonator's `std` back on,
nor may dsp turn any port's `std` back on in its minimal configuration. No OS clocks, threads,
blocking synchronization, thread-local state or std-only dependency belongs in that configuration.
Musical time and random seeds are explicit inputs, not readings of wall time or global entropy.

Allocation is allowed only in constructors and `prepare`, off the processing path. `process`,
render helpers, event handling, parameter changes and reset reuse bounded storage: no allocation,
reallocation, deallocation or logging. `Vec` and `Box` storage are not themselves violations;
growing a vector or dropping owned storage during processing is. State capture writes to
caller-prepared bounded storage; it must not allocate merely because it is outside a method
named `process`. The embedding application supplies an allocator if it constructs allocating
processors. This is not a promise that every instrument's memory or CPU footprint fits every MCU.

Float math uses `libm` when `std` is off, or stable `core` operations where the pinned toolchain
provides them. Cover both f32 and f64: `sinf`/`sin`, `cosf`/`cos`, `expf`/`exp`, `powf`/`pow`,
`tanhf`/`tanh`, `logf`/`log`, `sqrtf`/`sqrt`, and the additional operations in the survey. Preserve
the existing lookup tables, approximation order and explicit flush-to-zero wrappers. The default
`std` path retains its existing math and oracle floors; do not weaken an oracle test to accommodate
a portability change. Bare-metal compilation alone makes no bit-exactness or timing claim for
the `libm` path; numerical changes need host-side comparison evidence in their retrofit slice.

**Trait decision: use processing traits owned by the processing crates, adapted by `dsp`.**
Retain the present dependency direction: dsp depends on the ports, never the reverse. Each port
exposes its core through a small crate-local processing trait with its existing typed controls,
borrowed input/output slices and explicit event/sample positions; no editor, state-job polling,
registry or dependency on `transit_dsp::Plugin`. MIDI processors use a bounded event sink whose
caller also obeys the no-allocation contract. These traits wrap the current inherent processing
entry points rather than changing their algorithms. In dsp, separate existing built-in processing
cores from their desktop adapters by pure moves first, respecting the file-size ratchet.

`transit_dsp::Plugin`, `PluginInfo`, registry and desktop adapters remain available under `std`.
The no-default dsp library must expose usable pure processing modules (filters, dynamics, delay,
reverb, oscillators, meters and MIDI processing as applicable), not an empty crate that happens
to check. Gate out desktop host-call instrumentation, editor/state jobs, allocating offline
utilities and FFT-based panel analysis; make `rustfft`, `rtrb` and logging optional desktop
dependencies. Keep serialization out of the minimal core or configure it without std defaults.
Separate sample ownership and checkpoint transport from sample rendering and checkpoint encoding;
removing desktop transport must not remove the processor's ability to make sound.

This choice avoids moving the entire desktop trait surface into a new shared crate. IDs are
already borrowed `&str`; `PluginInfo::params` is a static slice, not a vector. `Params` owns two
vectors allocated at construction; `Plugin::process` borrows existing channel vectors. None of
those forces `std`: they can use `core` and `alloc`. The testing-only `fingerprint` takes
`std::hash::Hasher`, which can be spelled `core::hash::Hasher` in portable processing seams.
The real coupling is the trait's editor/state surface and the surrounding host instrumentation,
clocked freeze-state readers, logging and analysis dependencies. Keeping that adapter surface
under `std` preserves built-in hosting in Tier A while letting a port stand alone.

### Bare-metal gate and retrofit order

Add a separate, cheap `dsp-bare-metal` job on Linux in `.github/workflows/ci.yml`, alongside
`headless-core` and the desktop platform jobs, not inside their workspace test matrix. Install
the stable minimal toolchain and `rustup target add thumbv7em-none-eabihf` in that job. Use its
own target cache key. The final gate is exactly these per-package library checks:

```sh
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-dsp
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-macro-osc
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-resonator
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-modal-voice
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-texture
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-granular
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-modulator
cargo check --locked --target thumbv7em-none-eabihf --no-default-features -p transit-random-sequencer
```

Use neither `--workspace` nor `--all-targets`: no test binaries, C++ oracle, emulator, firmware
link, board support or hardware run belongs in this job. `cargo check` type-checks the target
library; it does not prove a runnable image or MCU resource budget. Existing tests, allocation
probes and ADR 0033 oracle comparisons continue under default `std` in the platform jobs.

New DSP crates enter the bare-metal job in their first slice. Retrofit existing crates in this
order, adding each to the enforced job in the same slice; no allowed failure and no marking an
unconverted crate portable. The eight-command list above is the completion criterion. Record
the remaining candidates explicitly while the job's enrolled package list grows. Enrolled so far
(implementation note, 2026-11-04): `transit-instrument-core`, the shared instrument primitives
[ADR 0041](0041-sampler-one-instrument-three-views.md) moved out of dsp (#269), a new crate that
started in Tier B; its check runs as a step of the linux job until the separate job lands.
Candidates, in order:

1. `resonator` — **small**: no production dependencies; core imports, alloc imports and two math wrappers.
2. `modal-voice` — **small**, after resonator: forwards its feature, changes five files' imports and one direct sine call; delay storage already survives reset.
3. `random-sequencer` — **small**: core imports and sqrt/power/rounding math; its time is already passed in and its queue is a fixed array.
4. `granular` — **small**: core/alloc imports, math wrappers and buffer-size rounding; no production dependency.
5. `modulator` — **small**: core/alloc imports and math wrappers, including vocoder setup; no production dependency.
6. `texture` — **medium**: core/alloc imports and math, plus bound checkpoint writers so caller capacity cannot turn capture into an allocation.
7. `macro-osc` — **medium**: core/alloc imports, math, disable `num-traits` defaults and replace six-op `OnceLock` with immutable precomputed algorithms or caller-owned prepared algorithms; no new unsafe or lazy initialization in render.
8. `dsp` pure processing — **large**, after all seven ports: split adapters from cores, isolate desktop dependencies, migrate math and parameter/event storage, retain useful no-default exports and prove both configurations.

These are scope estimates from source, not elapsed-time promises. Each slice keeps the default
behavior covered and includes the new compile proof. An unrelated change need not finish all
retrofits, but must not deepen a candidate's blockers. Tier A's headless implementation is a
separate slice and need not wait for all Tier B conversions.

### Layering and review rule

Portability is a second axis in [`layering.md`](../contributing/knowledge/layering.md), not a new
layer number. Tier B processing stays at L1, Tier A spans L0/L2/L3/L4, and desktop adapters are
marked separately even when they share the dsp package. Keep the `LAYERS`, `STANDALONE` and
dev-only oracle rules enforced by `cargo xtask layers`. The current layers gate checks dependency
direction, not feature-resolved `no_std` capability; the bare-metal job supplies that proof.
New Tier B dependencies must satisfy both the layer rule and the no-default compile gate.

**A Tier B crate that gains a std-only dependency, a clock, a thread or an allocation in process
is flagged against this amendment.** The remedy is to keep that behavior in a desktop adapter
or prepare it outside processing, not silently remove the crate from the gate. This applies to
unconverted candidates too. Decision 6 still governs Tier A; none of these rules relaxes the
audio-thread contract, unsafe policy or file-size budget.

### Appendix: source survey at `de79525d`

Paths and line numbers below refer to that base, before any retrofit. Read-only inspection used
`CARGO_TARGET_DIR=E:/transit-targets/codex-adr37` and `cargo tree --offline --locked -e no-dev -p
transit-<crate>` for every candidate and the hosting owners. No build or test was run. The `std::`
counts are files with literal matches anywhere under `src/`, including comments and tests; they
are **not** counts of production blockers. Allocation entries mean no allocating operation found
on the inspected prepared processing paths, not a newly executed allocation proof.

| Crate (all `transit-`) | Production dependencies; `std::` uses by kind | Math to route when std is off | Allocation in process; evidence | Retrofit / blocker |
|---|---|---|---|---|
| `resonator` | None (`crates/resonator/Cargo.toml:12`). 6 files: ops/cmp, array, mem, float constants; one comment-only file. | f32 `sqrt`, `powf` at `crates/resonator/src/float.rs:56` and `:63`; callers include `fm_voice.rs:31` and `:172`. The direct tangent at `stmlib/filter.rs:224` is test-only. | **No**: `crates/resonator/src/part.rs:641` processes prepared buffers; `:191` documents reset reuse, `stmlib/delay_line.rs:37` takes and clears existing storage. | **Small**; imports and math, no dependency blocker. |
| `modal-voice` | `resonator` (`crates/modal-voice/Cargo.toml:15`). 5 files: constants, mem, array. | f32 `sin`, `crates/modal-voice/src/dsp.rs:172`; inherited `F` math must come from the converted resonator. | **No**: `crates/modal-voice/src/part.rs:386`; boxed delays allocated at `fx.rs:38`, `:199`, `tube.rs:27`, retained at `voice.rs:74`. No heap `String` in processing (`ResonatorModel::String` is an enum). | **Small**; blocked on resonator's feature forwarding. |
| `random-sequencer` | None (`crates/random-sequencer/Cargo.toml:12`). 3 files: ops/cmp, array. | f32 `sqrt`/`powf`, `float.rs:64`; f64 `ceil`/`floor`/`round` and f32 rounding, `sequencer.rs:423`, `:475`, `:669`, `:701`, `:794`, `:810` under `crates/random-sequencer/src/`. | **No** internally: `crates/random-sequencer/src/sequencer.rs:404`, fixed queue push at `:540`; the supplied `emit` callback must also be allocation-free. | **Small**; no production dependency; transport stamps are inputs, not OS clocks. |
| `granular` | None (`crates/granular/Cargo.toml:12`). 2 files: ops/cmp and constants. | f32 `sqrt`/`powf`, `crates/granular/src/float.rs:61`; f64 `round`, `processor.rs:151`. Direct tangent and extra sqrt calls in stmlib are tests. | **No**: `crates/granular/src/processor.rs:454`/`:468`; owned memory allocated at `audio_buffer.rs:89`, `fx.rs:40`/`:239`, not grown in process. | **Small**; core/alloc imports and math. |
| `modulator` | None (`crates/modulator/Cargo.toml:12`). 4 files: ops/cmp/constants; converter's match is test-only. | f32 `sqrt`/`powf`, `crates/modulator/src/float.rs:63`, `:70`; direct setup sqrt at `vocoder.rs:284`. Converter sine and filter tangent are tests. | **No**: `crates/modulator/src/modulator.rs:124` and `:154`; boxes created at `:79` and `vocoder.rs:66`. | **Small**; core/alloc imports and math. |
| `texture` | None (`crates/texture/Cargo.toml:12`). 4 files: ops/cmp/constants, mem size; grain's std partial-sort reference is a comment (its array match is a test). | f32 `sqrt`/`powf`, `crates/texture/src/float.rs:63`, `:70` and `grain.rs:317`; f64 `round`, `lib.rs:146`. Processor sine and filter tangent matches are tests. | **No** in `crates/texture/src/processor.rs:336` or `synth.rs:395`; delay allocation at `delay_line.rs:116`. **Separate hazard:** checkpoint APIs append to caller `Vec<u8>` (`checkpoint.rs:14`, `processor.rs:129`, `synth.rs:348`) and can allocate without sufficient capacity. Retrofit must bound these APIs. | **Medium**; no dependency blocker, checkpoint contract and retained storage need audit. |
| `macro-osc` | `num-traits` 0.2.19, with host build helper `autocfg` (`crates/macro-osc/Cargo.toml:14`). 13 files: Box/vec imports, fmt/ops, six-op `OnceLock`. | f32 `sin`/`tan` at `crates/macro-osc/src/utils/filter.rs:473`/`:99`, `cos` at `utils/cosine_oscillator.rs:34` and `engine2/six_op_engine.rs:327`, **associated** `f32::sqrt` at `utils/mod.rs:145` (a dot-call search misses it). | **No** found in `crates/macro-osc/src/voice.rs:436`; six-op buffers allocated at `engine2/six_op_engine.rs:64`. `OnceLock::get_or_init` is in init at `:134`, not render, but still prevents the portable configuration. | **Medium**; disable `num-traits` std defaults; remove OnceLock without adding render-time initialization or unsafe. |
| `dsp` pure processing | `serde`, `log`, `rustfft`, `rtrb`, all seven ports (`crates/dsp/Cargo.toml:15`). 37 files, including tests: core-replaceable constants/hash/array/mem; Arc; TLS/AtomicU64; Duration/Instant and sleep in desktop state readers. | Both float widths: sin/cos/tan, exp/exp2, powf/powi, tanh, ln/log2/log10, sqrt, atan2/sin_cos and rounding/fraction operations. Examples: `crates/dsp/src/biquad.rs:53`, `lib.rs:679`, `plugins/harmonic.rs:68`, `plugins/filter/svf.rs:73`, `plugins/reverb.rs:299`; see inventory below. | **No** found in prepared built-in render paths (`crates/dsp/src/plugins/dynamics.rs:118`, `modal_voice.rs:919`, `texture.rs:465`). Channel `Vec`s are borrowed; params allocate at `params.rs:52`; events are capacity-bounded and reused at `plugins/instrument_queue.rs:57`/`:73`. Freeze capture writes into preallocated loans (`plugins/texture_synth_state.rs:67`/`:140`); caller-capacity hazards remain to audit. String formatting in `plugins/tuner.rs:81` is panel work, not process. | **Large**; all seven ports plus desktop dependencies/trait surface. `Plugin` is an adapter obstacle for dsp, **not** a dependency of any leaf port. |

The full literal `std::` inventory, grouped by file (paths relative to each crate's `src/`):

- `resonator`: `float.rs:25, 66, 73, 173`; `note_filter.rs:99`; `part.rs:167, 168, 173, 200, 201, 206, 697`; `reverb.rs:160`; `stmlib/delay_line.rs:37`; `stmlib/filter.rs:26, 33, 224, 234`.
- `modal-voice`: `dsp.rs:13`; `fx.rs:46, 217`; `modal.rs:62, 109`; `tube.rs:36`; `voice.rs:84`.
- `random-sequencer`: `float.rs:33, 74, 81, 168`; `sequencer.rs:273`; `xy_generator.rs:157`.
- `granular`: `float.rs:30, 71, 78, 165`; `stmlib/filter.rs:26, 33, 169, 179`.
- `modulator`: `converter.rs:122`; `float.rs:32, 73, 80, 167`; `stmlib/filter.rs:26, 33, 239, 249`; `xmod.rs:127`.
- `texture`: `checkpoint.rs:16`; `float.rs:32, 73, 80, 167`; `grain.rs:177, 270`; `stmlib/filter.rs:26, 33, 172, 182`.
- `macro-osc`: `engine/hihat_engine.rs:17, 18`; `engine/mod.rs:93`; `engine/modal_engine.rs:16, 17`; `engine/noise_engine.rs:15, 16`; `engine/particle_engine.rs:15, 16`; `engine/speech_engine.rs:22, 23`; `engine/string_engine.rs:16, 17`; `engine/virtual_analog_engine.rs:17, 18`; `engine2/phase_distortion_engine.rs:14, 15`; `engine2/six_op_engine.rs:11`; `engine2/wave_terrain_engine.rs:15, 16`; `fx/mod.rs:22`; `speech/lpc_speech_synth_controller.rs:6, 7`.
- `dsp`: `biquad.rs:4, 50, 178, 185, 204`; `editor_header.rs:118, 119`; `event.rs:217, 218`; `host_call.rs:18, 19, 20, 80`; `lib.rs:41, 358, 500, 551, 553, 559, 574`; `meter.rs:47, 83, 85`; `midi_fx/random_sequencer.rs:159`; `offline.rs:7, 43, 44, 66, 220, 239, 443`; `osc.rs:3`; `pan.rs:3, 182, 232, 364`; `pitch_detect_tests.rs:11, 21`; `pitch_detect.rs:144, 145, 187, 286`; `plugins/dynamics.rs:110, 216`; `plugins/eq.rs:83`; `plugins/filter.rs:510`; `plugins/filter/svf.rs:12, 35, 38, 73`; `plugins/filter/tests.rs:85, 86, 104, 110, 286, 299, 427, 518`; `plugins/harmonic.rs:68`; `plugins/instrument_queue.rs:74`; `plugins/lfo.rs:157`; `plugins/macro_osc_retrigger_tests.rs:41, 44`; `plugins/modal_voice_tests.rs:10, 446, 490, 496`; `plugins/modal_voice.rs:712, 792`; `plugins/resample.rs:29`; `plugins/resonator_tests.rs:9, 343, 377, 383`; `plugins/reverb.rs:187, 189, 299, 307`; `plugins/sampler.rs:9, 277, 441`; `plugins/texture_synth_state.rs:27, 98`; `plugins/texture_synth_tests.rs:9, 273, 312, 353, 422`; `plugins/texture/freeze_state.rs:12, 234`; `plugins/texture/tests.rs:7, 253, 336`; `plugins/time_stretcher.rs:71`; `plugins/time_stretcher/tests.rs:7, 246, 309`; `plugins/tuner_tests.rs:7, 106, 113`; `plugins/utility.rs:94`; `spectrum.rs:20, 46`; `util.rs:3, 269, 278`.

Desktop-only hazards are concrete: `crates/dsp/src/host_call.rs:20` uses `AtomicU64` and `:22`
uses TLS (64-bit atomics are also a target-width obstacle); `plugins/texture/freeze_state.rs:219`
reads `Instant`, `:223` logs and `:234` sleeps; `plugins/texture_synth_state.rs:88` reads
`Instant` and `:98` sleeps. These are control-side readers, not evidence of sleeping in process.
No `Mutex` or `HashMap` use was found in the eight candidate source trees, and no clock, thread
or `log::` macro in the seven leaf ports. The six-op `OnceLock` is their synchronization exception.

For completeness, the float-method scan of all `dsp/src` (including tests and desktop-only
utilities) found `sin` 53, `cos` 28, `exp` 8, `powf` 27, `tanh` 3, `ln` 11 and `sqrt` 28 call
sites; also `atan2` 1, `ceil` 5, `exp2` 3, `floor` 13, `fract` 3, `log10` 9, `log2` 23,
`powi` 3, `round` 36, `sin_cos` 9, `tan` 6 and `trunc` 2. These are lexical counts, not counts
of replacements: tests keep std and `core` availability depends on the toolchain. Production
anchors include `crates/dsp/src/biquad.rs:53` (f64 sine/cosine), `:55` (power), `:64` (sqrt),
`:115` (log10), `util.rs:31` (exp), `:102` (powi), `pan.rs:99` (atan2) and
`params.rs:90` (round), with paths relative to `crates/dsp/src/` after the first citation.
The seven leaf rows enumerate their production math families; none has a direct exp, tanh or
ln call. Searches covered both method and associated-function spellings and distinguished
lookup/approximation functions from standard float methods.

The dependency tree places `rustfft`'s numeric/transpose helpers under dsp, and serde's derive
proc-macro dependencies on the build host. Do not confuse host-side build helpers with target
std dependencies. The crate-local C++ oracle is `transit-macro-osc-oracle`, shared across the
ported families: see candidate manifests' dev-dependency sections, `crates/macro-osc-oracle/build.rs:1`
and `xtask/src/main.rs:118` (`DEV_ONLY`). It stays test-only with `std` and C++; it is never in the
bare-metal checks. Future effect oracles follow the same rule.

Trait and registration evidence: `crates/dsp/src/lib.rs:166` (`PluginInfo`), `:276` (`Plugin`),
`:299` (borrowed vectors), `:358` (testing fingerprint), `:439` (editor surface),
`crates/dsp/src/params.rs:43` (owned parameter vectors), `crates/dsp/src/plugins/mod.rs:26`
(registry macro) and `:60` (ported instrument adapters). Modal-voice was inspected throughout
`src/`, including its generated static sample/table modules; its owned delay buffers and reset
reuse, not the numerical table literals, determine the allocation estimate.

Hosting evidence: `crates/engine/Cargo.toml:15`, `crates/mix/Cargo.toml:15`,
`crates/playback/Cargo.toml:18`, `crates/plugin-host/Cargo.toml:12` and
`crates/ui-studio/Cargo.toml:19`; current dev edges at `crates/mix/Cargo.toml:29` and
`crates/playback/Cargo.toml:61`. Full `cargo tree -e no-dev` inspection confirms playback reaches
AU transitively through mix, while engine references it directly. CI insertion point:
`.github/workflows/ci.yml:36` (`jobs`); keep the existing platform tests. Layer registration:
`xtask/src/main.rs:56`, standalone list `:86`, dev-only oracle list `:118`.

The requested `gh --repo transit-audio/transit issue view 236` failed with network access denied:
`Post "https://api.github.com/graphql": dial tcp 140.82.112.6:443: connectex: An attempt was made to access a socket in a way forbidden by its access permissions.`
Its live text was not available; the feature names above are this amendment's specification,
not names verified from #236.

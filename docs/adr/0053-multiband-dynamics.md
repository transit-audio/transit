<!-- transit-index
title: Multiband Dynamics, a built-in three-band dynamics insert that compresses in both directions in each band: a zero-latency Linkwitz-Riley 4th-order three-way split on trapezoidal state-variable sections with the low band allpass-compensated (magnitude-flat, allpass phase at zero Depth; bit-exact only through insert bypass); a stereo-linked peak detector, hard curves with a fixed 6 dB knee, downward above an upper threshold and upward below a lower one (3:1 upward lifts 3 dB below to 1 dB below), boost capped at 36 dB; Depth, Downward and Upward scale the gain change in dB; Time scales every attack and release; per-block band meters leave the audio thread through atomics in the insert's scope tap; a new Tier B crate; no sidechain; factory preset "Over the top" equals the defaults
-->
# ADR 0053: Multiband Dynamics

- **Status:** proposed 2026-11-11
- **Date:** 2026-11-11
- **Implementation:** not started. Slices in the Slices section. Issue #448 (this device, milestone v0.8.0); #437 and #277 (portable math).
- **Reopen when:** the portable math record (#437, #277) lands and changes a helper this device names; a user asks for a sidechain, mid/side detection, a fourth band or a detector choice; the linear-phase split is asked for at mastering with the latency it costs; the CPU target in decision 11 is missed; or tuning by ear shows that the fixed knee, detector fall times or boost cap must become parameters.

## Context

Issue #448 asks for one device, named Multiband Dynamics: three bands, each compressed in both directions at once. Downward compression above an upper threshold pulls loud peaks down; upward compression below a lower threshold lifts quiet detail up. With aggressive settings it gives the hyper-compressed sound widely used in electronic music; with gentle ones it is a mastering and sound-design tool. Effects get a decision record first, and this device is transit's own design, not a port ([ADR 0042](0042-stock-effects-airwindows-ports.md) covers ports; this is not one).

**The panel is designed.** The design hand-off for #448 adds a "Multiband Dynamics" section to `docs/design/language/README.md`, a `multibandDynamics` block to `docs/design/language/tokens.json` and four boards (collapsed, expanded, meter, states). It is not on `main` yet; it lands before slice 3. It fixes the controls and what the engine must report: per band per block the largest boost and the largest reduction applied (dB, each 0 or more) and the band's input and output peak, and the device's input and output peaks per channel. This record makes the engine deliver exactly that and does not redesign the panel. Panel facts quoted here come from that section.

**Settled by the maintainer (2026-11-11, #448).** All seven open points of the hand-off are decided:

1. One hue (`mod`) for both directions of gain change; the direction carries the meaning.
2. The gain-change meter spans plus or minus 24 dB, linear. Past that it pins with a cap and the readout prints the true value.
3. The board's "Over the top" values are the initial preset and default values, to be tuned by ear once built (decision 9 lists them as initial values, not tested constants).
4. Time runs from x0.10 to x10, bipolar, x1.00 at center; gains run plus or minus 24 dB; the crossovers have ranges and a minimum spacing, defined here (decision 2.4).
5. An upward ratio of 3.0:1 lifts a signal 3 dB below the lower threshold to 1 dB below it.
6. More than one band may be soloed at once.
7. The minimum window width is measured once built.

**How built-in effects work today** (read on `main` at 3d276383):

- A built-in is a `Plugin` (`crates/dsp/src/lib.rs`) with a static `PluginInfo` (`id`, `name`, `short_name`, `category`, `params`, `pages`) registered in one table, `registry!` in `crates/dsp/src/plugins/mod.rs`. Parameters are static `ParamInfo` tables built with `lin`, `log`, `toggle` and `choice` (`crates/dsp/src/params.rs`), stored in `Params` and reached through `param_plumbing!`, which calls the device's `update()` on every `set_param`. `ParamInfo` has no "not automatable" flag: every parameter can be automated, MIDI-learned and mapped by a rack macro.
- The mix gives a built-in its parameter values at the block start (`apply_params` and `apply_values`, `crates/mix/src/automation.rs`): one `set_param` per changed value, on the audio thread. Built-ins do not take timed points (`sample_accurate_params` is false). Automation lanes address a parameter by insert slot and id (`AutoParam::Plugin { slot, param }`). Rack macros arrive through the same path.
- The existing Compressor/Limiter (`crates/dsp/src/plugins/dynamics.rs`) detects on the stereo-linked peak (`level.max(s.abs())` over channels, non-finite samples skipped), computes a soft-knee gain reduction in dB (`compress_gr`), smooths that reduction in the dB domain with a one-pole attack and release (`ballistic`), and converts with `db_to_gain` per sample. `time_coef` (`crates/dsp/src/util.rs`) turns milliseconds into a one-pole coefficient with `exp`. `db_to_gain` and `gain_to_db` (`crates/dsp/src/lib.rs`) call `powf` and `log10` from the platform. The Expander/Gate (same file) adds a peak envelope with an instant rise and a 5 ms fall (`env = level.max(env * env_rel)`).
- The Filter insert ([ADR 0038](0038-inserts-read-the-tracks-note-stream.md), `crates/dsp/src/plugins/filter.rs`) is the block-size precedent: everything runs per sample on the plug-in's own sample clock, its cutoff is smoothed in the log2 domain, and coefficients are recomputed every `CONTROL` (16) samples of that clock, so the output is bit-identical whatever the host block size for the same input and the same parameter values at the same absolute frames.
- Its state-variable sections live in `transit-instrument-core` (`crates/instrument-core/src/svf.rs`): the trapezoidal-integrator SVF with a prewarped cutoff (`Coefs::g_of`, `tan` through the crate's `math` module), clamped below Nyquist (`max_cutoff`), integrator states flushed of denormals and non-finite values (`util::flush`). `Section::tick` returns one mode per call. The older `Biquad` (`crates/dsp/src/biquad.rs`, transposed direct form II, RBJ formulas in f64) re-designs its coefficients keeping its state. There is no crossover anywhere in the workspace.
- `transit-instrument-core` routes `sin`, `cos`, `tan`, `exp`, `exp2`, `ln`, `log2` and `powf` through one `math` module (`crates/instrument-core/src/math.rs`): the platform's methods with `std`, `libm`'s without. That module is the seam a portable implementation will replace (#437, #277). It is `pub(crate)`.
- **Meter data.** The plan compiler gives each insert instance a `ScopeTap` (`crates/mix/src/scope.rs`): a preallocated ring of the per-frame peak of the insert's output (the largest channel), the last block's gain reduction and detector level, and a few instrument words, all atomics, written by the render with relaxed stores. The UI reads it through `ScopeHub` by track and insert uid. The strip pushes it after the insert runs (`crates/mix/src/strip.rs`, `tap.push(..., slot.plugin.gain_reduction_db(), slot.plugin.detector_level_db())`). It holds no per-channel peaks, no device input level and no per-band data. A value "as of the last block" is overwritten by the next block, so a UI frame sees only the last block before it.
- **Bypass.** A bypassed insert is skipped by the strip (`continue` before `run_insert`), so it neither processes nor pushes its tap; the audio passes bit-exact.
- **Suspension.** `run_insert` suspends only hosted effects on silent input; built-ins always run.
- **Failure containment.** `contain_failure` (`crates/mix/src/strip.rs`) silences a block that holds NaN or infinity and takes the plug-in out until the next reset.
- **Presets.** Built-in presets are small JSON files (`crates/ui-studio/src/presets.rs`): `{"plugin", "version": 1, "params"}`; factory presets live in `assets/presets/<plugin-id>/<name>.json` and are compiled into `presets::FACTORY`; recalling one sets every parameter, the unlisted ones to their defaults.
- **Session.** An insert is stored as `Insert { plugin: <registry id>, params: BTreeMap<String, f32>, bypass, active, preset, ... }` (`crates/model/src/mixer.rs`). `mix.insert` fills defaults and clamps given values into range; `mix.insert_param` clamps one value into its range (`crates/engine/src/cmd/mix.rs`). An unknown plug-in id fails to instantiate (`create_plugin` in `crates/mix/src/instance.rs` returns `None` and logs the failure).
- **Pages.** Every device with parameters gets factory macro pages beside its parameter table ([ADR 0047](0047-device-macro-strip.md), `PageInfo` and `PageSlot` in `crates/dsp/src/lib.rs`).
- **Mixer solo.** `mix.solo` (`crates/engine/src/cmd/mix.rs`) is additive by default (latch); it clears the other solos only when the solo mode contains X-OR or the call passes `exclusive`.
- **Meter widget.** `widgets/level_meter.rs` (`crates/ui-studio/src/widgets/level_meter.rs`) holds `Ballistics::PEAK_FALLOFF` (0 ms up, 300 ms down) and `Ballistics::VU` (300 ms each way) and a stateless `meter_step` the caller runs each frame.
- **Tier B.** Every new DSP crate starts in Tier B: `no_std` plus `alloc`, a `std` default feature, allocation only in constructors and `prepare`, and enrollment in the bare-metal compile job in its first slice ([ADR 0037](0037-adaptable-core-small-hardware.md)).
- **Portable math is decided for the project.** A bounce should be bit-identical across operating systems; a later record designs portable math so no rendered path calls the platform C runtime (#437, maintainer's comment; #277 for the built-in devices). Until then a pinned cross-platform hash may cover only IEEE-exact arithmetic.

**Conventions.** Read in the current manual of one reference workstation, for its three-band dynamics device: up to three bands, each with an Above and a Below threshold and ratio, giving downward and upward compression and expansion; per-band attack, release, an on/off activator, a solo button and input and output gain; a global Amount ("At 0%, each compressor/expander has an effective ratio of 1"); a global Time that "scales the durations of all of the Attack and Release controls"; a Peak or RMS detector switch; a Soft Knee switch; a sidechain; "Soloing a band mutes the others". That manual does not state the crossover filter type or slope, a spacing rule, the Time range, how an upward ratio maps levels, whether several bands can be soloed at once, latency, flat summing or a cap on upward gain. The other reference workstation's multiband compressor could not be read for this draft (its effects guide did not load); everything about it is unverified, including whether it compresses upward at all. So there is no verified agreement between the two beyond the shared basics (per-band thresholds, ratios, times, gain, on/off and solo, adjustable crossovers); where this record cites a convention it means only what is verified above.

## Decision

### 1. One device, its identity and its home

- **Registry id `multiband_dynamics`**, name "Multiband Dynamics", short name "Multiband", `Category::Dynamics`, an effect insert (`is_instrument: false`), `audiosuite: true` (the device is stateless across a render's start and has no latency, so offline processing is the same render). No other product's name appears in an id, a parameter, a preset or the panel.
- **The DSP lives in a new Tier B crate, `transit-multiband-dynamics`** (`crates/multiband-dynamics`): the split, the detector, the gain computer, the ballistics and the per-block meter values, with `default = ["std"]`, `#![cfg_attr(not(feature = "std"), no_std)]`, `alloc` for per-channel state, allocation only in `prepare`. It enters the bare-metal compile job in slice 1 (ADR 0037). It owns a `math` module like `transit-instrument-core`'s, naming exactly the functions of decision 7.
- **The `Plugin` wrapper lives in `transit-dsp`** (`crates/dsp/src/plugins/multiband.rs`): parameter table, pages, `update`, `process`, the meter hook. `transit-dsp` depends on the new crate with `std` named explicitly, as it does for the instrument crates.

### 2. The band split

**2.1 The choice: a Linkwitz-Riley 4th-order three-way split with the low band allpass-compensated, on trapezoidal state-variable sections.**

With `LP4(f)` and `HP4(f)` the 4th-order Linkwitz-Riley low and high pass (each a cascade of two 2nd-order Butterworth sections at `f`) and `AP(f) = LP4(f) + HP4(f)` (a 2nd-order allpass with Q = 1/√2):

- Low = `LP4(f1) · AP(f2) · x`
- Mid = `HP4(f1) · LP4(f2) · x`
- High = `HP4(f1) · HP4(f2) · x`
- Low + Mid + High = `AP(f1) · AP(f2) · x`

The sum has unit magnitude at every frequency. The identity holds for any `f1` and `f2`, so the spacing rule (2.4) exists for the bands' meaning, not for flatness.

Per channel this is seven SVF section ticks per sample: one Butterworth section at `f1` gives both its low and high output from one state; one more low-pass section completes `LP4(f1)` and one more high-pass section completes `HP4(f1)`; the same three at `f2` on the high branch; one allpass section at `f2` on the low branch (the SVF's allpass output, `v0 - 2k·v1` in the notation of `svf.rs`, with k = √2). The new crate holds its own multi-output section: `transit-instrument-core`'s `Section::tick` returns one mode per call, and changing a shared primitive would bring the full workspace gate for no gain. Its coefficients use the same design (`g = tan(π·f/fs)`, prewarped, the cutoff clamped below Nyquist as `max_cutoff` does) and its states are flushed of denormals and non-finite values as `util::flush` does.

**2.2 What "transparent" means.** At Depth 0, with every band on, no band soloed and every gain at 0 dB, the output is the input through `AP(f1) · AP(f2)`: magnitude-flat, with the allpass phase response of the split. It is not bit-exact. The device never switches the split out at Depth 0: a switch would jump the phase when Depth leaves 0, which is a click. Bit-exact pass-through is the insert's bypass, which the mix already gives (the strip skips the insert). Measured criterion (Evidence): the magnitude response of a Depth-0 render lies within ±0.01 dB of unity from 20 Hz to 0.45·fs, and its phase matches the analytic `AP(f1)·AP(f2)` within 0.5 degree, at 44.1, 48, 96 and 192 kHz.

**2.3 Latency.** None. `Plugin::latency` returns 0. `tail_samples` keeps the default (0): built-ins are never suspended (`run_insert` suspends hosted effects only), and the device's output decays with its input (decision 3 never lifts digital silence; the split's own ring is the only tail).

**2.4 Ranges, spacing and "checked when set, clamped when used".**

- Low · Mid (`xover_low`): 30 Hz to 2 kHz, log taper, default 90 Hz. Mid · High (`xover_high`): 200 Hz to 16 kHz, log taper, default 2.5 kHz (the hand-off's `crossover.defaults`).
- **Minimum spacing: Mid · High at least one octave above Low · Mid** (`xover_high ≥ 2 · xover_low`). Closer than that the mid band's two slopes overlap until it no longer has a passband of its own (proposed).
- **Checked when set.** The panel's crossover knobs stop at the spacing: a drag of one never pushes the other. `mix.insert_param` refuses, for this device only, a crossover value that breaks the spacing against the other stored crossover, naming the limit; it does not move the other one (a device-specific check through a new `transit_dsp::builtin_check_param(id, params, param, value) -> Result<(), String>`, `Ok` for every other device). `mix.insert` with both crossovers in `params` checks the pair the same way. Proposed.
- **Clamped when used.** Automation lanes, MIDI Learn, rack macros, preset files and session files are never refused. The DSP uses `f1u = min(xover_low, F2_MAX / 2)` and `f2u = clamp(xover_high, 2 · f1u, F2_MAX)` with `F2_MAX = min(16 kHz, max_cutoff(fs))`. The stored values are kept; the band headers show the used ones; `session.inspect` reports the stored values (the protocol has no per-device "used" field, and adding one is not needed for this device).

**2.5 Moving a crossover.** Each crossover's target is smoothed in the log2 domain (a `Smoother` with a 30 ms time constant, ticked per sample), and both splits' coefficients are recomputed every 16 samples of the plug-in's own sample clock from the smoothed values (the Filter's `CONTROL` rule), plus once in `prepare` and `reset`. The trapezoidal SVF keeps its states meaningful across coefficient changes, which is why it is used instead of the direct-form `Biquad`. The click criterion is in Evidence.

### 3. Detector, gain computer and ballistics (per band)

**3.1 Detector.** One detector per band, stereo-linked across all channels the device processes: `d[n] = max over channels of |band_c[n]|` (non-finite samples cannot reach it, decision 10). A peak envelope follows it with an instant rise and a fixed exponential fall: `e[n] = max(d[n], e[n-1] · r_b)`, the Expander/Gate's form, with the fall time per band fixed at Low 20 ms, Mid 5 ms, High 5 ms (proposed; constants, not parameters). The fall bridges the zero crossings of the band's own waveform, which a raw peak would see as level drops that the upward curve would answer with boost: 20 ms covers a half cycle down to 25 Hz. The level is `L = 20·log10(e)` in dB, computed as `LOG2_TO_DB · log2(e)` with `LOG2_TO_DB = 20·log10(2)`, floored at -144 dB like `gain_to_db`.

**3.2 The two static curves.** With `Td`, `Rd` the upper threshold and downward ratio and `Tu`, `Ru` the lower threshold and upward ratio, hard-knee forms:

- **Downward (reduction, dB ≥ 0):** `D(L) = (L - Td)·(1 - 1/Rd)` for `L > Td`, else 0. The output level is `Td + (L - Td)/Rd`: at 8:1, a level 8 dB above `Td` leaves 1 dB above it.
- **Upward (boost, dB ≥ 0):** `U(L) = (Tu - L)·(1 - 1/Ru)` for `L < Tu`, else 0. The output level is `Tu - (Tu - L)/Ru`: at 3.0:1, a level 3 dB below `Tu` leaves 1 dB below it (the maintainer's reading, settled point 5).
- **Between the thresholds:** both are 0; unity.
- **Thresholds crossed** (`Tu > Td`, which the threshold bar allows and draws as the overlap taking both fills): both curves apply independently and the band's gain change is the boost minus the reduction.

**3.3 Knee.** A fixed soft knee of 6 dB at each threshold (the Compressor's default knee), the quadratic form of `compress_gr`: within `|L - Td| ≤ 3`, `D = (1 - 1/Rd)·(L - Td + 3)² / 12`; mirrored for the upward curve within `|Tu - L| ≤ 3`, `U = (1 - 1/Ru)·(Tu - L + 3)² / 12`. Not a parameter: the panel has no knee control (proposed).

**3.4 The upward ceiling.** An upward compressor lifts the noise floor and tails without limit unless capped. The boost is capped at **36 dB** (`BOOST_CAP_DB`, a fixed constant, proposed). It is above the meter's 24 dB, so the meter's pinning (settled point 2) is reachable, as the hand-off expects ("full Upward on a quiet tail can lift past it"). With the Over the top Low band (`Tu` = -40 dB, 3:1) the cap is reached at a level of -94 dBFS. Digital silence stays silence: the gain multiplies zero.

**3.5 Ballistics.** The reduction and the boost are smoothed separately, each a one-pole in the dB domain like the Compressor's `ballistic`:

- **Attack applies when the signal gets louder in the band's terms; release when it gets quieter.** Reduction state `r`: attack while its target is above `r` (more reduction), release otherwise. Boost state `b`: attack while its target is below `b` (less boost, the level rose), release otherwise (more boost, the level fell). So a band lifts its tails at the release speed and stops lifting a new transient at the attack speed.
- **Time** scales both: effective attack = base attack × Time, effective release = base release × Time, each at least 0.01 ms. Coefficients come from `time_coef` (decision 7: `exp`), recomputed every 16 samples of the sample clock from a Time smoothed in the log2 domain (30 ms), so a Time sweep is not zippered.
- **Applied gain change (dB):** `G = depth_s · on_s · (up · b - down · r)`, where `depth_s`, `up`, `down` are the smoothed Depth, Upward and Downward amounts (0..1) and `on_s` is the band's on/off ramp (decision 4). The band's gain is `exp2(G / LOG2_TO_DB)` per sample, times the band's Gain.

**3.6 Depth, Downward and Upward scale the gain change in dB.** Scaling the dB change by `a` is the same as an effective ratio `R'` with `1 - 1/R' = a·(1 - 1/R)`: at 0 every band has an effective ratio of 1, which is the convention the reference manual states for its global amount. The output stays the sum of the three bands, each multiplied by a gain, so no dry signal is mixed with a phase-shifted wet one: the phase response is always the split's allpass, at every Depth. A dry/wet crossfade against the raw input would comb-filter (the bands carry the split's phase and the dry signal does not); a crossfade against the unprocessed band sum would be phase-coherent but is parallel compression with another static curve (rejected, Alternatives). Depth scales only the dynamic gain change: Input, Output and the band Gains are level controls outside it, so Depth 0 with band Gains at 0 dB is decision 2.2's transparent state (proposed; listed under Open for the maintainer).

**3.7 Signal order.** Input gain → split → per band: detector (on the band after Input) → gain change × band Gain → solo mute → sum → Output gain. The band input peak is measured after the split and before the gain change; the band output peak after the gain change and the band Gain, before the solo mute (so a band muted by another band's solo keeps its meters running, as the States board shows).

### 4. Solo and band on/off

- **Solo:** a toggle per band. While any band is soloed, every band that is not soloed is left out of the sum. More than one band may be soloed (settled point 6). This matches the mixer's default solo, which is additive (`mix.solo` without X-OR). The device does not follow the X-OR solo mode option: that option is for tracks, and a band solo is a listening control inside one device (proposed). The reference manual says "Soloing a band mutes the others" and does not say whether several can be soloed.
- **On/off** (the band's power button, `*_on`): off removes the band's gain change and its Gain (the band passes as split, unprocessed). The panel dims its controls and its gain-change meter reads `off`; its level meter keeps running.
- **No clicks.** Each band's solo mute and on/off are 5 ms linear ramps on the sample clock (a mute) and on `on_s` (the gain change and the Gain move to unity over 5 ms), so a toggle never steps the output.
- Both are ordinary parameters, saved with the session and automatable like every built-in parameter (`ParamInfo` has no way to say otherwise).

### 5. Parameters

35 parameters. Ids are stable and saved. "Smoothed" means a per-sample one-pole of the stated time on the plug-in's sample clock; the dB gains are smoothed as linear gains like the Compressor's makeup.

| Id | Name | Unit | Range | Default (= Over the top) | Taper | Smoothing |
|---|---|---|---|---|---|---|
| `depth` | Depth | % | 0 to 100 | 100 | linear | 20 ms |
| `time` | Time | factor (x) | 0.10 to 10 | 1.00 | log (x1.00 at center) | 30 ms, log2 domain |
| `downward` | Downward | % | 0 to 100 | 100 | linear | 20 ms |
| `upward` | Upward | % | 0 to 100 | 100 | linear | 20 ms |
| `input` | Input | dB | -24 to +24 | 0 | linear | 20 ms |
| `output` | Output | dB | -24 to +24 | 0 | linear | 20 ms |
| `xover_low` | Low · Mid | Hz | 30 to 2,000 | 90 | log | 30 ms, log2 domain; coefficients every 16 samples |
| `xover_high` | Mid · High | Hz | 200 to 16,000 | 2,500 | log | 30 ms, log2 domain; coefficients every 16 samples |
| `low_thr_down` | Low Down Threshold | dB | -60 to 0 | -20 | linear | 20 ms |
| `low_thr_up` | Low Up Threshold | dB | -60 to 0 | -40 | linear | 20 ms |
| `low_ratio_down` | Low Down Ratio | :1 | 1.0 to 100 | 8.0 | log | 20 ms |
| `low_ratio_up` | Low Up Ratio | :1 | 1.0 to 20 | 3.0 | log | 20 ms |
| `low_attack` | Low Attack | ms | 0.1 to 500 | 35 | log | coefficients every 16 samples |
| `low_release` | Low Release | ms | 5 to 3,000 | 120 | log | coefficients every 16 samples |
| `low_gain` | Low Gain | dB | -24 to +24 | 0 | linear | 20 ms |
| `low_solo` | Low Solo | toggle | off/on | off | | 5 ms ramp |
| `low_on` | Low On | toggle | off/on | on | | 5 ms ramp |
| `mid_*` | Mid ... | as Low | as Low | thresholds -16 / -34, ratios 8.0 / 3.0, attack 15, release 80, gain 0, solo off, on | as Low | as Low |
| `high_*` | High ... | as Low | as Low | thresholds -24 / -46, ratios 8.0 / 3.0, attack 5, release 40, gain 0, solo off, on | as Low | as Low |

- Thresholds and ratios are smoothed in the value they hold (dB, ratio) before the gain computer, which runs per sample, so a dragged threshold handle never steps the gain.
- **Time's unit.** `ParamInfo` has no factor unit. `time` is `Unit::None` (generic display `1.00`); the panel prints `x1.00` and the band detail's effective-time sub-label (`x 0.50 → 17.5 ms`). No new `Unit` variant (proposed).
- **The collapsed panel's controls map one to one** onto parameters: Depth, Time, Downward, Upward, Input, Output, the two crossovers, each band's Gain, Solo and On. The expanded Band detail adds each band's two thresholds (the threshold bar's handles), two ratios, Attack and Release. No collapsed control is a macro over expanded ones: Time and the two amounts scale at render time (decision 3); the stored per-band values never change when they move. The band detail shows base times; the sub-label shows base × Time.
- **Not parameters:** the knee (6 dB), the detector fall times, the boost cap (decision 3), and the panel's PEAK · AVG meter choice (decision 6.4).
- **Factory pages** (ADR 0047), in `PAGES`: `Main` (Depth, Time, Downward, Upward, Input, Output, Low · Mid, Mid · High); `Bands` (Low Gain, Mid Gain, High Gain, Low Solo, Mid Solo, High Solo, and two empty slots); `Low`, `Mid`, `High` (Down Threshold, Up Threshold, Down Ratio, Up Ratio, Attack, Release, Gain, On). Proposed.

### 6. Meter data for the panel

**6.1 What the device computes per block.** For each band: the largest boost and the largest reduction applied in the block, in dB (each ≥ 0, after Depth, the amounts and On: what the band's gain actually did; the reduction counted from `down · r` and the boost from `up · b`, both × `depth_s · on_s`), the band's input peak and output peak (linear, linked over channels). For the device: the input peak per channel (before Input gain: the signal arriving) and the output peak per channel (after Output gain). Values are finite and ≥ 0 by construction (decision 10).

**6.2 How it leaves the audio thread.** A new `Plugin` method, `fn band_meters(&self, out: &mut BandMeters) -> bool` (default `false`, read after `process` on the audio thread like `pad_activity`; must be cheap and allocation-free), with `BandMeters` a fixed struct in `transit-dsp`: `bands: [[f32; 4]; 3]` (boost dB, reduction dB, input peak, output peak) and `io: [[f32; 2]; 2]` (input L/R, output L/R). The `ScopeTap` gains 16 preallocated `AtomicU32` cells (counted in `ScopeTap::BYTES`). The strip writes them where it pushes the tap today (`crates/mix/src/strip.rs`, both insert paths), only while the tap is enabled, with `fetch_max` on the value's bits. For finite non-negative `f32`, bit order is value order, so `fetch_max` keeps the largest value since the last read. The UI reads each cell with `swap(0)` once per frame through a new `ScopeHub::band_meters(track, uid) -> Option<BandMeters>`. So a frame sees the largest change of every block since the previous frame, never only the last block, which is what PEAK's "jumps out to any larger change within the frame" needs. `fetch_max` and `swap` are lock-free read-modify-writes; nothing allocates or blocks. Proposed.

**6.3 Channel layouts on the In and Out meters.** Mono: one value, drawn in both wells. Stereo: channel 0 and channel 1. More than two channels: the left well shows the largest even-indexed channel and the right well the largest odd-indexed one (proposed). The device processes every channel the strip has (up to `MAX_CHANNELS`), all linked.

**6.4 Ballistics.** The engine publishes raw block maxima; the panel applies the ballistics the tokens name (`multibandDynamics.ballistics`): PEAK (default) is `Ballistics::PEAK_FALLOFF`, AVG is `Ballistics::VU`, each half of a gain-change meter stepped on its own with `meter_step`, the holds (1000 ms, `gainChange.hold.holdMs`) and the over-range cap drawn by the panel, the readout printing the true held value. The PEAK · AVG choice is UI state of the open panel, per instance, not saved and not a parameter (proposed).

**6.5 Bypassed.** The strip skips a bypassed insert, so no meter data arrives. The States board says the gain-change meters fall to the line (the panel lets them fall, since it knows the insert is bypassed) and "the level meters show the dry signal". The In and Out wells can: proposed, while a bypassed insert's tap is enabled, the strip writes the passing block's per-channel peaks into the tap's In and Out cells (equal, since nothing changes it). The band level meters cannot without running the split, which bypass must not do; they fall empty. This differs from the board and is listed under Open for the maintainer.

### 7. Determinism and the transcendental functions

The device must be able to render bit-identically across operating systems once the portable math record lands. Every transcendental call goes through the new crate's `math` module, and only these:

- **Per sample, per band (signal path):** `log2` (detector level to dB, decision 3.1) and `exp2` (gain change to linear gain, decision 3.5). Six calls per sample for three bands, independent of the channel count (the detector is linked).
- **At control rate, every 16 samples:** `tan` (two crossover prewarps), `exp2` (the log2-domain smoothed crossover and Time values back to Hz and factor), `exp` (six one-pole coefficients from the effective attack and release times, `time_coef`'s formula).
- **When a parameter is set:** `exp2` for dB gains (Input, Output, band Gains) converted as `exp2(dB / LOG2_TO_DB)`, and `log2` for the log2-domain smoother targets; `exp` for the smoothers' and the detector falls' coefficients in `prepare`.
- Nothing else: no `powf`, `log10`, `sin` or `cos`. The dsp-wide `db_to_gain` and `gain_to_db` (platform `powf` and `log10`) are not used in the render, so this device does not add to #277's list.

When the shared portable helpers exist, the `math` module forwards to them and nothing else changes. Until then the desktop build uses the platform's functions through the module, and no cross-platform golden is pinned over a render that calls them (decision 12).

### 8. Block size, sample rate, reset and silence

- **Block size.** Everything runs per sample on the plug-in's own sample clock; coefficient updates fall every 16 samples of that clock, never on host block boundaries. The output is bit-identical whatever the host block size for the same input and the same parameter values at the same absolute frames (the Filter's rule). Parameter values reach built-ins at block starts (`apply_params`), so automation is as block-quantized as it is for every built-in today.
- **Sample rates.** Coefficients and times are designed from the prepared rate; the crossovers are clamped below Nyquist as `max_cutoff` does. Decision 2.2's flatness is tested at 44.1, 48, 96 and 192 kHz. A rate change goes through `prepare`.
- **Reset** (`Plugin::reset`): split states, detector envelopes, reduction and boost states and the meter maxima to zero; smoothers snapped to their targets; the sample clock to 0 and the coefficients recomputed. **`prepare`** allocates per-channel state for `sane_channels(channels)` and calls `reset`. `snap_initial_params` keeps the default (reset's snaps).
- **Silence and denormals.** Split states are flushed below 1e-25 as `util::flush` does; the envelopes are floored at the -144 dB level; a silent input gives an exactly silent output, whatever the boost.

### 9. The factory preset and the defaults

- **"Over the top"** is a factory preset file, `assets/presets/multiband_dynamics/Over the top.json`, compiled into `presets::FACTORY`. Its values are the parameter defaults of decision 5, so a fresh insert already is Over the top, with no preset load at insert time. A test pins that every preset value equals the default.
- **Initial values, not tested constants** (settled point 3): Depth 100 %, Time x1.00, Downward and Upward 100 %; Low -20 / -40 dB, Mid -16 / -34 dB, High -24 / -46 dB (down / up threshold); 8.0:1 down and 3.0:1 up in every band; attack / release Low 35 / 120 ms, Mid 15 / 80 ms, High 5 / 40 ms; crossovers 90 Hz and 2.5 kHz; Input, Output and band Gains 0 dB. These are the board's values. Input and Output gain are not on the board; 0 dB is proposed. Every value is tuned by ear once the device is built, in the slice that lands the preset (slice 6), and the record of that pass names what changed.
- Whether the title bar names "Over the top" on a fresh insert is listed under Open for the maintainer.

### 10. Hostile input

Non-finite input samples (NaN, infinity) are read as 0 at the device's input: the split, the detector and the meters never see them, and the output stays finite, so `contain_failure` never takes the device out because of its input. Out-of-range parameter values are clamped by `Params::set` (NaN becomes the default), as for every built-in.

### 11. CPU cost

Per channel and sample: seven SVF section ticks, three band gains and a sum; per sample: three linked detectors and six transcendental calls. Target: stereo at 48 kHz in `plugin_bench` (`crates/dsp/examples/plugin_bench.rs`) costs at most four times the Compressor/Limiter at the same block size, measured on the implementer's machine and recorded in slice 2 (proposed; a recorded measurement, not a CI gate, as the bench is wall-clock).

### 12. Sidechain

Out of scope. `builtin_takes_key` stays false for this device. A sidechain would add: `has_key_input`, a key split through a second copy of the crossovers (so each band detects on the key's band), the key mapped onto the strip's channels by the existing `KeyBank`, and a decision whether the key drives one band, all bands, or per band. It would not change the signal path or the meters.

### 13. Session format and older builds

- An insert of this device is an ordinary `Insert { plugin: "multiband_dynamics", params }`. No new session field, no `format_version` change, no `needs` entry, no migration shim (alpha).
- An older build reading such a session: `create_plugin` finds no such id and logs a failed creation; the slot plays without the device; the stored parameters stay in the `Insert` and are written back unchanged on save (a reading of `crates/mix/src/instance.rs` and `crates/model/src/mixer.rs`, to be confirmed by a test in slice 2). That is the same as any plug-in missing on a machine, so a `needs` marker would add nothing.
- A parameter id once shipped never changes meaning. While alpha, a change to an id is a clean break said in the release notes, not an alias.

### 14. Evidence an implementation must carry

All tests are deterministic, use no wall clock and no audio device, and drive time by counting samples. Each negative control says what fails without the rule.

- **Flat sum at Depth 0** (slice 1). Impulse response of the split's sum at 44.1, 48, 96 and 192 kHz and three crossover pairs (90 / 2,500, 30 / 60, 2,000 / 16,000): magnitude within ±0.01 dB of unity from 20 Hz to 0.45·fs; phase within 0.5 degree of the analytic `AP(f1)·AP(f2)` (computed in f64 in the test). Negative control: the same sum without the low band's allpass shows a dip of more than 1 dB near `f2`. And the device at Depth 0 equals the bare split sum sample for sample (in process).
- **Each static curve** (slice 1). The gain computer evaluated in dB with constant input levels: downward 8:1 at `Td` + 8 gives 7 dB reduction; upward 3:1 at `Tu` - 3 gives 2 dB boost; between the thresholds 0; crossed thresholds give boost minus reduction; the knee's value and slope continuous at both edges; the cap at 36 dB. These are basic arithmetic on given dB values (no transcendental call), so they may be pinned as exact values. Negative control: the inverted upward reading (3 dB below lifted to 3·3 = 9 below, or by 3 dB) fails the 2 dB expectation.
- **Attack and release timing** (slice 1). A level step fed to one band's gain state: the reduction reaches 63.2 % of its step after attack × Time samples within one sample, the boost likewise after release × Time when the level falls; Time x0.10 and x10 scale both; the detector fall times as stated. Counted in samples, compared in process against the analytic one-pole.
- **Block-size independence** (slice 1 for the core, slice 2 for the device). The same input and constant parameters at block sizes 1, 7, 64 and 512 give bit-identical output and identical end state (`Plugin::fingerprint`). Negative control: a test build that recomputes coefficients at block starts instead of on the sample clock differs at block size 7.
- **No allocation on the audio thread** (slice 2). A counting allocator over `process`, `set_param` of every parameter, `reset` and `band_meters`, as `crates/dsp/tests/modulator_alloc.rs` does; zero allocations. Bare-metal compile of the crate (slice 1).
- **Automation without clicks** (slices 1 and 2). Measurable criterion: input a 100 Hz sine at -6 dBFS at 48 kHz; step one parameter at a block start (Depth 0 to 100 %, Time x0.10 to x10, a crossover 90 Hz to 400 Hz and 2.5 kHz to 600 Hz, a band Gain -24 to +24 dB, a threshold -60 to 0 dB, solo and On toggled); the output high-passed at 12 kHz (fourth-order Butterworth, in the test) holds no sample above -60 dBFS in the 50 ms after the step. Negative control: the same step with the parameter's smoothing time set to 0 exceeds -60 dBFS. Compared in process.
- **Meter values** (slice 2). A tone in one band only: its band input peak equals the tone's peak, the other bands' are far lower; a known level above `Td` reports the computed reduction and no boost; below `Tu` the boost; with Depth 0 both are 0; a band switched off reports 0 gain change and a running output peak; `fetch_max` publication keeps the largest of three blocks pushed between two reads and `swap` clears it; the cells are written only while the tap is enabled.
- **Hostile input** (slice 1). NaN and infinity in the input give finite output and unchanged state afterward (negative control: the Compressor's path passes NaN through); a run of 1e-30 values leaves no denormal in any state; silence in gives exactly zero out with the full boost dialed in.
- **Crossover spacing** (slice 2). `mix.insert_param` refuses a `xover_high` below twice the stored `xover_low` and the reverse, with the document unchanged; an automation lane that crosses them renders with the clamped pair and keeps the stored values.
- **Preset** (slice 2). The factory preset parses and every value equals its parameter's default; recalling it after edits restores every parameter.
- **Older builds** (slice 2). A session whose insert has an unknown built-in id round-trips through load and save with its parameters unchanged.
- **Pinned results.** Only IEEE-exact values: the parameter table and pages, the preset values, the static curves on given dB inputs, the spacing clamp, the `fetch_max` publication. Every audio render that passes through the split or the gain (they call `tan`, `log2`, `exp2` and `exp`) is proven by an in-process comparison (against an analytic f64 reference or a second render), never by a pinned hash, until the portable math record lands.

## Slices

Each builds, passes the gates and is useful or invisible on its own. Docs ride with each landing that changes public behavior.

1. **DSP core.** The `transit-multiband-dynamics` crate (Tier B, bare-metal job): the multi-output SVF section, the three-way split, detector, curves, knee, cap, ballistics, Time, Depth and amounts, solo and On ramps, input sanitizing, per-block meter values, the `math` module. Tests: flat sum, curves, timing, block size (core), hostile input, clicks (core level). Invisible.
2. **Device and parameters.** The `Plugin` wrapper in `transit-dsp`, the parameter table and pages, registry entry, `Plugin::band_meters` and `BandMeters`, the `ScopeTap` cells and `ScopeHub::band_meters`, the strip's writes (including the In and Out cells while bypassed, if decided), `builtin_check_param` and the spacing refusal, the factory preset file, the CPU measurement. Tests: allocation, block size (device), clicks (device), meters, spacing, preset, older builds. Useful: the device is insertable, automatable and works with the generic panel and the protocol.
3. **Panel, collapsed.** After the design hand-off lands: the `multibandDynamics` tokens in `crate::tokens` (ADR 0045), the hero's main column, band columns with headers (range following the used crossovers), solo and power buttons, Gain knobs, crossover gutters and knobs, In and Out knobs, the Band detail bar collapsed with its summary; meters drawn as empty wells. Tests: panel layout against tokens, controls write the right parameters, the crossover knob stops at the spacing.
4. **Meters.** The level meter's gain-change variant and the band level meter as the README specifies, the In and Out wells, clip cells, the PEAK · AVG control, holds and the over-range cap, the bypassed, soloed and band-off states. Tests: the widget's geometry and pinning from given values, `meter_step` per half, the states' dimming.
5. **Band detail.** The threshold bar (handles as parameters, fills, overlap, input level, drag, type and reset), the detail knobs, the effective-time sub-labels. Tests: handle drags write thresholds, the overlap draws both fills.
6. **Preset, help and docs.** The tuning pass by ear on Over the top (values updated in the preset and the defaults together, the pass recorded), help entries for the device and its meters, the user guide's section, the README Features line, the minimum window width measured by the established method and recorded.

Order: 1 before 2; 2 before 3; 3 before 4 and 5; all before 6.

## Consequences

- transit gets a multiband dynamics device with zero latency and an exact flat sum at Depth 0, at the cost of the split's allpass phase, which a linear-phase split would avoid with latency.
- The meter path gains an accumulate-and-swap publication that keeps every block's maximum for the panel; other devices can use the same cells later.
- The device names its transcendental functions, so the portable math record can make it bit-identical across operating systems by replacing one module.
- One more built-in parameter table that is long (35 entries); pages (decision 5) keep the macro strip usable.
- A device-specific parameter check enters `mix.insert_param` (if decided), the first of its kind for inserts.

## Alternatives rejected

- **Linear-phase FIR split.** Flat in magnitude and phase, but a band edge at 30 Hz needs an impulse response of tens of thousands of taps (latency of hundreds of milliseconds at 48 kHz, or partitioned convolution with its own latency and cost), crossover automation means redesigning and crossfading filters, and the latency enters delay compensation for every track it sits on. Reopen for a mastering mode if asked.
- **Subtractive (complementary) split** (High = x - LP(x) and so on). Sums exactly to the input, bit for bit, at Depth 0, but the derived bands have poor stopband rejection (a 2nd-order low pass leaves the high band with a 6 dB per octave skirt and a magnitude bump around the crossover), so each band's compressor reacts to its neighbors' energy and the bands' gain changes leak across the crossovers. The exact sum is lost anyway as soon as any band's gain differs from unity.
- **Butterworth or other non-Linkwitz-Riley IIR splits.** Do not sum flat (a 3 dB bump or dip at each crossover).
- **Direct-form biquads for the split** (the existing `Biquad`). Usable at fixed frequencies, but coefficient changes under a running state disturb a direct form more than a trapezoidal SVF, and crossover automation is required.
- **RMS detector, or a Peak/RMS switch** (the reference device has one). A switch adds a parameter and a second set of tuning; RMS reacts late to transients, which the downward curve exists to catch. A peak envelope with a per-band fall gives the aggressive default and stays smooth enough for the upward curve. Reopen if asked.
- **Mid/side or per-channel detection.** Linked detection keeps the image still, as the existing dynamics do; mid/side is a mastering feature for a later decision.
- **Depth as a dry/wet crossfade** against the raw input (comb filtering against the split's phase) or against the band sum (parallel compression, another curve than the ratios state).
- **A boost cap as a parameter, or no cap.** No cap lifts the noise floor without bound; a parameter adds a control the panel does not have. Fixed at 36 dB, reopen if tuning shows otherwise.
- **Bit-exact bypass at Depth 0 by switching the split out.** Steps the phase when Depth leaves 0; the insert's bypass already gives a bit-exact path.
- **The X-OR mixer solo mode for band solo.** A track-level preference; band solo is local to the device.

## Open for the maintainer

The seven design points are settled (Context). These gaps were found while writing the record; each has a recommendation and stands as "proposed" in the decision text until decided.

1. **Bypassed metering differs from the States board.** The board says the level meters show the dry signal when the device is bypassed. The In and Out wells can (the strip writes the passing block's peaks); the band level meters cannot, because bypass does not run the split. Recommend: In and Out show the dry signal, band level meters fall empty, the board updated to match. Alternative: run the split for metering while bypassed (costs CPU on a bypassed insert and breaks "bypass skips the insert").
2. **Depth scales only the dynamic gain change**, not Input, Output or the band Gains. Recommend as written (Depth 0 with band Gains at 0 dB is the transparent state; Gains are tone and level controls). Alternative: Depth also scales the band Gains, so Depth 0 is always transparent whatever the Gains.
3. **Crossover spacing of one octave, refused by `mix.insert_param` and clamped when used.** Recommend as written. Alternative: no refusal anywhere, only the panel's knob stop and the clamp at use (simpler, but departs from "checked when set").
4. **Fixed knee (6 dB), detector fall times (20 / 5 / 5 ms) and boost cap (36 dB)** as constants, not parameters, tuned in the slice 6 pass. Recommend as written.
5. **Upward ratio range 1.0:1 to 20:1** (downward 1.0:1 to 100:1, the Compressor's). Recommend: 20:1 is effectively "everything below lifted to the threshold" within the 36 dB cap.
6. **The title bar on a fresh insert.** The defaults equal Over the top, but no other device writes a preset name on insert. Recommend: `mix.insert` writes `preset: "Over the top"` for this device so the title bar names what the user hears; alternative: no special case, the title bar shows no preset until one is chosen.
7. **The PEAK · AVG choice is panel state, not saved.** Recommend as written; alternative: saved per insert (it would need a field outside the parameter table, since a parameter would be automatable).
8. **More than two channels on the In and Out wells** fold even and odd channels. Recommend as written; it only affects buses wider than stereo.

## Related

[ADR 0037](0037-adaptable-core-small-hardware.md) (Tier B), [ADR 0038](0038-inserts-read-the-tracks-note-stream.md) (the Filter's sample-clock rule), [ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md) (the inherited dynamics stay), [ADR 0042](0042-stock-effects-airwindows-ports.md) (ported effects, not this device), [ADR 0045](0045-design-language-in-code.md) (tokens), [ADR 0047](0047-device-macro-strip.md) (pages), [ADR 0023](0023-session-compatibility-markers.md) (why no `needs` entry), [ADR 0041](0041-sampler-one-instrument-three-views.md) ("checked when set, clamped when used"). Issues #448, #437, #277.

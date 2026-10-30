# ADR 0033: Built-in instruments: a port of the Mutable Instruments family

- **Status:** proposed 2026-10-29 (maintainer decision)
- **Date:** 2026-10-29
- **Reopen when:** the crates.io evaluation finds no acceptable starting point and the Rust port proves too costly to carry; the upstream license terms change; the null-test residual floor cannot be measured stably on CI hardware; or a product question asks for engines the family does not cover.

## Context

transit has no built-in instruments beyond what it inherited. The maintainer wants wavetable and granular synthesis. Writing generic engines from scratch would mean months of sound design with no reference to judge them against.

Émilie Gillet's open-source Mutable Instruments DSP (the `eurorack` repository on GitHub, code under the MIT license) is a coherent, proven family:

- Plaits: a macro oscillator with virtual analog, wavetable, FM, additive, physical-model and drum engines. Firmware 1.2 adds six-operator FM, wave terrain, string machine and chiptune.
- Rings and Elements: physical modeling.
- Clouds: granular texture synthesis.
- Warps: a meta-modulator, which is an effect.
- Marbles and Grids: random and drum sequencers, which are MIDI effects.

The modules were built to run on a Cortex-M4, so CPU cost is low, and they share one design language.

## Decision

Port the family as small, focused built-in instruments and effects. They are mono by design where the originals are. Users build bigger instruments by composition: a track chain (an instrument plus inserts) and a group of tracks, the rack of [ADR 0034](0034-racks-layering-instruments-through-track-groups.md).

Order: Plaits first, with a voice allocator and a small editor, as the first slice of v0.6.0. Then Rings, Clouds, Warps, then Marbles and Grids.

Rules, each agreed by the maintainer:

1. Port only from the original repository. Never port from VCV Rack's Audible Instruments, which is GPLv3 and would contaminate transit's MIT/Apache-2.0 licensing.
2. No Mutable Instruments branding on the instruments. They get descriptive names of transit's own (as VCV did with Macro Oscillator, Resonator and Texture Synthesizer). Credit Émilie Gillet explicitly, with her MIT notice, in the about box, in each instrument's help entry and in a third-party license file.
3. Polyphony, the editor window and the rack are transit's own work. Polyphony is a voice allocator around N mono engines with a note-to-macro mapping (pitch, harmonics, timbre, morph). The editor follows the ownership rules of [ADR 0030](0030-editor-window-cohesion.md).

### Implementation

A Rust port in a transit-owned crate with a descriptive, brand-free name (the Plaits port is `transit-macro-osc`; see the amendment below). The shipped app is pure Rust.

Before any port starts, evaluate the existing crates.io port (`mi-plaits-dsp`) for license and fidelity. The evaluation is a prerequisite, not a decision: it informs whether to adopt, fork or write the port, and this ADR does not pre-empt it.

### Gate

Null tests against her C++, wrapped as a dev-only oracle. The oracle is built with the `cc` crate for tests and CI only and is never linked into the app.

- Same model, seeded noise, fixed parameter sweeps, compared sample by sample at 48 kHz with her block size.
- The residual floor is measured first, with FP contraction and fast-math off. The gate is set at that floor with margin. A residual above the floor is an escape.
- The port is tested at the mono-voice level. The allocator, the editor and the rack get ordinary tests.

## Consequences

- A coherent instrument section with one design language and low CPU.
- The DSP risk is retired by the oracle; the design work moves to composition and UI.
- Every ported module adds its own oracle build to CI.
- Third-party license text and credits become a maintained deliverable of each release that ships a port.

## Alternatives considered

- Generic wavetable and granular engines from scratch: months of sound design and no reference to test against.
- The C++ via FFI inside the app: adds a toolchain and an unsafe surface. Rejected; the C++ stays in the test oracle.
- The VCV Rack ports: GPLv3, which conflicts with transit's licensing.
- Shipping under the original names: contrary to the branding request.

## Amendment 2026-10-29: crate naming and the Plaits source decision

Maintainer decision, 2026-10-29. This amendment supersedes the crate naming in Implementation above and records the outcome of the crates.io evaluation (#193).

**(a) Crate naming.** The rule `transit-mi-*` is replaced. The eurorack README asks derivative works not to keep the original module names, so crate names are descriptive and brand-free: each names what the crate does, not whose module it came from. The Plaits port is the crate `transit-macro-osc` (macro oscillator). Later modules take names that describe what they do, for example Rings is `transit-modal-resonator`, not `transit-rings`.

**(b) Source decision (#193, 2026-10-29): option (b), vendor the MIT Rust port.** The port is `mi-plaits-dsp-rs` by Oliver Rockstedt (GitHub `sourcebox`), vendored at commit `effe9864bb` into `transit-macro-osc`. Its terms:

- Rockstedt's MIT notice stays in the source and the crate.
- Émilie Gillet's full MIT notice from `eurorack` and `stmlib` is restored in the crate `LICENSE` and in the third-party license file.
- The source commit (`effe9864bb`) is recorded in the crate and in the third-party file.
- Before any engine ships, each one is corrected against the C++ oracle (#194): per-instance RNG; sample rate as a parameter, not a constant; an audit of the interpolate clamps; the VA auxiliary oscillator phase returned to firmware behavior; and the snare and VOSIM details.

**(c) Fallback.** If the vendored code needs more repair than it saves, port from the firmware instead (option c), behind the same oracle gate. The null tests compare output sample by sample, so the swap is invisible to users.

Related: #193 (the evaluation and this decision), #194 (the C++ oracle that gates each engine).

## Implementation note: oracle

The Gate oracle is `crates/macro-osc-oracle` (`transit-macro-osc-oracle`, #194): the upstream C++ vendored unchanged, dev-only (`cargo xtask layers` rejects any normal or build dependency on it), with its FFI `unsafe` confined to `src/ffi.rs`; the sweep, the residual and the measured floor are in [gates.md, Oracle null tests](../contributing/evidence/gates.md#oracle-null-tests).

## Oracle deviation note 2026-10-30: the six-op engine after shared RAM

The firmware's engines share one 16 KB RAM arena (`Voice::Init` frees the allocator before each engine's `Init`), and the six-op engine keeps its staggered-render accumulator there without clearing it in `Reset`. On the module, the first block after the voice returns to a six-op bank from the string machine, speech, swarm, particle noise or inharmonic string engine (or the wavetable engine, on a 64-bit host only) therefore adds that engine's leftover bytes, read as floats, to the output: arbitrary values up to a NaN, which the C++ cast turns into a full-scale -32768 click. The port's engines own their memory, so the six-op engine replays its own last partial sum, which is the module's output whenever nothing overwrote it; the port is never louder than the module there. Won't fix: on the module, the first 12-frame block (0.25 ms) after returning to a six-op bank from one of six engines adds that engine's leftover memory to the output, which can be a click; transit plays the six-op engine's own tail instead, as the module does after any other engine. Reproducing it would mean emulating every engine's byte layout and the host's pointer size to replay garbage. The voice gate pins the difference to that one block (`six_op_reentry_after_a_ram_sharing_engine` in `crates/macro-osc/tests/voice_scenarios_vs_oracle.rs`); every other frame of every voice scenario is compared at 0 LSB. This is an oracle deviation, not a transit known issue: no user can hit it, since nobody compares the DAW with the module (#195).

## Implementation note 2026-10-30: the Macro Oscillator instrument (tuning, names, MIDI)

The first instrument of the family ships as the built-in "Macro Oscillator" (`macro_oscillator`, in `transit-dsp` beside the other built-ins, over `transit-macro-osc`; `transit-dsp` takes the same-layer dependency and is no longer standalone, #195).

- **Tuning.** The firmware computes pitch against its corrected rate (47872.34 Hz, the module's codec clock), so the module plays in tune while the same arithmetic at a true 48 kHz plays 4.6 cents sharp. The instrument plays concert pitch by default (A4 = 440 Hz): `Voice::set_tuning(Tuning::Concert)` scales every frequency the firmware derives from its corrected rate (the voice's `a0`, the six-operator voices' reference, the speech formants) by 47872.34 / 48000, and divides the LPC speech pitch shift's corrected rate by the same factor, so A4 is 440 Hz on every path. The firmware's LPC path is pitched differently by design: in `lpc_speech_synth_controller.cc` the corrected rate cancels out of `pitch_shift` and the LPC clock is the raw sample clock / 6, so at a true 48 kHz the firmware's LPC speech is in tune while its other paths are 4.6 cents sharp (on the module, the other paths are in tune and LPC speech is 4.6 cents flat). `crates/macro-osc/tests/voice_pitch.rs` checks every path's absolute pitch at every rate ([E63](../escapes/E63-the-speech-model-played-out-of-tune-away-from-48-khz.md)). The port plays at concert pitch; the firmware's arithmetic is kept bit-exact in the crate (`Tuning::Firmware`, the crate default) for the null tests, and the module itself plays in tune because its audio clock runs at the corrected rate. So there is no switch back to the firmware's pitch: at a true 48 kHz it would only detune the instrument by 4.6 cents while claiming to match the hardware (a "Hardware Tuning" parameter existed briefly on the slice 3 branch and was removed before merge; a session that stored it loads, and the open logs the ignored id).
- **Names.** The 24 models are named by what they do, in the firmware's order: Filtered analog, Phase distortion, Six-op FM 1 to 3, Wave terrain, String machine, Chiptune, Virtual analog, Waveshaper, FM, Grain, Additive, Wavetable, Chord, Speech, Swarm, Filtered noise, Particle noise, Inharmonic string, Modal, Bass drum, Snare, Hi-hat. No module or brand name appears in a UI string or identifier.
- **MIDI.** The held note is the voice's V/OCT input, a note-on raises its trigger and sets its level input from the velocity, a note-off lowers both so the low-pass gate's decay is the release. Mono, last-note priority, legato as the module's trigger input behaves: a note under a held one changes pitch with no new trigger. A new phrase whose note-on shares a 12-frame block with the last note-off gets one low block first, so the voice sees the edge. A legato note sets the level input to its own velocity, so the level steps mid-phrase as the module's level CV does (Ableton and Logic keep the running envelope on legato; documented in the user guide). Sustain holds released notes; pitch bend spans a parameter range (2 semitones by default); the mod wheel adds to a chosen control (Timbre by default).
- **Blocks and output.** The voice renders 12-frame blocks (its envelopes are timed per block) whatever the host block, carrying rendered frames across host blocks, so any host block size renders the same bits; events take effect at the next 12-frame boundary. The int16 frames become floats as `(1 - s) / 32768`: polarity restored and the firmware's +1 truncation offset removed (silence is exactly 0), full scale one LSB under 0 dBFS (+1 reads 32767/32768, -1 reads -32766/32768; only the clip reads 32769/32768, +0.0003 dB); the default level is -6 dB.
- **Credits.** Émilie Gillet's MIT notices (eurorack, stmlib) and Oliver Rockstedt's are in ATTRIBUTION.md, NOTICE and Help > About transit; the panel and its help entries credit the original; the user guide says it is a port, not an emulation of the hardware's analog stages.
- **Rack-ready, not racked.** One mono voice, MIDI in, one audio output on every channel, parameters addressed by automation id (`plugin:instrument:<id>`). Unison and layering are ADR 0034's racks; polyphony inside the one instrument followed in #196 (next note).

## Implementation note 2026-10-30: voices, note-to-macro mapping, editor (#196)

- **Voices.** `voices` (1 to 16, default 8) firmware voices inside the one instrument, all built with the instance (the parameter can change on the audio thread without allocating). Voice `i` is seeded `Random::DEFAULT_SEED + i`, and a reset reseeds every voice, so exports and renders after a reset play the same bits. A note-on takes, in order: the voice already playing that key (held, under the pedal, or in its tail), the lowest-numbered free voice, the quietest voice in its release tail (a peak follower falling 0.5% per 12-frame block; the oldest phrase among equals), else the oldest held voice; a stolen held voice gets one low trigger block first, so it sees a new edge. A note-off leaves the voice's low-pass gate tail playing; a voice is free once it holds no key, its trigger is low and both outputs have stayed within one int16 LSB of silence (-90 dBFS) for 50 ms (six-op FM 2 settles on a ±1 LSB residue, never exact zero), and a free voice is not rendered; every model goes idle within the tail (`every_model_releases_to_an_idle_voice_within_the_tail`). The chiptune model bypasses the low-pass gate and, with no envelope set, holds its note for ever (on the module too, with the TIMBRE attenuverter at 0): a held key keeps it held, and after the note-off the voice sets its envelope from Decay (`chiptune_release`, silent in 0.15 s to 9 s), so its note ends as on the other models. This fixes a slice 3 defect (a chiptune note never ended). Voices above a lowered count release and play out. `voices` = 1 is the mono instrument above, bit for bit in its legato rules. The voices sum in voice order, so the instrument plays exactly the sum of independent mono voices (`n_voices_render_the_sum_of_n_mono_renders_bit_for_bit`). No unison and no automatic gain for wide chords: voices add, as in Ableton's and Logic's synths. The steal order follows the convention of both DAWs' stock synths (steal the oldest note), with a preference for released voices first, quietest first; a stolen held voice is silent for one 12-frame trigger block (0.25 ms at 48 kHz) so its envelopes restart on a clean edge.
- **Note-to-macro mapping.** Velocity (0..1) and the key (-1..1 around middle C, MIDI 60, which Live and Logic both name C3 and Live's key tracking is relative to; ±1 at 60 keys away) drive Harmonics, Timbre, Morph and Decay through a depth each (-100% to +100%, default 0) and a curve per source (Linear, Exponential, S-curve: the clip fades' curves, `FadeShape::gain` called directly). As in the MIDI Learn range model the source's travel spans a part of the target's travel (no change to `depth` for velocity, `-depth` to `depth` for the key) and a negative span inverts. The sum is clamped to 0..1, then the mod wheel adds; with every depth at 0 each target is its knob exactly, so the defaults play as #195 (velocity sets the level, as before). Key tracking of Timbre ships as the "Key Tracked Timbre" factory preset (`key_timbre` +50%).
- **Editor.** Every built-in editor in transit is an egui window inside the main window's viewport (`builtin_editor.rs`), not a separate OS window, so ADR 0030's outcome (minimize, restore and front with the main window, no taskbar entry, never duplicated) holds by construction and the `editor_group` requests do not apply; `ui.inspect` lists it as `builtin` (`the_macro_oscillator_editor_stays_with_the_main_window_and_is_never_duplicated`). Rather than a first viewport for one built-in, the panel grows in place: a Voices section with activity lights (read from the instrument's scope tap, `Plugin::voice_activity`, stored only while the panel is open) and the mapping grid (a row per source, a column per target).
- **Cost.** One voice costs 5 to 70 us per 256-frame block at 48 kHz (0.1% to 1.3% of one core; the median of the eight models measured is 26.6 us, 0.5%) on the development machine, measured by `crates/dsp/tests/macro_osc_cost.rs` (ignored, release); the figures are in the crate README for the rack budget.

## Implementation note 2026-10-30: the resonator port (#198, slice 1)

- **Source: option (c), a port from the firmware.** No Rust port of this module exists to vendor (Oliver Rockstedt's repositories carry only the macro oscillator; crates.io and GitHub have none), so `transit-resonator` is transit's own port of `eurorack/rings/dsp` at `08460a69` and the stmlib helpers it uses, from her MIT C++ only (never from VCV's GPLv3 Audible Instruments). Its license chain is Émilie Gillet's MIT notices alone (`crates/resonator/LICENSE-THIRD-PARTY`, `VENDOR.md`). For the second module of this pair (slice 2), the one candidate for option (b) is `mutated-elements` (MIT, derived from the firmware, frozen upstream), pending a provenance and fidelity check.
- **Crate names.** The port is `transit-resonator`: it holds six models, not only the modal one, so the amendment's example name `transit-modal-resonator` would undersell it. The oracle stays one crate, `transit-macro-osc-oracle`, with a module per family (`resonator`, `resonator_sweep`, `cpp/resonator_oracle.cc`) sharing the build, the session lock, the residual and the floor; its name is brand-free, and a family-neutral rename is left for a quiet gap, since it would touch the in-flight instrument branches.
- **Gate.** All six models at polyphony 1 to 4 null bit-identical over the resonator sweep and the scenarios ([gates.md, Oracle null tests](../contributing/evidence/gates.md#oracle-null-tests)). The reference is the firmware's C++ as its desktop test harness builds it (flush-to-zero on, no fused multiply-adds), not the hardware's bits: the module runs with FZ off and its GCC build fuses multiply-adds. The oracle's flush-to-zero is reproduced by doing the port's arithmetic in an `f32` newtype that flushes every subnormal result, rather than at chosen spots; the sweep never reaches the subnormal range, the 20 s silences do.
- **Rate and pitch.** The firmware computes pitch against a true 48 kHz (`a3 = 440 / 48000`), so it needs no tuning switch: MIDI note 69 plays 440 Hz. The port runs at 48 kHz only, like the firmware; supporting other host rates (resampling, or a rate parameter with E63 pitch tests at every rate) is the instrument's decision in slice 3.
- **User-facing names (proposal).** The instrument over this port: "Resonator". Its models: Modal, Sympathetic Strings, Inharmonic String, FM Voice, Chord Strings (the quantized sympathetic strings), String and Reverb. The instrument over the second module of the pair: "Modal Voice". No module or brand name in a UI string or identifier.

## Decision note 2026-10-31: the Macro Oscillator is the default instrument, the Subtractive Synth is removed (#234)

With polyphony in (#196), a new instrument track carries the Macro Oscillator (default model, 8 voices) and the built-in Subtractive Synth is removed outright: its code, plug-in id (`subtractive_synth`), editor panel, factory presets, help and README line. transit is alpha software, so there is no compatibility shim for the synth itself; the one rule below keeps old sessions opening. Drum Synth and Sampler are unchanged.

Convention: Logic opens a new software-instrument track with its default patch loaded and Ableton leaves a MIDI track empty; we keep Logic's "ready to play" default, which is what the Subtractive Synth gave until now. Every path that creates an instrument track (`track.new`, the Add Instrument Track menu and shortcut, MIDI file import, the demo) uses `transit_model::DEFAULT_INSTRUMENT`.

**Substitution rule on load.** An instrument slot that names `subtractive_synth` becomes a Macro Oscillator with defaults when the session loads (`transit_model::legacy_synth::retire_subtractive_synth`, called by `Session::from_json`, so every open path, recovery and session import apply it):

- the old synth's parameters and preset name are dropped, since they mean nothing to the new instrument;
- the session's MIDI mappings, the track's automation lanes and the rack macro targets that addressed the old instrument's parameters (`plugin:instrument:<id>`) are dropped; mappings and lanes on inserts, and everything that addresses the slot as a slot (rack membership, zones, routing, volume, pan), keep working;
- the load report gets one line per track that lost something, then the status line "N track(s) used the retired Subtractive Synth and now carry the Macro Oscillator";
- the session opens as modified (the engine keeps the file's own content as the saved state), so the user sees the substitution on save; a saved session no longer names the old id and opens clean.

The corpus keeps the sessions released builds wrote with the old synth (`crates/model/tests/corpus`); `a_release_session_with_the_old_synth_opens_with_a_macro_oscillator_and_one_report` loads them, and the lossless round-trip tests read them with the id swapped. MIDI-mapping profiles are app-level, not session data, and are not rewritten: a profile mapping to an old instrument parameter finds no such parameter and does nothing.

The shared event queue and ADSR that lived in the synth's file stay, as `crates/dsp/src/plugins/instrument_queue.rs`, for the Drum Synth, the Sampler and the Macro Oscillator.

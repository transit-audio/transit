# ADR 0040: Inherited built-in effects retired in stages

- **Status:** accepted 2026-10-31 (maintainer decision, #264)
- **Date:** 2026-10-31
- **Reopen when:** a stage 2 replacement does not land and the device it replaces becomes a reason users cannot finish a project; or a retired device turns out to be one that Ableton and Logic users reach for daily with no equivalent in the kept set.

## Context

transit inherited its built-in devices from the SoundCraft fork. They were written to fill a registry, not to a transit design: a Drum Synth, a two-tap Pitch Shifter, a Channel Strip, several modulation effects, small utilities. The instruments and effects transit is porting or writing for itself (the Macro Oscillator, the Filter and Texture inserts, the Time Stretcher of #199, a Pro EQ in #233) replace them one by one.

Each month a device stays, sessions that use it accumulate, and removing it later breaks more of them. The maintainer decided on 2026-10-31 to break now, while transit is alpha: "Better to break now in alpha than to keep gaining a user base on old plugins that we scrap."

## Decision

**1. Stage 1 (v0.6.0, Breaking): twelve devices go.** Pitch Shifter (the two-tap one; the ported Time Stretcher covers pitch shifting), Drum Synth (the Macro Oscillator's percussion models cover it; kits from folders come later), Channel Strip, Maximizer, De-Esser, Rectifier, Lo-Fi, Time Shift, Mod Delay, Chorus, Flanger and Phaser. Plug-in ids: `pitch_shifter`, `drum_synth`, `channel_strip`, `maximizer`, `de_esser`, `rectifier`, `lofi`, `time_shift`, `mod_delay`, `chorus`, `flanger`, `phaser`. Their DSP, registry entries, panels, help, tests and docs mentions are removed. No factory preset existed for any of them.

**2. Stage 2 (separate issues, each when its replacement lands):**

| Stays for now | Goes when |
| --- | --- |
| Compressor/Limiter, Expander/Gate | a transit dynamics section replaces them (the mixer sidechain route and the racks depend on them) |
| EQ 7-Band, EQ 1-Band | the Pro EQ (#233) lands |
| Room Reverb, Plate Reverb | a transit or ported reverb lands |
| Saturator, Gain | a replacement lands (Gain is the pass-through the tests use until then) |

Until then these stay untouched, and stage 1's tests use them (Gain for pass-through, Compressor for dynamics and latency, the Filter insert for a modulated effect).

**3. The load rule** follows the Subtractive Synth retirement ([ADR 0033](0033-built-in-instruments-mutable-instruments-port.md), #234). A session that carries a retired device opens with that slot emptied (an insert slot, or the instrument slot for the Drum Synth). The device's parameters, and the MIDI mappings, automation lanes and rack macro targets that pointed at it, are dropped. The load report lists each track, slot and device by name (`track "Vox", insert B: the retired Chorus was removed; dropped 1 automation lane(s), 1 MIDI mapping(s), 1 rack macro target(s)`), and the status line reads `N insert(s) used retired built-in effects and were removed`, one line with the count. The session opens as modified, so the removal shows on save and a saved session carries no retired id. Nothing else on the track changes. A user preset file for a built-in device that names a retired one is skipped, with a line in the log. The rule applies on every open path (a file, a bundle, bytes).

**4. No other shim.** Nothing is substituted for the removed device and no id is kept alive: the same rule as the Subtractive Synth, because transit is alpha software and the session format may still change.

## Consequences

- A session saved by v0.5.x with any of the twelve opens with a quieter or different track until the user inserts a replacement; the release notes list the replacements.
- Each stage 2 removal repeats this ADR's load rule with its own list; the retired list and the load code live in one place (`transit_model::legacy_effects`).
- The sessions kept in `crates/model/tests/corpus` that carry a Chorus or a Maximizer (the v0.3.2 to v0.5.0 demos) are the standing proof of the load rule.
- The registry test lists exactly the kept built-ins, so a retired device cannot return unnoticed.

## Alternatives considered

- Keep the devices until their replacements land: more sessions depend on them each month, and a retired device's tests, panels and help keep costing maintenance.
- Substitute a kept device (a Gain, a Compressor) for each retired one: it would sound different without saying why, and leave an insert the user did not choose. Emptying the slot and reporting it is honest.
- Retire all twenty devices at once: the other eight (dynamics, EQ, reverb, Saturator and Gain) have no replacement yet, and the mixer's sidechain route and the racks depend on the dynamics.

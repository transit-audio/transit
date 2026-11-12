<!-- transit-index
title: Inherited built-in effects are retired in stages: stage 1 (v0.6.0, Breaking) removes Pitch Shifter, Drum Synth, Channel Strip, Maximizer, De-Esser, Rectifier, Lo-Fi, Time Shift, Mod Delay, Chorus, Flanger and Phaser; stage 2 (Compressor/Limiter, Expander/Gate, EQ 7-Band and 1-Band, Room and Plate Reverb, Saturator, Gain) goes when each replacement lands; a session that carries a retired device opens with the slot emptied, its mappings and rack macro targets dropped (amended 2026-11-12: from stage 2 on its automation lanes are kept as unassigned lanes), a load report by track, slot and name, and the status line "N insert(s) used retired built-in effects and were removed"; opens as modified; no other shim
-->
# ADR 0040: Inherited built-in effects retired in stages

- **Status:** accepted 2026-10-31 (maintainer decision, #264); amended 2026-11-12: from this date a stage 2 retirement keeps the retired device's automation lanes as unassigned lanes (decision 3)
- **Date:** 2026-10-31
- **Reopen when:** a stage 2 replacement does not land and the device it replaces becomes a reason users cannot finish a project; or a retired device turns out to be one that Ableton and Logic users reach for daily with no equivalent in the kept set.

## Context

transit inherited its built-in devices from the SoundCraft fork. They were written to fill a registry, not to a transit design: a Drum Synth, a two-tap Pitch Shifter, a Channel Strip, several modulation effects, small utilities. The instruments and effects transit is porting or writing for itself (the Macro Oscillator, the Filter and Texture inserts, the Time Stretcher of #199, the Parametric EQ of #233 (ADR 0054)) replace them one by one.

Each month a device stays, sessions that use it accumulate, and removing it later breaks more of them. The maintainer decided on 2026-10-31 to break now, while transit is alpha: "Better to break now in alpha than to keep gaining a user base on old plugins that we scrap."

## Decision

**1. Stage 1 (v0.6.0, Breaking): twelve devices go.** Pitch Shifter (the two-tap one; the ported Time Stretcher covers pitch shifting), Drum Synth (the Macro Oscillator's percussion models cover it; kits from folders come later), Channel Strip, Maximizer, De-Esser, Rectifier, Lo-Fi, Time Shift, Mod Delay, Chorus, Flanger and Phaser. Plug-in ids: `pitch_shifter`, `drum_synth`, `channel_strip`, `maximizer`, `de_esser`, `rectifier`, `lofi`, `time_shift`, `mod_delay`, `chorus`, `flanger`, `phaser`. Their DSP, registry entries, panels, help, tests and docs mentions are removed. No factory preset existed for any of them.

**2. Stage 2 (separate issues, each when its replacement lands):**

| Stays for now | Goes when |
| --- | --- |
| Compressor/Limiter, Expander/Gate | a transit dynamics section replaces them (the mixer sidechain route and the racks depend on them) |
| EQ 7-Band, EQ 1-Band | the Parametric EQ (#233, ADR 0054) lands |
| Room Reverb, Plate Reverb | a transit or ported reverb lands |
| Saturator, Gain | a replacement lands (Gain is the pass-through the tests use until then) |

Until then these stay untouched, and stage 1's tests use them (Gain for pass-through, Compressor for dynamics and latency, the Filter insert for a modulated effect).

**3. The load rule** follows the Subtractive Synth retirement ([ADR 0033](0033-built-in-instruments-mutable-instruments-port.md), #234). A session that carries a retired device opens with that slot emptied (an insert slot, or the instrument slot for the Drum Synth). The device's parameters, and the MIDI mappings, automation lanes and rack macro targets that pointed at it, are dropped (amended 2026-11-12: for a stage 2 retirement the automation lanes are not dropped but kept as unassigned lanes; see the amendment below; stage 1's retirements stay as shipped). The load report lists each track, slot and device by name (`track "Vox", insert B: the retired Chorus was removed; dropped 1 automation lane(s), 1 MIDI mapping(s), 1 rack macro target(s)`), and the status line reads `N insert(s) used retired built-in effects and were removed`, one line with the count. The session opens as modified, so the removal shows on save and a saved session carries no retired id. Nothing else on the track changes. A user preset file for a built-in device that names a retired one is skipped, with a line in the log. The rule applies on every open path (a file, a bundle, bytes).

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

## Amendment 2026-11-12: a retired device's automation lanes are kept

Amended 2026-11-12, accepted (maintainer decision). Decision 3 dropped the automation lanes of a retired device with the device. From this date, every stage 2 retirement keeps them: the lanes stay on the slot as lanes with no assigned parameter ("unassigned" lanes). Nothing plays from them, the curves survive, and they can later be assigned to a parameter of a replacement device. His reason: lanes "should stay because they can be reassigned to a replacement", and "some people spend ENORMOUS amounts of time on automation". The same holds when a plug-in in a slot is replaced or removed (the engine already skips a lane whose parameter the slot's plug-in does not have, issue #296).

- **Unchanged:** the slot is emptied; the load report is written by track, slot and device name, and the status line counts the removed inserts; the session opens as modified; nothing is substituted and nothing is converted: an old device's settings or lanes are never converted into the replacement (decision 4, the no-shim rule, stands). Keeping a lane is not a conversion: nothing is assigned until the user assigns it.
- **Changed:** in the load report, the kept lanes are named as unassigned instead of counted as dropped (the wording belongs to each removal's issue; the first is the EQ 7-Band and EQ 1-Band removal of ADR 0054 decision 16).
- **Dependency:** the visible unassigned state and the "Assign to..." gesture are tracked in issue #510 and ship no later than the first stage 2 retirement (the EQ retirement).
- **Risk to handle:** lanes bind by slot and parameter id, so a retired lane left in place would be picked up by a later device with a same-named parameter. The implementation marks a retired lane so that it never binds by id coincidence; see ADR 0054's amendment item 8.
- **History:** the twelve devices retired in stage 1 (v0.6.0) and the Subtractive Synth and one-sample Sampler retirements shipped under the old rule and stay as written. The new rule applies from this date.
- **Open, not decided:** whether MIDI mappings and rack macro targets that pointed at a retired device are kept the same way or dropped with a note (today's rule, which stands until decided: dropped); whether the rule also applies retroactively to devices retired in earlier releases (it cannot recover lanes already dropped from sessions saved since).

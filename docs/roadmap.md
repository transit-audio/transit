# Roadmap

The plan is the set of GitHub milestones at https://github.com/transit-audio/transit/milestones. Each one answers a single question, and a release is done when the answer is yes. The order can change; the themes do not mix.

| Milestone | Theme | The question |
|---|---|---|
| v0.3.1 | Small deferred fixes | Did the v0.3.0 reports get their follow-ups? |
| v0.4.0 | Engine: multicore and plug-in reliability | Does transit stay on deadline with the cores on and plug-ins loaded? |
| v0.5.0 | Editing: arrangement and MIDI workflow | Does editing feel like Ableton and Logic? |
| v0.6.0 | Instruments | Can you make a track from nothing but transit? Direction: [ADR 0033](adr/0033-built-in-instruments-mutable-instruments-port.md) and [ADR 0034](adr/0034-racks-layering-instruments-through-track-groups.md). |
| v0.7.0 | Performance and control surfaces | Can you play transit from hardware? Direction: [ADR 0037](adr/0037-adaptable-core-small-hardware.md) (headless core gate, Raspberry Pi 5 benchmark). |

Rules that shape the list:

- Engine correctness, stability and performance come before new features ([principles](principles.md)). v0.4.0 is the engine and nothing creative.
- Multicore rendering ships on by default only when it is at least as reliable on deadlines as the serial path across the agreed buffer sizes and under load (#124).
- Workflow behavior follows what Ableton and Logic agree on unless transit can do clearly better; the reason is recorded in the ADR.
- A defect found by a user is fixed in the next patch release when it is a regression or a crash; otherwise it joins the milestone whose question it belongs to.

Dependencies across milestones are noted on the issues (for example, kit parameter locks in v0.6.0 wait on sample-offset parameter delivery, #117, in v0.4.0). Tracking issues such as #31 carry no milestone.

## Beta

transit calls itself alpha until the six lines of the beta bar ([#228](https://github.com/transit-audio/transit/issues/228)) hold:

1. Session format commitment: every session saved from the beta release on loads in every later build, with a migration path, and the corpus tests gate it.
2. Core workflows complete end to end: record, edit, mix, automate and bounce, with the clip tools (#181 slices 2 and 3), range selection (#224) and a help catalog that covers every control (#178 slice 2).
3. Instruments and hosting on all three platforms: plug-in editors on Windows, macOS and Linux (#147 done for X11), and built-in instruments including the Macro Oscillator, racks (#197) and at least one more port (#198).
4. No correctness items in the known-issues list: K-entries may be limits or cosmetics, never data loss, and no open defect loses audio or edits.
5. Stability evidence per release: the eight-hour endurance run and the one-hour loaded run pass on every minor and are recorded under docs/benchmarks.
6. Outside confirmation: testers on each platform report through a full release cycle.

Features run through v0.7.0. After that the cadence slows and the effort moves to bug hunting and polish. Beta is the first release after that which meets every line above.

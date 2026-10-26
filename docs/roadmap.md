# Roadmap

The plan is the set of GitHub milestones at https://github.com/transit-audio/transit/milestones. Each one answers a single question, and a release is done when the answer is yes. The order can change; the themes do not mix.

| Milestone | Theme | The question |
|---|---|---|
| v0.3.1 | Small deferred fixes | Did the v0.3.0 reports get their follow-ups? |
| v0.4.0 | Engine: multicore and plug-in reliability | Does transit stay on deadline with the cores on and plug-ins loaded? |
| v0.5.0 | Editing: arrangement and MIDI workflow | Does editing feel like Ableton and Logic? |
| v0.6.0 | Instruments | Can you make a track from nothing but transit? |
| v0.7.0 | Performance and control surfaces | Can you play transit from hardware? |

Rules that shape the list:

- Engine correctness, stability and performance come before new features ([principles](principles.md)). v0.4.0 is the engine and nothing creative.
- Multicore rendering ships on by default only when it is at least as reliable on deadlines as the serial path across the agreed buffer sizes and under load (#124).
- Workflow behavior follows what Ableton and Logic agree on unless transit can do clearly better; the reason is recorded in the ADR.
- A defect found by a user is fixed in the next patch release when it is a regression or a crash; otherwise it joins the milestone whose question it belongs to.

Dependencies across milestones are noted on the issues (for example, kit parameter locks in v0.6.0 wait on sample-offset parameter delivery, #117, in v0.4.0). Tracking issues such as #31 carry no milestone.

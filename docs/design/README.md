# Design mockups

Clickable HTML mockups that a slice is built to, kept beside the ADR that decides the behavior. A mockup is a picture of the agreed panel, not a specification: the ADR's text wins where they differ, and real screenshots replace the mockup once the slice lands.

| Mockup | ADR | Slice | Notes |
|---|---|---|---|
| [Sampler panel](sampler-panel-mockup.html) | [0041](../adr/0041-sampler-one-instrument-three-views.md) | #269 | Pads (4x4 grid per bank, selected-pad strip), Slices (Auto detectors and a Manual bench with zoom, drag and zero-crossing snap), Instrument (key map with velocity splits and round robins). Reviewed by the maintainer 2026-11-03. |

## Device design language

The maintainer's design language for every device (instruments, effects, utilities) lives in [language/README.md](language/README.md) with machine-readable tokens in [language/tokens.json](language/tokens.json) and design-canvas mockups under language/mockups/ (they render on the design canvas, not standalone). It lands in v0.7.0 (issue #326), Macro Oscillator first, and new device panels follow it once its tokens exist in code.

# ADR 0045: The device design language is the UI's source of truth, mirrored in code

- **Status:** accepted 2026-11-06 (#326); settled after one judging round (below). v0.7.0 ships dark only (the maintainer's decision); the derived light set stays in code as the baseline for a later designed light theme.
- **Date:** 2026-11-06
- **Implementation:** the foundation job of #326: `crates/ui-studio/src/tokens.rs` and `tokens_tests.rs`, the token sets in `theme.rs`, the fonts in `apps/transit/src/fonts.rs`, the kit in `crates/ui-studio/src/widgets/`.
- **Amends:** [ADR 0010](0010-typography-and-iconography.md) decisions 1 to 4 (the fonts); its icon, attribution and window-title decisions stand.
- **Reopen when:** the light theme's design pass starts (see 6); a panel needs a component the language does not have (it is added to the language first, not drawn locally); or egui gains letter spacing or OpenType features that change how the type roles are applied.

## Context

The maintainer set a device design language for v0.7.0 on 2026-11-04 (#326): graphite panels, one signal color for what you set, one mod color for modulation, meters the only other color, every value printed in mono in real units, IBM Plex Sans and Plex Mono. It lives in [`docs/design/language`](../design/language/README.md) as a README and a machine-readable `tokens.json`.

The studio already paints every color from one token table (the UI plan's slice 5, `theme.rs`, with a dark and a light set behind View > Appearance), and every panel has deterministic layout tests. What was missing: the design's names in code, its type roles and metrics, its fonts, and the shared controls (knob, fader, toggle, switch, segmented control, display well, section bar, level meter, amount bar), so each panel would otherwise redraw them its own way. [ADR 0008](0008-fork-soundcraft-keep-the-engine-build-our-own-ux.md) set the direction: keep the engine, build our own UX. This ADR is how that UX stays one language as the panels move to it.

## Decision

1. **The design language is the source of truth.** `docs/design/language/README.md` and `tokens.json` define colors, type, space, shape and components. A new component, token or rule is added there in the same landing that settles it (the README's "Keeping this document current").
2. **Tokens in code mirror tokens.json, with a drift test.** `crate::tokens` holds every color (the dark set, which is the design; the arrangement and transport tokens the design does not name; the component literals; the light set), every type role, the space scale, layout metrics, radii and component metrics as named constants. `tokens_tests.rs` reads `tokens.json` and fails when a value differs or a name exists on one side only. The older token names (`bg`, `accent`, `text_dim`, `record`, ...) read the design values, so nothing that paints today needed to change to move to graphite.
3. **Panel tests assert tokens, not literal colors.** A layout struct records the tokens a control painted with; tests compare against `DARK`/`LIGHT` fields. The literal-color scan covers every file that paints, the tuner and the MIDI LED included.
4. **A visual migration carries no behavior change.** Moving a panel to the kit is its own commit; a change to what a control does (a gesture, a parameter, a default) is a separate commit with its own test. Existing tests that assert a behavior (a click, a drag, a key) do not change in a visual migration; tests that asserted an old literal color or metric are updated to assert the token they now read.
5. **Migration order.** The Macro Oscillator first (the reference device of the language), then the other built-in instruments and effects, then the mixer, then the main window (arrange, ruler, transport, browser, headers, piano roll). New panels (the Sampler views, the stock effects) are built from the kit from the start. The panels' current knob stays as `widgets::param_knob` until the last panel moves off it.
6. **The light theme is kept, derived.** The dark set is the design. The light set is derived by one rule: grounds mirror their contrast to the window ground in the opposite direction (inset wells white), inks keep their hue and at least their dark contrast. Tests enforce the rule. v0.7.0 ships dark only: View > Appearance shows System and Light unavailable, and the derived set waits in code for a designed light theme.
7. **Fonts are bundled under the OFL with attribution.** IBM Plex Sans (Regular, Medium, SemiBold) from the Plex Sans 1.1.0 release and IBM Plex Mono (Regular, Medium) from the Plex Mono 2.5.0 release, unmodified, with `OFL.txt` beside them and one `ATTRIBUTION.md` row per file (`cargo xtask assets` enforces it). Their digits are tabular by default, so no feature freezing is needed (ADR 0010's reason for modifying Inter is gone). Inter Tab and JetBrains Mono are removed; the system fonts and the Hangul fallback stay behind every family.
8. **The kit stores no state.** Each control takes its values and returns a response, the new value and a layout struct. Collapse (a per-user view setting) and enable (a patch parameter) are inputs; meter ballistics state is held by the caller and stepped with a pure function at the frame's `dt`.
9. **No analog dials.** A slow reading is a ballistics mode of the flat meter (peak fall-off by default; VU as an averaging preset with a reference), never a needle. The maintainer considered a VU needle and rejected it as out of place in the flat language.

## Alternatives considered

- **Keep the old names and add the design's beside them, unmapped.** Rejected: two palettes would coexist until every panel moved, and the chrome would stay off-language for months.
- **Generate the Rust constants from tokens.json in a build script.** Rejected for now: a build script adds a build step to every UI build for a file that changes rarely; the drift test catches the same mistakes with no machinery. Revisit if the file grows past what a reviewer reads.
- **Drop the light theme.** Rejected here: it is a shipped feature with tests, and dropping it is a product decision for the maintainer, not a side effect of a token change.
- **Keep Inter and JetBrains Mono.** Rejected: the maintainer chose Plex for the language; ADR 0010 already named Plex as the fallback.

## Consequences

- The whole app moves to graphite in one landing (chrome through `visuals()`, everything else through the remapped names), while the panels keep their current layouts until their migration.
- Some old names now alias the same value (`solo` and `accent` are both `signal`; `record` is `meter.hot`; a bypassed device's status reads `text.faint`). The mixer meter's hot segment, which used `solo`, reads `meter.hot` (since `1d9c2b1a`); its threshold (linear 0.9, about −0.9 dBFS) moves to above 0 dBFS when the mixer migrates to the kit's meter.
- The application chrome rules (menus, popups, dialogs, tool windows, buttons, fields, warnings and errors), the loop's own hue and the hatch come from the maintainer's second design document, the application chrome (draft 1), written into the design README; a warning carries no color, so `warn` left the token set.
- The derived light set's inks are darker than before (signal, mod and the meters keep their dark contrast against a near-white window); it does not ship in v0.7.0, and View > Appearance shows System and Light unavailable.
- The Macro Oscillator is the first device migrated to the kit (326-M): its editor draws the maintainer's board in a floating device window, and every other device keeps the older panel system until its own migration.
- As of the #326 chrome landing the light set is the maintainer's designed set (`colorLight`, pinned by the drift test), not the derivation of decision 6, and still does not ship; the content palette (`palette`) and the pad spec (`pad`) are documented in the design README and tokens.json ahead of their implementation.

## Judging round (2026-11-06)

A second pass against the bar of [ADR 0009](0009-refutation-review-before-every-push.md): look for a counterexample to each decision.

- *2, the drift test can pass while code and file disagree:* only if a constant is registered under the wrong name. The registries are built from the named constants in one macro, and the theme sets are tested against those constants (`the_old_token_names_read_the_design_values`). Sound.
- *3, a literal can sneak back in:* only in a file the scan does not list. The scan list is explicit; the kit's files paint only through `TOKENS` (each layout records its tokens and the tests compare them). Kept as is; a new painting file joins the scan when it lands.
- *6, the light rule can produce an unreadable pair:* the rule bounds contrast against the window only. Checked against panels and insets too: `text.faint` keeps 4.5:1 on panel and inset in both sets (tested), and every toggle letter keeps 4.5:1 on its fill (tested). Sound; the look is the open question, not legibility.
- *7, removing Inter breaks a path that names it:* a repository search after the switch finds no code naming the Inter or JetBrains families; `bold` stays as an alias. Sound.
- *8, a stateless knob cannot hold a text field open:* the open field is view state kept in egui's temporary memory under the knob's id, not a value the caller owns. Accepted and documented.

No decision changed. Settled.

## Related

- [ADR 0008](0008-fork-soundcraft-keep-the-engine-build-our-own-ux.md): build our own UX.
- [ADR 0010](0010-typography-and-iconography.md): typography and iconography (amended here for the fonts).
- [`docs/design/language`](../design/language/README.md): the design language and `tokens.json`.

# ADR 0023: Session compatibility markers: a `needs` list warns, `format_version` refuses

- **Status:** accepted 2026-10-24 (retroactive; decided 2026-10-24 in #70, known issue K40)
- **Date:** 2026-10-24
- **Reopen when:** a feature arrives that an older reader cannot ignore safely and a warning is not enough (it must refuse, which is a `format_version` bump); the warning is shown to users who save over a file without reading it and files are lost in practice; or the marker needs to carry more than a feature name (a minimum build, a per-feature version).

## Context

Sessions are `.transit` JSON that users' work lives in (`docs/contributing/knowledge/session-format.md`). The format rules already keep it forward-tolerant: new fields use `#[serde(default)]`, `deny_unknown_fields` is never used, and a file from a newer build loads on the previous one, ignoring what it does not know. `format_version` (`Session::FORMAT_VERSION`, 1) gates only files this build is too old for: `Session::from_json` refuses a higher number (`SessionError::Version`).

That leaves a gap for a feature an older build would drop without a word. [ADR 0019](0019-clip-internal-loops.md) made the case concrete: a clip's `looping` field is unknown to an older build, which loads the file, ignores the field, shows an unlooped clip of the saved length with audio running past the source end silent, and drops the loop when it saves. Nothing tells the user, and bumping `format_version` would refuse every file that has no looped clip in it as well. K40 recorded the gap; #70 asked for "a version marker that warns".

## Decision

1. **A top-level `needs` array lists features a reader must understand to keep the file whole.** `Session::to_json` (`crates/model/src/session.rs`) writes it only when `Session::needs()` is nonempty. A session that uses no such feature is byte-identical to before, so existing files and fixtures do not change.
2. **Entries are short names; today there is one, `clip_loop`,** listed when any clip has a loop (ADR 0019). `Session::KNOWN_NEEDS` holds the names this build understands. An entry is added to `needs()` and `KNOWN_NEEDS` together, when a new feature would be lost by an older reader.
3. **An unknown entry warns on load and never rejects.** `Session::from_json` compares each entry that is a string against `KNOWN_NEEDS` and, if any are unknown, adds one message to `Session::load_notes`: this session uses features this version does not know (the names), parts of it may be missing or sound different, and saving it from here can drop them. The engine's `open_session` shows the notes (`crates/engine/src/io.rs`). The names are cut to 40 characters and at most 8 are listed, because the file is untrusted. Non-string entries are ignored. The note is runtime state and is not saved back (tested).
4. **`format_version` stays reserved for breaking changes.** Only a change that makes an old reader unable to load the file safely bumps it, and the old reader then refuses with `SessionError::Version`. A feature that an old reader can load but would lose goes in `needs`.
5. **What older builds do.**
   - Builds that predate the marker ignore the key (rule 2 of the format page) and keep dropping the loop on save. The marker cannot warn from builds that already shipped; K40 stays open for them.
   - Builds with the marker and no knowledge of a given entry warn once and load the file as usual.
   - Builds that know the entry load it with no note.
6. **Two clip-loop rules shipped with the marker (#70), recorded here because they decide what a looped file means.**
   - A MIDI loop cycle steps from the tick grid origin, so a clip that starts between ticks keeps every cycle on the first cycle's offset.
   - A tempo change that would squeeze a MIDI loop under the minimum cycle (`MIN_CYCLE`, 64 samples, `crates/model/src/clip.rs`) is refused, naming the clip, instead of dropping the loop silently. A MIDI cycle is a fixed span of samples, so a tempo change inside a looped clip otherwise drifts later cycles off the bar grid (K40, open).
7. **Tests.** `a_session_with_a_loop_lists_it_under_needs_and_one_without_stays_as_before` (the marker appears, is absent otherwise, and a file with the marker removed still loads the loop), `a_session_that_needs_an_unknown_feature_warns_on_load_and_is_not_rejected`, and `rejects_garbage_and_future_versions` (`crates/model/src/session.rs`). A format change is reviewed like an engine change; the refutation attack list includes "old loads, new loads on previous build" (`docs/contributing/evidence/refutation-review.md`).

## Consequences

- A user who opens a newer file on a build that knows the marker is told before saving. A user on a build that predates it is not; K40 says so and advises not saving looped sessions from older builds.
- The marker is advisory: nothing blocks a save after the warning. Refusing is what `format_version` is for.
- The check is by name and exact match, so a name never changes meaning. A feature that changes shape later needs a new name.
- Each new feature that older readers would drop needs two edits (`needs()` and `KNOWN_NEEDS`) and a round-trip test with and without it.

## Alternatives rejected

- **Bump `format_version` for loops (#70 asked for a marker that warns instead).** Older builds would refuse every file written after the first looped clip existed, whether or not it contains one, and the "newer files load on the previous build" rule would be gone for the common case.

## Related

[ADR 0019](0019-clip-internal-loops.md) (the first entry), [ADR 0011](0011-record-arm-and-input-monitor-are-runtime-state.md) (runtime state stays out), `docs/contributing/knowledge/session-format.md`, `docs/known-issues/README.md` (K40), issue #70.

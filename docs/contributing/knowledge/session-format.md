# Session format

A session is `crates/model` data (`Session` in `crates/model/src/session.rs`), saved as `.transit`
JSON plus an `Audio Files/` folder. Users' work lives in these files. The engine can also read and
write a session as one zip bundle, `.transitz`, holding the `.transit` and its `Audio Files/`; only the retired web app used it (ADR 0032).

## Rules

1. **New fields use `#[serde(default)]`** (and `skip_serializing_if` where an absent value is the
   default), so an old session loads unchanged.
2. **Never `deny_unknown_fields`.** A file written by a newer build must load on the previous build,
   ignoring what it does not know.
3. **Old sessions must load.** The corpus (below) holds sessions saved by released builds and
   every build loads them.
4. **No silent rewrites.** Unknown or unresolved values (for example route names) are preserved on
   save (`routing.md`).
5. **Hostile input:** treat file contents as untrusted; no panics, bounded allocations
   (`crates/engine/tests/hostile_sessions.rs`).
6. **Runtime state stays out.** Record arm and input monitor are runtime state, not session data
   ([ADR 0011](../../adr/0011-record-arm-and-input-monitor-are-runtime-state.md)).
7. Per-machine settings (devices, paths) belong in `ui.json`, not in the session.

## The corpus and the field rule

`crates/model/tests/corpus/` holds two sessions per release, `v<version>-demo.transit` and
`v<version>-features.transit`, produced by the release's own code (its README says how). The tests
in `crates/model/tests/corpus.rs` load each one with no `load_notes`, check that a save then load
is lossless, and that no JSON key of the old file is missing after a save. **Every release adds
its two files** (`docs/releasing.md`, step 3); existing files are never regenerated. A change to a
serialized type in `crates/model` keeps the corpus tests green, or bumps `format_version` with a
migration.

**Field rule, enforced by `crates/model/tests/serde_defaults.rs`:** every field of a struct that
derives `Deserialize` is `Option<_>`, carries `#[serde(default ...)]` (or a `skip` attribute), or
sits in a struct with a container-level `#[serde(default)]`. The fields that are required by design
(written by every build since the first release) are the baseline list `REQUIRED_BY_DESIGN` in that
test; a new field joins the code with a default, not that list. The check is a line-based scan of
`crates/model/src`: it reads structs with named fields and skips enum variants and tuple structs,
so a new serialized enum variant with fields still needs the review question "does an old file
load".

## The `needs` marker

`format_version` only gates files this build is too old for (it refuses them). A feature that an
older build would silently drop instead is listed by `Session::to_json` under a top-level
`needs` array (`Session::needs`; today `["clip_loop"]` when any clip loops, ADR 0019). The key is
absent when nothing needs it, so those files are unchanged. Builds that predate the marker ignore
it (rule 2); builds that have it compare each entry with `Session::KNOWN_NEEDS` on load and post one
status message for unknown entries (`Session::load_notes`, shown by `open_session`), then load the
file as usual. It cannot warn from builds that shipped before it existed: an older build still
drops a loop on save (known issue K40). Add an entry to `needs()` and `KNOWN_NEEDS` together when
a new feature would be lost by an older reader, and test a round-trip with and without it.

## Retired features

A feature that is removed leaves old files that still carry it. Its serde variants are gone, so
`Session::from_json` strips it from the parsed JSON before deserializing (`crates/model/src/legacy.rs`)
and posts one `load_notes` line, so the file opens instead of failing. Today that is video (ADR 0031):
a `Video` track, `Video` clip content and the `videos` list are dropped. The corpus holds a
hand-written file for it (`v0.3.2-video.transit`, loaded by its own test, not the lossless round
trip), and `RETIRED_KEYS` in `tests/corpus.rs` names the key the older corpus files have and a save
no longer writes. Add a retired feature to the same pre-pass, with a fixture and a note.

A format change is reviewed like an engine change: the refutation attack list includes "old loads,
new loads on previous build" (`../evidence/refutation-review.md`).

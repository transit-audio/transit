# Session compatibility corpus

Sessions saved by released builds. `tests/corpus.rs` loads every `*.transit` here with the
current code, checks that nothing in it needs a note, that a save then load is lossless, and that
no JSON key the old file had is missing after a save. A change to a serialized type that breaks
one of these files is a break of users' sessions: keep the test green or bump `format_version`
with a migration (`docs/contributing/knowledge/session-format.md`).

## Files

Two per release, named `v<version>-<name>.transit`:

- `v<version>-demo.transit`: the demo session (`transit --demo`), saved.
- `v<version>-features.transit`: a small session that uses groups, an FX channel with a send, a
  looped MIDI clip, an audio clip with fades, an automation lane with two breakpoints, a MIDI
  mapping and an External Instrument track. When a release adds a serialized feature, extend the
  builder in `crates/engine/tests/corpus_generate.rs` to use it before generating that release's file.

v0.4.1 adds an optional `Session.unplaced_takes` (#192, recordings too long to decode). The v0.4.1 features session does not carry one: an entry is a load note, and the corpus test wants zero notes.

One file is hand-written, not saved by a build: `v0.3.2-video.transit` has an audio track, a Video
track, a video clip and a `videos` entry. It pins the sanitize rule of ADR 0031 (it opens with the video
dropped and one note) and is loaded by its own test, so the lossless round trip skips it (`SANITIZED`
in `tests/corpus.rs`).

Files hold the session JSON only, no `Audio Files/`. Nothing here reads audio.

## Producing a release's files

The files are what the release's code writes. Generate them from the release commit, without the
release binary:

```
cargo test -p transit-engine --test corpus_generate -- --ignored --nocapture
```

That builds the demo session with the app's own builder (`transit_engine::demo`), builds the
features session with the model's constructors, and saves each through `save_session`, the call
File > Save As makes, so the version, ids and field layout are the build's own. The version in the
name is the workspace version. Alternatively save the same sessions from the release build
through the control channel (`transit --demo --control <port>`, then `file.save_as`) and copy
the `.transit` files here.

## The rule

Every release adds its two files before the tag (`docs/releasing.md`). Existing files are never
regenerated or edited: they are the record of what that release wrote.

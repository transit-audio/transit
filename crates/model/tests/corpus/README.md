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

v0.5.0 adds optional fields (`Source::disk` for disk-backed takes, the Macro Oscillator's parameters and voice settings by id) that default on load. The v0.5.0 sessions do not carry a disk-backed source or a Macro Oscillator track: the generator builds neither without a file on disk or a hosted instrument, and the corpus test wants zero notes.

One file is hand-written, not saved by a build: `v0.3.2-video.transit` has an audio track, a Video
track, a video clip and a `videos` entry. It pins the sanitize rule of ADR 0031 (it opens with the video
dropped and one note) and is loaded by its own test, so the lossless round trip skips it (`SANITIZED`
in `tests/corpus.rs`).

One more is saved ahead of its release: `v0.6.0-rack.transit` (#197, ADR 0034; its two members carry the Macro Oscillator, the default instrument since #234) holds one rack
(a routing folder with two instrument members, key and velocity zones with fades, a MIDI clip on
the rack, a mapped and automated macro and a learned one). It is written by `write_rack_fixture` in
`corpus_generate.rs` from the racks branch, named for the release that ships racks, and goes
through the same lossless round trip as the release files; its own test checks the rack loads.
Another is saved ahead of its release: `v0.6.0-midi-effects.transit` (#201, ADR 0036) holds an
instrument track playing a MIDI clip through two MIDI effects, Transpose with an automated
parameter and a bypassed Velocity. It is written by `write_midi_effect_fixture` in
`crates/engine/tests/midi_effects.rs`, named for the release that ships the slot, and goes through
the same lossless round trip as the release files; its own test checks the chain loads.

Another is saved ahead of its release: `v0.6.0-takes.transit` (#25, ADR 0039) holds an audio
track and a MIDI track, each with a take stack of three numbered takes (`Clip::take`) on three
playlists; the audio track plays a shorter take inside a longer one. It is written by
`write_takes_fixture` in `corpus_generate.rs`, named for the release that ships take lanes, and
goes through the same lossless round trip; its own test checks the stacks and what plays.

The hand-assembled `v0.5.1-pre-take-lanes.transit` (#249) preserves the pre-#25 shape: three
unnumbered audio laps (the last partial), a later clip on the original playlist, and three MIDI
laps with different pitches. Each track has only its last lap active on playlist 3. The model
round trip preserves that shape; engine open migrates it in one undoable step. It is derived
from the take-lane fixture, with no embedded media or retired instruments.

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

## Sessions saved with the retired Subtractive Synth

Every released file above (v0.3.2 to v0.4.1) names `subtractive_synth` in an instrument slot, which v0.6.0 removed (#234, ADR 0033). They stay as they are, because they are what those builds wrote. Loading one substitutes a default Macro Oscillator and reports it: `a_release_session_with_the_old_synth_opens_with_a_macro_oscillator_and_one_report` checks that, and the lossless round-trip and key-drop tests read these files with the old id swapped for `macro_oscillator` in the text, so every other key still has to survive a save. A release's generated files from v0.6.0 on carry the Macro Oscillator.

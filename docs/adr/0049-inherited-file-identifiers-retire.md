# ADR 0049: The inherited file identifiers retire; v0.7.x reads both, v0.8.0 reads transit's only

- **Status:** proposed 2026-11-08 (the maintainer's direction of 2026-11-08)
- **Date:** 2026-11-08
- **Implementation:** after this record is settled, on branch `adr/0049-inherited-file-identifiers`: the clip group reader and writer (`crates/engine/src/clip_group_file.rs`), the track preset reader and writer (`crates/engine/src/cmd/track_more.rs`), the session MIME type at its five registration points in `packaging/`, old-tag fixtures in `crates/engine/tests/fixtures/`, and the tests in Evidence.
- **Reopen when:** another inherited identifier is found in a file transit writes, beyond the ones decision 6 lists; the session format commitment of the beta bar ([#228](https://github.com/transit-audio/transit/issues/228), line 1) changes how formats are versioned; or users report pre-0.7 files they cannot convert because they skipped 0.7.x (decision 3 says what they do).

## Context

transit was forked from SoundCraft, and two file formats it writes still carry the fork's name as
their format tag. The maintainer, 2026-11-08: "I want to push 0.7.0 hard so we need to make very
clear to people in docs and notices what to do. How can we import the old versions? This code we
might be able to ship in 0.7.0 and remove later and tell people to use 0.7.0 to migrate before using
later version." transit's rule while alpha is no migration code unless it is dated; this record
dates it.

What a grep of the repository at `transit` 6588370f found (every `soundcraft` hit in `crates/`,
`apps/`, `xtask/` and `packaging/`, string literals and byte strings separated from comments):

- **The clip group file** (`.scgrp`, written by `file.export_clip_groups` and read by
  `file.import_clip_groups`): JSON whose `format` is `"soundcraft.clipgroup"` (`clip_group_file.rs:16`, `FORMAT`), with `version: 1`. The reader
  refuses any other tag with "not a SoundCraft clip group file" (`clip_group_file.rs:116`). A file
  that is not JSON, or lacks a field, is refused earlier with "not a clip group file: <serde
  error>". The `version` field is written and not checked.
- **The track preset** (`track.save_preset` and `track.load_preset`; the caller names the file,
  the tests use `.json`): JSON whose `format` is `"soundcraft-track-preset"`
  (`cmd/track_more.rs:18`, `PRESET_FORMAT`), with `version: 1`. The reader refuses any other value,
  a missing `format` included, with "<path>: not a SoundCraft track preset" (`track_more.rs:277`).
  `version` is written and not checked.
- Neither format has a menu item in transit's UI today (`crates/ui-studio/src/menus.rs` builds the
  menus by hand and names neither): the four commands are reached through the control protocol,
  MCP, the assistant and the CLI. The users who have such files made them that way, or with the
  inherited app.
- **The session MIME type** `application/x-soundcraft-session`, in five registration points:
  `packaging/linux/transit.mime.xml` (the `mime-type` element), `packaging/linux/transit.desktop`
  (`MimeType=`), `packaging/linux/transit.metainfo.xml.in` (`<mediatype>`),
  `packaging/macos/Info.plist.in` (the UTI's `public.mime-type`) and `packaging/windows/transit.wxs`
  (the `.transit` extension's `ContentType`). The FreeBSD package and both Flatpak manifests install
  the Linux XML and desktop file and name no type of their own. The type is never written into a
  file: it lives in each system's file-type database, so it needs no reading phase.
- **The session file itself carries no inherited name in its structure.** A `.transit` file is JSON
  with `format_version: 1` (`Session::FORMAT_VERSION`, `crates/model/src/session.rs:392`) and an
  optional top-level `needs` list ([ADR 0023](0023-session-compatibility-markers.md)); it has no
  `format` string. `crates/model` names the fork in comments only (`lib.rs:1`, `track.rs:142`,
  `legacy_effects.rs:3`). **One inherited identifier is inside it, as opaque data:** a hosted VST3
  plug-in's saved state, stored base64 in the insert's `state`, starts with the 4-byte magic
  `SCV3` (`crates/vst3-host/src/preset.rs:26`, `STATE_MAGIC`; written by `join_state`, read by
  `split_state`, which takes a blob without the magic as a bare component state). It dates from
  the fork (2026-10-06). Decision 6 says what happens to it.
- **The clip group extension** `scgrp` (`clip_group_file.rs:15`, `EXTENSION`) is the fork's
  abbreviation too. The reader never looks at the extension; the writer adds it when the path
  given has none.
- **Not persisted, or another job's scope** (no file transit writes for the user carries them):
  the scanner reply prefix `SOUNDCRAFT-SCAN ` (`clap-host` and `vst3-host` `scan_cache.rs:25`, a
  line on the stdout of transit's own scanner child, read back by the same build); the plug-in
  host name reported to CLAP and VST3 plug-ins, the MCP server name, the logging targets, the
  environment variables, the inherited settings folder for the scan cache and the CLI's folder
  name (all renamed or removed by the remnants chore, branch `chore/soundcraft-remnants`, with
  K114 for the rescan); thread names and temp-folder names in tests; test-fixture plug-in ids
  `org.soundcraft.test.*` (the CLAP and VST3 fixtures, never shipped); and the attribution lines
  (the About box credit, "Based on SoundCraft by storytold." in the Linux package description and
  metainfo, `license.rtf`), which stay on purpose.

**Convention.** Ableton Live and Logic Pro open sessions from much older versions indefinitely, and
neither asks the user to step through an intermediate version. transit deliberately differs while
alpha (workflow follows what Ableton and Logic agree on unless transit has a reason, and then the
record says so): the files involved are two small side formats, not sessions; carrying a reader for
the fork's tags into the beta format commitment would freeze them there; and one release of
reading both gives every user a converter they already have. The beta bar (#228, line 1) restores
the commitment for transit's own formats.

## Decision

1. **From v0.7.0 transit writes its own identifiers.**
   - The clip group writer writes `"format": "transit.clipgroup"`; the track preset writer writes
     `"format": "transit-track-preset"`. Both keep `"version": 1`, and nothing else in either
     file changes. The two old tags become named constants beside the new ones
     (`LEGACY_FORMAT = "soundcraft.clipgroup"`, `LEGACY_PRESET_FORMAT = "soundcraft-track-preset"`),
     each used in one place: the reader's accept check.
   - The session MIME type is `application/x-transit-session`, at all five registration points
     listed in Context (the mime XML's `type`, the desktop file's `MimeType=` entry, the
     metainfo's `<mediatype>`, the plist's `public.mime-type`, the MSI's `ContentType`). The
     comments that say "MIME type unchanged" in the plist and the `.wxs` are corrected. The macOS
     UTI (`dev.derekmwright.transit.session`), the `.transit` and `.transitz` extensions and the
     Windows ProgIds do not change.
2. **v0.7.x reads the old tags too, converts on the next write, and says so once per read.**
   - The clip group reader accepts `transit.clipgroup` and `soundcraft.clipgroup`; the track
     preset reader accepts `transit-track-preset` and `soundcraft-track-preset`. An old-tag file
     reads exactly as a new-tag file with the same content (same clips, same audio, same mixer and
     instrument), and the file on disk is not touched by the read.
   - Neither format is ever rewritten in place: a clip group or a preset is read into a session,
     and the session carries no tag. The file converts when the user writes it again
     (`file.export_clip_groups` of the imported clips, `track.save_preset` of the loaded track),
     and what is written is always the new tag.
   - A read of an old-tag file posts one line through `Engine::message` (the status line and the
     log), the pattern the clip group import already uses for dropped video
     ([ADR 0031](0031-transit-does-not-host-video.md)) and session loads use for retired devices
     ([ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md)). The lines are fixed, with
     the file's name (its final path component) in front:
     - clip group: `<name>: saved before transit 0.7.0; export these clips again to keep the file opening after 0.8.0`
     - track preset: `<name>: saved before transit 0.7.0; save this preset again to keep it opening after 0.8.0`

     One line per read of an old-tag file; a new-tag read posts nothing. Why a line and not only
     the release notes: the conversion is not automatic for these two formats (decision 2's
     second point), so the user has to act, and the status line is where transit already tells a
     user what a read changed.
3. **v0.8.0 reads transit's tags only and refuses the old ones with a fixed message.**
   - The message, exactly:
     `This file was saved by a version before transit 0.7.0. Open and save it in transit 0.7.0 first: https://github.com/transit-audio/transit/releases/tag/v0.7.0`
     It is one constant (`PRE_0_7_REFUSAL`) shared by both readers. The clip group reader returns
     it as `EngineError::BadParams("file.import_clip_groups", <message>)`; the preset reader as
     `EngineError::Io("<path>: <message>")`, keeping each reader's existing error shape.
   - **The refusal path is written in v0.7.0.** In v0.7.x it is reached by a file whose `format` is
     a string that is neither the new nor the old tag. A file that is not JSON, or has no string
     `format`, is not a transit file of any version and keeps the existing structural error with
     the fork's name taken out: "not a clip group file: <serde error>" (unchanged) and
     "<path>: not a transit track preset" (was "not a SoundCraft track preset"). The clip group's
     "not a SoundCraft clip group file" goes away, since every string tag now reaches either the
     accept path or the refusal.
   - **The removal is a dated step: the v0.8.0 tag.** Before that tag, the old tag is deleted from
     each reader's accept check (one line per format, plus the two constants and the two notices
     of decision 2), and an old-tag file then falls into the refusal unchanged. One test per format
     carries the date: it reads the old-tag fixture and expects success with the decision 2 line
     while the workspace version (`CARGO_PKG_VERSION`) is below 0.8.0, and the refusal message at
     0.8.0 or above. Bumping the version to 0.8.0 without the removal fails that test, so the step
     cannot be forgotten; the known-issues row K115 carries the date for readers.
   - A user who skipped 0.7.x installs 0.7.x (any 0.7 release; the link names 0.7.0), imports and
     exports or loads and saves, and returns to the newer build. Releases stay downloadable from
     GitHub, so the path does not expire.
4. **Docs and notices.**
   - The user guide gains a section "Upgrading to 0.7.0": what converts and how (the two file
     formats, the line the status line shows, the two write commands), that 0.8.0 and later refuse
     unconverted files with the fixed message, the one-line instruction, and that sessions and the
     file associations need nothing from the user.
   - The known-issues README gets row K115 (the next free number after K114, which the remnants
     chore adds; the row takes the next free number at landing if that changes): the two old tags
     read through 0.7.x and are refused from the v0.8.0 tag.
   - `docs/releases/v0.7.0.md` is created as the release-notes skeleton in the shape of the v0.6.x
     notes: the opening paragraph with the alpha sentence and the upgrade line, `## Highlights`
     first with this change as a Breaking bullet, then Fixes, Known issues and Checksums for the
     release housekeeping job to fill. The release workflow's fixed alpha block (on the remnants
     branch) is prepended to the body at release time, so the notes file does not repeat it.
5. **The MIME change takes effect at install, in one step.** The type is a registration, not file
   content, so v0.7.0 registers only the new type and no build reads the old one. On Linux the
   package replaces its XML under the same file name and `update-mime-database` runs from the
   post-install script, so `*.transit` maps to the new type; a per-user default in
   `mimeapps.list` keyed on the old type no longer applies, and the user picks transit for
   `.transit` files once (the desktop file declares the new type, so transit is offered). On
   Windows the MSI upgrade rewrites the `.transit` key's `Content Type`. On macOS the UTI, not the
   MIME type, drives the association.
6. **The other inherited identifiers found while writing this record** (each pending the
   maintainer's confirmation):
   - *The clip group extension* `scgrp` becomes `transitclips` for files written from v0.7.0
     (`EXTENSION`, the command's help text "writes a .scgrp clip group file", and the docs that
     name it). No reading rule is needed: the reader never checks the extension, so an old
     `.scgrp` file opens under decision 2 and is refused under decision 3 by its tag alone.
   - *The VST3 state magic* `SCV3` stays as it is, recorded as transit's in-session VST3 state
     container magic (the comment at `STATE_MAGIC` says so). Three reasons. No user sees it. A
     stored state is kept as it is when its plug-in is not installed (the session saves the blob it
     loaded), so a session saved in 0.7.x could still carry `SCV3`, and a v0.8.0 refusal would lock
     the user out of a whole session over a plug-in they no longer have. And `split_state` reads a
     blob without the magic as a bare component state, so dropping `SCV3` from the reader would
     hand plug-ins the wrong bytes without an error instead of refusing. A change to it belongs
     with a session `format_version` change at the beta format commitment, under ADR 0023.
   - *The scanner reply prefix* `SOUNDCRAFT-SCAN ` is renamed freely whenever convenient (a
     follow-up of the remnants chore); both ends are the same build, so no compatibility applies.

## Consequences

- Clip group files and track presets written by v0.7.0 do not open in v0.6.x or older: those
  builds refuse the new tag with their own "not a SoundCraft ..." errors. Going back a version is
  not supported while alpha; the release notes say files written by 0.7.0 stay in 0.7.0 and later.
- Users with clip group files or presets from before 0.7.0 have to act once, within 0.7.x. The
  status line, the user guide, the release notes and K115 each say how.
- The v0.8.0 release has one required code step (decision 3); the version-gated tests make a
  release branch that skips it fail.
- On Linux, a hand-set per-user default app for `.transit` files is lost once at the upgrade
  (decision 5).
- The two old tags appear in the repository after v0.8.0 only in the refusal tests' fixtures.

## Evidence

The implementation adds, each with a negative control that fails on the code before the change:

- `crates/engine/tests/fixtures/`: `clipgroup_pre_0_7.scgrp` (tag `soundcraft.clipgroup`, one lane
  of two clips and one WAV in its `Audio` folder) and `track_preset_pre_0_7.json` (tag
  `soundcraft-track-preset`, a mixer with an insert and an instrument).
- The clip group and the preset tests: the old-tag fixture reads, posts the decision 2 line once,
  places the same clips (or sets the same mixer and instrument) as the same content with the new
  tag, and leaves the fixture file byte-identical; exporting (saving) again writes the new tag and
  no old one; a new-tag file posts no line; a file with `"format": "something.else"` is refused
  with exactly `PRE_0_7_REFUSAL`; a file with no `format` keeps the structural error; the
  version-gated test of decision 3 for each format.
- The export writes `.transitclips` when the path has no extension, and an old `.scgrp` name with a
  new tag imports (decision 6, if confirmed).
- A packaging test (beside the existing check that the `.wxs` `System.AppUserModel.ID` equals
  `APP_USER_MODEL_ID`): each of the five registration points names `application/x-transit-session`
  and no file under `packaging/` names `x-soundcraft-session`.

## Alternatives considered

- **Read the old tags forever.** That is the Ableton and Logic convention, and costs one line per
  format. Rejected while alpha: a reader for the fork's tags would ride into the beta format
  commitment and could not be removed after it; the dated window gives the same outcome for every
  user who upgrades through 0.7.x.
- **Refuse the old tags in 0.7.0.** Rejected: no build would read both, so a user's existing clip
  groups and presets would be lost with no way to convert them.
- **Rewrite files in place on read.** Rejected: an import that modifies the file it reads is a
  surprise, can fail on read-only media, and leaves the user unsure which build wrote what. The
  user writes the file again, and the status line says to.
- **A separate converter command or tool.** Rejected: more code and more to explain than reading
  both tags in the commands users already have, for two formats with no other change.
- **Keep the inherited names.** Rejected: the fork's name would stay in user files permanently, and
  at beta it would be frozen there.
- **Rename `SCV3` with the others.** Rejected in decision 6: no user benefit, a stale-blob path that
  would lock out sessions, and a silent misread if the reader dropped it.

## Related

- [ADR 0023](0023-session-compatibility-markers.md): how the session itself is versioned
  (`format_version`, `needs`); this record leaves the session format alone.
- [ADR 0031](0031-transit-does-not-host-video.md) and
  [ADR 0040](0040-inherited-built-in-effects-retired-in-stages.md): the status-line notice pattern
  decision 2 follows, and the "break now in alpha" precedent.
- [ADR 0014](0014-transit-gets-its-own-repository.md): the fork's replacement by this repository.

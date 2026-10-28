# ADR 0031: transit does not host video

- **Status:** accepted 2026-10-28 (decided by Derek, issue #176)
- **Date:** 2026-10-28
- **Reopen when:** a product question puts picture in scope (film scoring is a milestone, not a leftover), in which case it returns as a new feature with its own ADR rather than by reverting this removal.

## Context

transit inherited video support: `crates/video` (about 16,500 lines, a pure-Rust H.264 and ProRes decoder with an MP4 demuxer), an import command (`file.import_video`), a Video track and clip in the model, and a Video window and track lane in the legacy egui UI. The studio UI never draws a frame: it only excluded Video tracks in three places. Nothing on the roadmap uses it, no released build let a user create a video clip through the studio UI, and the crate carried code (about 1,600 lines, #16) that backs only its own tests. It is dead weight in build time, review surface and the layer table.

## Decision

transit does not host video. One batch removes:

- `crates/video` and its workspace entry, `file.import_video` and `import_video_bytes`, the commands and menu rows that only served it (`video.*`, `setup.video_sync_offset`, `view.dropped_video_frame_indicator`), the legacy UI's Video window and track lane, and the xtask layer entries.
- In the model: `ClipContent::Video`, `TrackKind::Video`, `VideoSource` and `Session::videos`.

## The sanitize rule

An old session must still open. `Session::from_json` runs a pre-pass on the parsed JSON (`crates/model/src/legacy.rs`, `drop_video`) before serde sees it, because the variants no longer exist and serde would refuse the file:

1. The `videos` list is removed.
2. A track whose kind is `Video` is deleted, with its clips and automation. The alternative, keeping it as an empty track, would leave a lane that cannot hold anything (a Video track has no audio path, and the other kinds cannot take its place without a guess), so the file would show a dead track the user can only delete. Deleting it is the least surprising result. Nothing routes through a Video track, so no other track loses an input.
3. A clip whose content is `Video`, on any track, is deleted.
4. When anything was dropped the session carries one note in `load_notes`, shown where missing plug-ins and missing media are reported: "video clips removed: transit does not host video". A file without video gets no note. The note is not saved back.

`format_version` stays 1: no released build let a user create a video clip through the studio UI, and a save from this build has no `videos` key.

## Consequences

- About 17,000 lines and one crate leave the workspace; the layer table, the standalone list and the wasm list shrink by one entry.
- Importing a movie file for its audio no longer works through `file.import_video`; extract the audio and use Import Audio.
- The corpus holds `v0.3.2-video.transit`, a hand-written file pinning the rule, and the key-drop test names `/videos` as retired, so the older corpus files stay as they were written.
- The `image` dependency stays: screenshots encode PNG in both UI crates.

## Related

Issues #176, #16. [ADR 0023](0023-session-compatibility-markers.md) (the notes and `needs` path this reuses), `docs/contributing/knowledge/session-format.md`.

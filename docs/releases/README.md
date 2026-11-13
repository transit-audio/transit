# Release notes

One file per tag, named `vX.Y.Z.md` (for example `v0.3.0.md`). Write and commit it before the tag is
pushed: `release.yml` uses it as the GitHub release body, and falls back to a generic body when the
file is missing. Put a `## Highlights` section first, as top-level bullets, because the Discord announce
step posts those bullets. See [releasing.md](../releasing.md).

## Warnings

Anything the reader must not miss starts with the warning triangle emoji (⚠️) before its bold lead
sentence, in the file here and on the GitHub release page:

- breaking changes and changed behavior (sessions, presets, mappings or sound that differ after upgrading);
- issues with the release (known breakage, an affected platform), including a note added after publishing;
- any callout that needs the reader's attention (back up first, an action to take).

Ordinary highlights, fixes and features get no emoji, and no other emoji is used. A note added to a
published release goes on the GitHub release page and into the file here in the same change.

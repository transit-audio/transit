# Release notes

One file per tag, named `vX.Y.Z.md` (for example `v0.3.0.md`). Write and commit it before the tag is
pushed: `release.yml` uses it as the GitHub release body, and falls back to a generic body when the
file is missing. Put a `## Highlights` section first, as top-level bullets, because the Discord announce
step posts those bullets. See [releasing.md](../releasing.md).

## Warnings

The warning triangle emoji (⚠️) is for what can cost the reader work or needs them to act: a session or
preset that an older or newer build cannot open, data that is dropped, a step to take before or after
upgrading, known breakage on a platform. When everything carries one, the ones that matter are lost
(v0.7.3 shipped with 21).

Layout, in this order, above `## Highlights`:

- `## Before you upgrade`: only the items above, one bullet each, each starting with the triangle before
  its bold lead sentence. Leave the section out when there are none.
- `## Changed behavior`: sessions, exports, mappings or sound that differ after upgrading, as plain
  bullets with no triangle, each saying where the details are.

In `## Fixes` and `## Known issues` an entry gets a triangle only if it meets the rule in the first
paragraph. A fix is not a warning: describe what was wrong and what happens now in one or two
sentences, and leave the mechanism to the issue it links.

No other emoji is used. A note added to a published release goes on the GitHub release page and into
the file here in the same change.

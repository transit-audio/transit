# Release notes

One file per tag, named `vX.Y.Z.md` (for example `v0.3.0.md`). Write and commit it before the tag is
pushed: `release.yml` uses it as the GitHub release body, and falls back to a generic body when the
file is missing. Put a `## Highlights` section first, as top-level bullets, because the Discord announce
step posts those bullets. See [releasing.md](../releasing.md).

# Releasing transit

Releases are tag-driven. `.github/workflows/release.yml` does the building.

1. Bump `[workspace.package] version` in the root `Cargo.toml` (for example `0.3.0`; the app and
   every crate inherit it) and refresh `Cargo.lock` with `cargo metadata`. Update the README's two current-release sentences (the intro note and the Downloads paragraph: the tag link and the asset names) in the same commit. Write the release notes in
   `docs/releases/vX.Y.Z.md` with a `## Highlights` section first (see `docs/releases/README.md`). Commit
   both, push, and wait for CI to be green.
2. Refresh the screenshots (`docs/images` and `site/assets`) at every release that changes the UI. Take them
   from the release build through the control channel, using the built-in demo session
   (`--demo --no-audio --control 0`) on a scratch config directory, never a user's real config. Five shots:
   arrange, mixer, piano roll, MIDI mappings, and Audio Devices with fake devices.

3. Add this release's corpus files, `crates/model/tests/corpus/v<version>-demo.transit` and `v<version>-features.transit`,
   from the release commit (before tagging, so the files are what the tag writes):
   `cargo test -p transit-engine --test corpus_generate -- --ignored --nocapture`. Extend the features session
   in `crates/engine/tests/corpus_generate.rs` first if the release serializes something new, then run
   `cargo test -p transit-model` and commit the two files (`crates/model/tests/corpus/README.md`). Never
   regenerate an earlier release's files.

4. Housekeeping, before the tag:
   - Re-read `AGENTS.md` and everything it points to (`CLAUDE.md`, `docs/contributing/agents.md`,
     `CONTRIBUTING.md`, the evidence docs) against the rules in force, and fix stale statements.
   - Refresh the "What each record decided" table in `docs/benchmarks/README.md` with any record or
     decision since the last release.
   - Refresh the escapes index (`docs/escapes/README.md`) and the escape count quoted in the README's
     "How we decide" section.
   - Confirm the known-issues table matches the Known issues in the release notes.

5. Tag that commit `vX.Y.Z` or `vX.Y.Z-pre` (for example `v0.2.0-alpha.1`) and push the tag:

   ```sh
   git tag v0.2.0-alpha.1
   git push origin v0.2.0-alpha.1
   ```

6. `release.yml` builds the archives and attaches them to the GitHub release for the tag (a pre-release only when the tag has a suffix):
   - Windows x86_64 (one download, a `.msi` installer and a portable `.zip`): `windows-x86_64`
     (native WASAPI and native ASIO; see below). Unsigned unless the
     Windows signing secrets below are set. SmartScreen may ask for "More info", then "Run anyway".
   - macOS arm64: `transit-v<version>-macos-arm64.dmg` (`transit.app` with a drag-to-Applications
     window, built by `packaging/macos/package.sh`) and a `macos-arm64` tarball with the bare binary
     and `audio-bench`. Ad-hoc signed and not notarized unless the signing secrets below are set.
     If macOS blocks the app, run `xattr -dr com.apple.quarantine /Applications/transit.app` or
     right-click it and choose Open.
   - Linux x86_64: tarball, AppImage, deb and rpm, built with the native JACK backend (`native-jack`;
     libjack is loaded at run time, so the binaries start without JACK installed).
7. The release body comes from the repository: when `docs/releases/<tag>.md` exists at the tag, the
   publish job uses it as the whole body. Without that file the body is the generic download text plus
   GitHub's generated notes. Check the release page when the archives look right. The body can still be
   replaced after publication with `gh release edit <tag> --notes-file <file>`.
8. `release.yml` then announces the release on Discord (the "Announce on Discord" step, which runs after
   the release is published). It posts as "transit releaser" and reads the published release body back, posting the top-level bullets of its `## Highlights` section (up to
   1800 characters; the first paragraph when there is no Highlights section) with a link to the
   release, and stops at the checksums section. Put the Highlights first when you write the notes.
   The step uses the `DISCORD_WEBHOOK_URL` repository secret; with the secret missing it logs and
   skips, and a Discord failure never fails the release (`continue-on-error`). To test the webhook
   without a release, run the workflow by hand with `discord_test` set; only the test post runs.

## Released

- `v0.4.1`, 2026-10-30, commit `4832235a`: point release (keyed plans on the workers by default, stuck plug-in Give up, title bars, status timeout, browser Locations, multi-clip copy, long takes refused loudly). Body from `docs/releases/v0.4.1.md`.
- `v0.4.0`, 2026-10-29, commit `ef9ad4e5`: engine release (parallel render pool on by default, #172, #117, #171, video removed). Body from `docs/releases/v0.4.0.md`.
- `v0.3.2`, 2026-10-27, commit `caca6f0e`: patch release (FX channel send, step idle polling, Linux diagnostics report, MIDI Mappings alignment, timing lint). Body from `docs/releases/v0.3.2.md`.
- `v0.3.1`, 2026-10-26, commit `c9c73d23`: patch release (interface zoom, step input, looped clips in the piano roll, Cycle from the selection, MIDI Mappings table, diagnostics report); screenshots refreshed.
- `v0.3.0`, 2026-10-26, commit `8d0925a7`: first release with the Linux packages (AppImage, deb, rpm) carrying the
  native JACK backend, and the Windows MSI with its installer dialogs. Body from `docs/releases/v0.3.0.md`.
- `v0.2.0-alpha.1`, 2026-10-22, commit `995cf66`. The cpal ASIO zip and MSI were removed from this
  release by hand; the workflow no longer builds them (commits `451bb52`, `f792a0e`, `f751a17`). Its DMG is
  named `transit-0.2.0-alpha.1-macos-aarch64.dmg`; later tags use the `transit-v<version>-macos-arm64.dmg` form.

## macOS signing and notarization (optional)

With all five repository secrets set, the macOS job imports the certificate into a temporary
keychain, signs `transit.app` and the DMG (hardened runtime, `packaging/macos/entitlements.plist`),
submits them with `notarytool --wait` and staples the tickets. With any of them missing the step logs
that it is skipping and the DMG is ad-hoc signed. The owner provides:

- `MACOS_CERTIFICATE_P12`: a Developer ID Application certificate exported with its private key
  as a `.p12`, base64-encoded (`base64 -i certificate.p12 | pbcopy`).
- `MACOS_CERTIFICATE_PASSWORD`: the password chosen when exporting the `.p12`.
- `APPLE_ID`: the Apple ID of the developer account.
- `APPLE_TEAM_ID`: the 10-character team ID.
- `APPLE_APP_PASSWORD`: an app-specific password for that Apple ID (appleid.apple.com).

When the build was notarized, say so in the release notes; they say the app is unsigned otherwise.

## Windows MSI and signing (optional)

Each Windows build also produces `transit-<tag>-<name>.msi`, built from `packaging/windows/transit.wxs`
with WiX v5 (`dotnet tool install --global wix --version 5.0.2` in the job; the source uses the WiX
v4 schema, which the WiX v3 on `windows-latest` cannot compile). It installs `transit.exe` and
`audio-bench.exe` per machine into Program Files (UAC prompt), adds a Start Menu shortcut and the
`.transit` and `.transitz` associations, and offers transit under "Open with"
for audio and MIDI files. The zip stays as the portable form. The upgrade code in the `.wxs` never changes.

An MSI version is numeric `major.minor.build`, so the tag `v0.2.0` becomes `0.2.0` and a pre-release
tag such as `v0.1.0-alpha.1` becomes `0.1.0` (the suffix is dropped; the workflow fails if the result
is not three numeric fields). The full version is kept in the installer's comments (Apps and
features) and in `HKLM\Software\transit\Version`. Same-version upgrades are allowed, so successive
pre-releases of one `X.Y.Z` replace each other. Successive Windows MSIs share one product,
so installing a newer one replaces the older.

With both repository secrets set, `packaging/windows/sign.ps1` signs `transit.exe`, `audio-bench.exe`
and the MSI with `signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256` (the files in
the zip are the signed ones). With either missing it logs that the files are unsigned and the build
continues. The owner provides:

- `WINDOWS_CERTIFICATE_PFX`: a code-signing certificate with its private key exported as a `.pfx`,
  base64-encoded (`[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`).
- `WINDOWS_CERTIFICATE_PASSWORD`: the password chosen when exporting the `.pfx`.

(`sign.ps1` also supports Azure Trusted Signing through `AZURE_*` secrets.) An EV or OV
code-signing certificate costs money (roughly a few hundred dollars a year; EV needs a hardware
token or a cloud signing service). Until the releases are signed, and until a signed publisher
builds reputation with SmartScreen, Windows warns on download and install: "More info", then "Run anyway".
Say in the release notes when the build was signed; the notes say unsigned otherwise.

## ASIO

The release has one Windows download (zip and MSI). It carries the native ASIO backend
(`transit/native-asio`), which is SDK-free and needs no Steinberg agreement. The earlier cpal ASIO
host, which compiled against the Steinberg SDK, has been removed.

Because the zip and MSI ship the native ASIO backend, `README-WINDOWS.txt` in the zip carries the
required notice: "ASIO is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries."

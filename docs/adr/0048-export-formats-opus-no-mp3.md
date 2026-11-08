# ADR 0048: Export formats: Opus, no MP3

- **Status:** accepted 2026-11-08 (#238; the maintainer's format decision of 2026-10-30)
- **Date:** 2026-11-08
- **Implementation:** #238, branch `io/238-opus-export`: `crates/opus` (`transit-opus`, libopus 1.6.1 vendored and built with `cc`), the Ogg Opus writer and reader in `transit-audio-io`, the Bounce dialog's Opus row and bitrate row, and `format: "opus"` with `bitrate` on the bounce commands.
- **Amends:** [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md): its section "Amendment 2026-11-08 (ADR 0048, accepted)" adds `transit-opus` to the unsafe allowlist with its FFI in `src/ffi.rs` only, and restates the allowlist in full (decision 4). On acceptance, ADR 0013's README row gains "amended by ADR 0048".
- **Reopen when:** a user needs surround Opus (channel mapping family 1, K112); a platform transit ships on cannot compile the vendored C with its own compiler; libopus's license changes; a lossy format other than Opus is asked for by more than one user with a reason Opus cannot meet (AAC for a delivery spec is the likely one); an MP3 encoder of LAME's quality appears under a license compatible with MIT/Apache (that removes the license reason and leaves the quality one for the maintainer to weigh); or the reference encoder's quality is overtaken by a maintained pure-Rust encoder that the tests below can hold to the same figures.

## Context

The maintainer decided on 2026-10-30 (#238) that transit's lossy export format is Opus and only
Opus. His reasons, from the issue: the MP3 encoder everyone uses, LAME, is LGPL, which means
dynamic-linking contortions in an MIT/Apache app; MP3 is the worse codec at every bitrate; Opus is
BSD-licensed, links cleanly, and plays in every browser, phone and messaging app. Lossless export
(WAV, AIFF, FLAC) is unchanged; import keeps decoding MP3 and Ogg Vorbis.

**Convention.** Ableton Live exports MP3; Logic Pro exports MP3 and AAC. Neither exports Opus.
transit deliberately differs here (workflow follows what Ableton and Logic agree on unless transit
can do clearly better, and then the ADR says so): Opus is the better codec at
every bitrate the dialog offers, it is the format browsers, Android, WhatsApp, Discord and
Telegram already use, and it costs no license entanglement. A user who needs MP3 for a delivery
spec converts the WAV or FLAC bounce with another tool.

## Decision

1. **Lossy export is Opus only.** `FileFormat::Opus` is the one lossy format `transit-audio-io`
   writes: Ogg Opus per RFC 7845, `.opus`. No MP3 or AAC encoder is added, in any crate, behind
   any feature. `encode_format_for("mp3")` stays `None` and the bounce commands refuse it.
2. **The encoder is the reference libopus, vendored.** libopus 1.6.1 from the release tarball
   (SHA-256 `6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1`, VENDOR.md),
   unchanged, in `crates/opus/vendor/`, compiled by `crates/opus/build.rs` with the `cc` crate
   from upstream's own source lists: the floating-point build, portable C (no SIMD or run-time CPU
   detection), no deep-learning features (`dnn/` is not vendored). No cmake, meson, autotools,
   pkg-config or system library is involved, so Windows MSVC, macOS arm64 and Linux build it with
   a C compiler alone. The Ogg container is transit's own Rust (a page writer and reader, about
   250 lines); libogg is not used. Options considered:
   - *A Rust binding crate that bundles libopus* (`opus` 0.4 over `opusic-sys` 0.7,
     `audiopus_sys` 0.2, `libopus_sys` 0.4, `opus-static-sys` 1.5.2). Rejected for three reasons.
     They bundle an older release (`audiopus_sys`: 1.3) or link a system libopus through
     pkg-config, which a package would then depend on. Their cmake builds choose the defines,
     and transit needs to set them itself: portable C with no SIMD or run-time CPU detection, so
     the output is the same on every runner; no `dnn/` and its 23 MB of model weights; thread-safe
     stack arrays (`VAR_ARRAYS` or `USE_ALLOCA`, never `NONTHREADSAFE_PSEUDOSTACK`). And cmake
     would become a prerequisite on every developer box and on the WSL gate, where `cc` needs
     only the C compiler.
   - *A pure-Rust port* (`opus-rs` 0.1, `mousiki` 0.2): no C at all, but young, independently
     written encoders whose quality and conformance transit would have to prove; `unsafe-libopus`
     (a c2rust transpile of 1.3.1) exposes the C API as `unsafe fn`s, so it removes no `unsafe`
     from transit and pins an old release.
   - *Vendored libopus with our own `cc` build* (chosen): the reference encoder, the current
     release, the defines in transit's hands, one file of FFI that a reviewer can audit in one
     pass.
   License: libopus is BSD 3-clause (with royalty-free patent grants from Xiph.Org, Microsoft and
   Broadcom, listed in its COPYING); nothing GPL, LGPL, AGPL or PolyForm is in the graph
   (`cargo tree -i transit-opus`: its only dependencies are `thiserror` and the build-time `cc`).
   The notice is in `crates/opus/LICENSE-THIRD-PARTY`; in NOTICE, which ships in the Windows zip
   and MSI, the Linux tarball, deb and rpm, the FreeBSD package and the Flatpak (each copies the
   docs to `share/doc/transit`); and in Help > About transit, compiled into every binary, which is
   the carrier for the AppImage (it moves the docs out) and the dmg (it copies none).
   ATTRIBUTION.md has its row.
3. **Writer parameters.** 48 kHz (other session rates go through a streaming windowed-sinc
   converter in `transit-audio-io`, the same kernel as `transit_dsp::offline::resample`; the
   crate sits below `transit-dsp`), 20 ms frames, application `AUDIO`, signal `MUSIC`,
   complexity 10, constrained VBR at a bitrate from `EncodeOptions::bitrate_kbps`.
   - *Bitrates.* Stereo: 96, 128, 160, 192 and 256 kb/s, 160 by default. Mono (a mono bus): 96,
     128 and 160 kb/s, 96 by default, since a mono stream needs about half the rate of a stereo
     one. The commands take 6 to 510 kb/s; without
     `bitrate` they use the default for the bounce's channel count.
   - *Constrained VBR.* It is libopus's own default VBR mode (`opus_defines.h`: "Constrained VBR
     (default)"). It bounds the rate over a short window, so "160 kb/s" means a file of about that
     size, and a test pins each preset's size within 10 percent of its rate (measured: 2 to 4 kb/s
     over). Unconstrained VBR does not: on the test material, a synthetic chord with white noise
     added (`music_with` in `crates/audio-io/tests/opus.rs`, the worst case for unconstrained
     VBR), it came out 77 to 93 kb/s over every preset (96 kb/s gave 173). A real mix overshoots
     less, but the size would no longer follow the choice.
   - *Length.* The OpusHead's pre-skip is the encoder's lookahead (312 samples), the stream is
     padded to the next frame and the last granule position trims it, so a bounce decodes to
     exactly `round(frames × 48000 / rate)` samples.
   - *Tags.* OpusTags carry the libopus version as the vendor string, `ENCODER=transit <version>`
     and `TITLE=` (the session's name; a clip's name for Export Clips).
   - *Channels.* Channel mapping family 0: mono and stereo. More channels are refused with a clear
     error (K112). The multistream and projection encoders and decoders are already compiled (they
     are in `OPUS_SOURCES`), so family 1 needs Rust only: the OpusHead mapping table, the
     SMPTE-to-Vorbis channel reorder and a multistream handle in `ffi.rs` under the same review.
     No new vendoring.
   - The bit depth and dither do not apply, and the dialog shows the bitrate row in place of the
     depth row.
4. **`unsafe` stays in one FFI module.** `transit-opus` joins the xtask unsafe allowlist. Its
   `unsafe` is confined to `src/ffi.rs` (the `extern "C"` block, two owned handles and their
   calls); `src/lib.rs` denies `unsafe_code` and exposes a safe `Encoder` and `Decoder`, and
   `cargo xtask layers` checks that the one `allow(unsafe_code)` in the crate is on `mod ffi;`.
   Every block carries a `// SAFETY:` comment quoting `opus.h` for the rule it relies on, and the
   crate gets the quoted-unsafe refutation review of ADR 0009 before merge and on every libopus
   update. The handles are `Send` (opus.h: a state "is always continuous in memory and only a
   shallow copy is sufficient to copy it") and never `Sync` ("A single codec state may only be
   accessed from a single thread at a time"). ADR 0013's amendment section records this, notes
   that this is the first vendored C linked into the shipped app, and restates the allowlist.
5. **Layers.** `transit-opus` is an L0 standalone crate; `transit-audio-io` takes it as its one
   workspace dependency (a same-layer edge) and leaves the standalone list. The headless core
   gate's graph gains `transit-opus` through `transit-engine` and needs only a C compiler; no
   Tier B crate depends on it.
6. **Import comes with the decoder.** libopus's decoder is in the same library, so `.opus` files
   import: the sniffer tells `OggS` + `OpusHead` from other Ogg, the reader decodes at 48 kHz,
   drops the pre-skip, trims to the last granule position and applies the header's output gain.
   Mono and stereo only, as for export.
7. **The rest of #238's list:**
   - *A 64 kb/s preset:* dropped. It is a speech rate, below what a music mix needs; it is added
     on request, and the commands already reach it (`bitrate: 64`).
   - *A VBR/CBR toggle:* dropped. Constrained VBR is libopus's default and already bounds the
     size; CBR serves streaming over fixed-rate links, not bounced files.
   - *An artist tag:* dropped. The session has no artist field; the title tag stays.
   - *A saved Opus bounce preference:* deferred to #367, the Bounce dialog remembering the last
     format, bit depth and bitrate per machine in `ui.json` (not in the session, which #238
     named). It does not block closing #238.

## Consequences

- Every package grows by the compiled libopus: the Windows x64 release `transit.exe` grows by
  516,608 bytes (45,246,464 to 45,763,072, about 0.5 MB or 1.1 percent; `cargo build --release -p
  transit` at the branch's base `8d0cd3c9` and at revision `8be964b5`, MSVC x64), which also counts the Rust
  Ogg, resampler and dialog code. The source tree grows by 3.1 MB of C under
  `crates/opus/vendor/`.
- The app build needs a C compiler on every platform: MSVC on Windows and the Xcode Command Line
  Tools on macOS (both already prerequisites), `gcc` or `cc` on Linux. A cross build for the
  ADR 0037 reference board (aarch64 Linux) needs an aarch64 C cross compiler too (`cc` honors
  `CC_aarch64_unknown_linux_gnu`). `docs/building.md` says so.
- A libopus update is a vendored-source change: new tarball and hash, the same file set, the
  tests' measured figures (pre-skip, spectrum floor, bitrate ratios) measured again, and the
  quoted-unsafe review.
- A libopus security release is picked up in the next transit release; release housekeeping checks xiph's release page (https://downloads.xiph.org/releases/opus/) for a newer release, and the update follows VENDOR.md's procedure. `.opus` import passes
  packets from user files to the C decoder (the Ogg parsing is Rust; the decoder is built with
  `ENABLE_HARDENING`).
- MP3 export requests have a documented answer (the user guide's Bounce section and the help
  entry say why in one sentence).
- Surround Opus is a known issue (K112) until family 1 lands; its scope is Rust only (decision 3).

## Evidence

- `crates/audio-io/tests/opus.rs`: lengths after pre-skip for 0.5, 1 and 10 s at 44.1 and 48 kHz,
  mono and stereo; the 1 kHz sine at 160 kb/s through a spectrum sweep (fundamental within 0.1 dB;
  the floor per distance band measured and pinned) with negative controls; each bitrate preset's
  size ratio within 10 percent; the defaults (160 stereo, 96 mono); the surround refusal; tags;
  sniffer and extensions; streamed and whole-buffer output byte-identical; corrupt files are
  errors.
- `crates/engine/tests/bounce_opus.rs`: the four bounce commands write `.opus` files of the right
  length with the bitrate asked for.
- `crates/ui-studio/src/slice6_tests.rs`: choosing Opus hides the depth row and shows the bitrate
  row, a mono bus offers the mono rates at 96, the extension follows, and the request carries the
  bitrate (each with its negative control).
- `xtask/src/tests.rs`: the confinement check passes on the repository and fails on a stray
  `allow(unsafe_code)`, a missing crate-root `deny` and an allow on another module.

## Related

- [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md): the unsafe discipline this crate follows, and
  the amendment that lists it.
- [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md): the vendoring and notice
  pattern (`VENDOR.md`, `LICENSE-THIRD-PARTY`, ATTRIBUTION.md, About) this crate follows.
- [ADR 0037](0037-adaptable-core-small-hardware.md): the headless gate's graph, which now holds a C
  build, and the reference board's cross build.

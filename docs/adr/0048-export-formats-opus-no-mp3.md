# ADR 0048: Export formats: Opus, no MP3

- **Status:** proposed (draft for the author and judging rounds; the maintainer's decision on the format is of 2026-10-30, #238)
- **Date:** 2026-11-08
- **Implementation:** #238, branch `io/238-opus-export`: `crates/opus` (`transit-opus`, libopus 1.6.1 vendored and built with `cc`), the Ogg Opus writer and reader in `transit-audio-io`, the Bounce dialog's Opus row and bitrate row, and `format: "opus"` with `bitrate` on the bounce commands.
- **Amends:** [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md) (adds `transit-opus` to the unsafe allowlist, FFI in `src/ffi.rs` only, decision 4). On acceptance, ADR 0013's file and README row gain "amended by ADR 0048".
- **Reopen when:** a user needs surround Opus (channel mapping family 1, K112); a platform transit ships on cannot compile the vendored C with its own compiler; libopus's license changes; a lossy format other than Opus is asked for by more than one user with a reason Opus cannot meet (AAC for a delivery spec is the likely one); or the reference encoder's quality is overtaken by a maintained pure-Rust encoder that the tests below can hold to the same figures.

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
   (SHA-256 `6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1`, VENDOR.md), unchanged, in `crates/opus/vendor/`, compiled by
   `crates/opus/build.rs` with the `cc` crate from upstream's own source lists: the
   floating-point build, portable C (no SIMD or run-time CPU detection), no deep-learning
   features (`dnn/` is not vendored). No cmake, meson, autotools, pkg-config or system library is
   involved, so Windows MSVC, macOS arm64 and Linux build it with the compiler the Rust toolchain
   already needs. The Ogg container is transit's own Rust (a page writer and reader, about 250
   lines); libogg is not used. Options considered:
   - *A Rust binding crate that bundles libopus* (`opus` 0.4 over `opusic-sys` 0.7,
     `audiopus_sys` 0.2, `libopus_sys` 0.4, `opus-static-sys` 1.5.2): every one builds the
     bundled source with cmake (or links a system libopus through pkg-config), which this
     repository's rule of no build-tool dependency on a runner refuses; `audiopus_sys` bundles
     the older 1.3.
   - *A pure-Rust port* (`opus-rs` 0.1, `mousiki` 0.2): no C at all, but young, independently
     written encoders whose quality and conformance transit would have to prove; `unsafe-libopus`
     (a c2rust transpile of 1.3.1) exposes the C API as `unsafe fn`s, so it removes no `unsafe`
     from transit and pins an old release.
   - *Vendored libopus with our own `cc` build* (chosen): the reference encoder, the current
     release, one file of FFI that a reviewer can audit in one pass.
   License: libopus is BSD 3-clause (with royalty-free patent grants from Xiph.Org, Microsoft and
   Broadcom, listed in its COPYING); nothing GPL, LGPL, AGPL or PolyForm is in the graph
   (`cargo tree -i transit-opus`: its only dependencies are `thiserror` and the build-time `cc`).
   The notice is in `crates/opus/LICENSE-THIRD-PARTY`, NOTICE (which the Windows zip and MSI and
   the Linux tarball, deb and rpm ship) and Help > About transit (compiled into every binary, so
   the AppImage and the dmg carry it too). ATTRIBUTION.md has its row.
3. **Writer parameters.** 48 kHz (other session rates go through a streaming windowed-sinc
   converter in `transit-audio-io`, the same kernel as `transit_dsp::offline::resample`; the
   crate sits below `transit-dsp`), 20 ms frames, application `AUDIO`, signal `MUSIC`,
   complexity 10, constrained VBR (libopus's default VBR mode) at a bitrate from
   `EncodeOptions::bitrate_kbps` (default 160; the dialog offers 96, 128, 160, 192 and 256; the
   commands take 6 to 510). Constrained rather than unconstrained VBR because a user picking
   "160 kb/s" expects a file of that size: measured on test material, unconstrained VBR came out
   77 to 93 kb/s over the target at every preset (96 kb/s gave 173), constrained VBR 2 to 4 kb/s
   over (within 2 percent). The OpusHead's
   pre-skip is the encoder's lookahead (312 samples), the stream is padded to the next frame and
   the last granule position trims it, so a bounce decodes to exactly `round(frames × 48000 /
   rate)` samples. OpusTags carry the libopus version as the vendor string, `ENCODER=transit
   <version>` and `TITLE=` (the session's name; a clip's name for Export Clips). Channel mapping
   family 0: mono and stereo. More channels are refused with a clear error (K112): surround Opus
   needs family 1 and the multistream encoder, a follow-up. The bit depth and dither do not
   apply and the dialog hides the depth row.
4. **`unsafe` stays in one FFI module.** `transit-opus` joins the xtask unsafe allowlist. Its
   `unsafe` is confined to `src/ffi.rs` (the `extern "C"` block, two owned handles and their
   calls); `src/lib.rs` denies `unsafe_code` and exposes a safe `Encoder` and `Decoder`. Every
   block carries a `// SAFETY:` comment quoting `opus.h` for the rule it relies on, and the
   crate gets the quoted-unsafe refutation review of ADR 0009 before merge and on every libopus
   update. The handles are `Send` (opus.h: a state "is always continuous in memory and only a
   shallow copy is sufficient to copy it") and never `Sync` ("A single codec state may only be
   accessed from a single thread at a time").
5. **Layers.** `transit-opus` is an L0 standalone crate; `transit-audio-io` takes it as its one
   workspace dependency (a same-layer edge) and leaves the standalone list. The headless core
   gate's graph gains `transit-opus` through `transit-engine` and needs only a C compiler; no
   Tier B crate depends on it.
6. **Import comes with the decoder.** libopus's decoder is in the same library, so `.opus` files
   import: the sniffer tells `OggS` + `OpusHead` from other Ogg, the reader decodes at 48 kHz,
   drops the pre-skip, trims to the last granule position and applies the header's output gain.
   Mono and stereo only, as for export.

## Consequences

- Every package grows by the compiled libopus (measured in the #238 result note); the source tree
  by 3.1 MB of C under `crates/opus/vendor/`.
- A libopus update is a vendored-source change: new tarball and hash, the same file set, the
  tests' measured figures (pre-skip, spectrum floor, bitrate ratios) measured again, and the
  quoted-unsafe review.
- MP3 export requests have a documented answer (the user guide's Bounce section and the help
  entry say why in one sentence).
- Surround Opus is a known issue (K112) until family 1 lands.

## Evidence

- `crates/audio-io/tests/opus.rs`: lengths after pre-skip for 0.5, 1 and 10 s at 44.1 and 48 kHz,
  mono and stereo; the 1 kHz sine at 160 kb/s through a spectrum sweep (fundamental within 0.1 dB;
  the floor per distance band measured and pinned) with negative controls; each bitrate preset's
  size ratio within 10 percent; the surround refusal; tags; sniffer and extensions; streamed and
  whole-buffer output byte-identical; corrupt files are errors.
- `crates/engine/tests/bounce_opus.rs`: the four bounce commands write `.opus` files of the right
  length with the bitrate asked for.
- `crates/ui-studio/src/slice6_tests.rs`: choosing Opus hides the depth row and shows the bitrate
  row, the extension follows, and the request carries the bitrate (each with its negative
  control).

## Related

- [ADR 0013](0013-unsafe-only-in-soundcraft-rt.md): the unsafe discipline this crate follows.
- [ADR 0033](0033-built-in-instruments-mutable-instruments-port.md): the vendoring and notice
  pattern (`VENDOR.md`, `LICENSE-THIRD-PARTY`, ATTRIBUTION.md, About) this crate follows.
- [ADR 0037](0037-adaptable-core-small-hardware.md): the headless gate's graph, which now holds a C
  build.

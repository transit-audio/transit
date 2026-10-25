# Building transit

## Prerequisites

- Rust 1.90 or newer (`rust-version` in `Cargo.toml`), from <https://rustup.rs>.
- Windows: the Visual Studio 2019+ Build Tools with the "Desktop development with C++" workload. The app runs on Windows 10 version 1607 or later (the plug-in editor window uses the per-monitor DPI functions that version added).
- Linux: `libasound2-dev` and `pkg-config` (for example `sudo apt install libasound2-dev pkg-config`).
- Linux, the native JACK backend (`--features native-jack`, below): also `libjack-jackd2-dev`, for the build only.
- Linux, headless test runs (CI, containers, WSL2): the egui tests render with wgpu and need a Vulkan adapter. Install `mesa-vulkan-drivers libvulkan1` (Mesa's CPU driver, lavapipe) and run with `WGPU_BACKEND=vulkan VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json` (`VK_DRIVER_FILES` is the newer name for the same setting). CI sets both. On WSL2 the default GL path can segfault, so set these there too.
- macOS: the Xcode Command Line Tools (`xcode-select --install`).

## Build and run

```sh
cargo build -p transit --release
cargo run -p transit --release
```

The binary is `target/release/transit` (`transit.exe` on Windows).

## Where settings live

transit keeps `ui.json`, `Autosave/` and `logs/` in its own settings folder:

- Windows: `%APPDATA%\transit`
- macOS: `~/Library/Application Support/transit`
- Linux: `~/.config/transit` (or `$XDG_CONFIG_HOME/transit`)

Set `TRANSIT_CONFIG_DIR` to use a different folder, for example to keep a test build away from your
real settings. The plug-in scan cache is shared with the inherited soundcraft apps (`SOUNDCRAFT_CONFIG_DIR` overrides).

The log is `logs/transit.log` in that folder. `--verbose`, `--log <directives>` and `RUST_LOG` raise its detail; see [Troubleshooting](troubleshooting.md).

`transit --report [path]` writes a diagnostics text file (version and build, OS, CPU, audio stack versions, the device lists, the saved audio settings and the last 200 log lines) to `transit-report-<YYYYMMDD-HHMM>.txt` in the current directory (the time is UTC) or to `path`, prints where, and exits. It needs no display, opens no audio stream and hides the user name and home folder. The report starts with a `Generated:` line (UTC). The Windows release has no console of its own: started from a terminal, `--report` and `--version` print there; started from a shortcut, the path (or the error) shows in a small message box. The file is written either way. Audio Devices and Callback Telemetry have a Copy diagnostics button with the same text, plus the open-device lines and counters of the running session.

## The update check

transit checks once a day, after the splash, whether a newer release exists (Help > Check for Updates… runs it on demand). The check is a single anonymous request to GitHub's public API (`api.github.com/repos/transit-audio/transit/releases`); nothing else is sent, and nothing is downloaded or installed. Turn it off with Help > Check for New Versions on Launch, or set `TRANSIT_NO_UPDATE_CHECK=1` (CI, tests, packagers; this also disables the Help entry).

The code is behind the Cargo feature `update-check` of the `transit` app, on by default (it brings `ureq` on rustls, no OpenSSL). Headless and distro builds can leave it out: `cargo build -p transit --no-default-features` has no HTTP dependency (and no native audio backends; see below).

## Packaging

`packaging/` holds the per-platform scripts, all building `-p transit` and reading the version from
`apps/transit/Cargo.toml`: `linux/package.sh` (AppImage, deb, rpm, tar.gz; `flatpak-bundle.sh` and
the `dev.derekmwright.transit*.yml` manifests for Flatpak), `macos/package.sh` (`transit.app` in a DMG
named `transit`; the icon is `assets/app-icon/transit.icns`), `windows/package.ps1` (MSI and portable zip),
and `freebsd/package.sh`. Outputs land in `dist/release/` as `transit-<version>-<platform>-<arch>.*`.
The tagged GitHub release (`.github/workflows/release.yml`) is separate: it ships plain archives, plus on Windows an MSI built from `packaging/windows/transit.wxs` with the same WiX v5 (`dotnet tool install --global wix --version 5.0.2`) and optional signing (`docs/releasing.md`).

### Windows installer

The MSI uses the stock WiX UI (`WixUI_InstallDir`): it shows welcome, license, install folder, progress and finish pages. `msiexec /i transit-<ver>-windows-x64.msi /qn` installs silently, with no dialogs. Building it needs the WiX UI extension (`wix extension add --global WixToolset.UI.wixext/5.0.2`); `package.ps1` and the release workflow add it. The license page text is `packaging/windows/license.rtf` (MIT and Apache-2.0).

### Linux packages

`packaging/linux/package.sh` builds the AppImage, deb, rpm and tar.gz (`--formats "appimage deb rpm"` picks some; `--skip-build` reuses `target/release/transit`). The deb and rpm need [nfpm](https://nfpm.goreleaser.com); the AppImage downloads `appimagetool` on first use, and `zsyncmake` (the `zsync` package) adds the `.zsync` for delta updates. Build on an old distribution (CI uses Ubuntu 22.04) so the binary runs on newer ones. The files are `transit-<version>-linux-<arch>.{AppImage,AppImage.zsync,deb,rpm,tar.gz}` in `dist/release/`. On a tag, the release workflow runs the script for the AppImage, deb and rpm and attaches them, with the `.zsync`, to the GitHub release next to the `transit-<tag>-linux-x86_64.tar.gz` that holds the binary and `audio-bench`.

## Native JACK backend (Linux)

`crates/audio-jack` (`transit-audio-jack`, [ADR 0021](adr/0021-native-linux-audio-over-the-jack-client-api.md))
talks to JACK 2, or to PipeWire through its JACK library, over the JACK client API. It is behind the
`native-jack` feature and compiled only on Linux (the crate is empty elsewhere):

```sh
sudo apt install libjack-jackd2-dev   # jack.pc for jack-sys's build script; nothing links against libjack
cargo build --release -p transit --features native-jack
cargo build --release -p audio-bench --features audio-bench/native-jack
target/release/audio-bench --backend jack --list-devices
```

- **libjack is loaded at run time** (`libjack.so.0`, through `jack-sys`'s `dynamic_loading`), so the
  binary starts on a system without JACK; the JACK system then says "JACK library not found". On
  Debian and Ubuntu, PipeWire's `pipewire-jack` keeps its `libjack` off the loader path: start transit
  with `pw-jack transit`, or install the distribution's `ld.so.conf.d` example file. Fedora and Arch
  replace `libjack` outright.
- **The server must be running.** transit never starts one (`JackNoStartServer`); with none it says
  "JACK/PipeWire server not running". The server sets the rate and the quantum; under PipeWire pin
  the quantum (`pw-metadata -n settings 0 clock.force-quantum 128`) or start transit with
  `PIPEWIRE_QUANTUM=128/48000`.
- **Runtime:** a running JACK 2 or PipeWire server (with PipeWire's JACK library, `pipewire-jack`).
  On Debian and Ubuntu with PipeWire, start transit as `pw-jack transit`. In Audio Devices choose the
  JACK system and Backend > Native JACK (or leave Auto: with a server answering, Auto opens Native
  JACK on the JACK system and on the untouched default; with none it opens cpal's ALSA host). The
  buffer size and rate are the server's; the panel shows them and where to change them. Set
  `TRANSIT_JACK_AUTOCONNECT=0` to leave transit's ports unconnected for a patchbay.
- Tests use a fake server and need no JACK: `cargo test -p transit-audio-jack` (any platform). On
  Linux `cargo test -p transit-audio-jack --features libjack` also runs the real-server tests, which
  skip when no server answers; start a dummy server first to run them:
  `jackd -r -d dummy -r 48000 -p 128 &` (`jackd2` package). The tests that kill or re-quantize a
  server are ignored and start their own (see the knowledge note).

Design notes: [contributing/knowledge/jack-backend.md](contributing/knowledge/jack-backend.md).

## Building with ASIO (Windows)

The ASIO backend is native: it needs nothing beyond the Windows prerequisites above. The app offers
it with the `native-asio` feature, which is on by default. (The earlier cpal ASIO path, which needed
the Steinberg SDK and libclang, was retired; see ADR 0016.)

### Native ASIO backend (no SDK)

`crates/audio-asio` (`transit-audio-asio`) has hand-written bindings, so it needs no Steinberg SDK,
no C++ compile, no bindgen, no libclang, no `CPAL_ASIO_DIR` and no environment variables. It is
compiled only on 64-bit Windows (`cfg(all(windows, target_pointer_width = "64"))`; the crate is empty
elsewhere) and builds with a plain `cargo build`. A plain `cargo build --release -p transit`
on Windows includes it and Native WASAPI: the `native-asio` and `native-wasapi` features are default
features (they compile to nothing on Linux and macOS). Choose Audio system ASIO, then Backend "Native
ASIO" in Audio Devices. To opt out, build with `--no-default-features` (add `--features update-check`
to keep the update check); then the build has no ASIO. The benchmark uses it through the `audio-bench/native-asio`
feature:

```powershell
cargo build --release -p audio-bench --features audio-bench/native-asio
target\release\audio-bench.exe --list-devices --system ASIO
target\release\audio-bench.exe --backend asio --device "<driver registry name>" --loopback --out-ch 1 --in-ch 1 --seconds 10
```

Tests, no hardware needed:

```powershell
cargo test -p transit-audio-asio
```

The tests that open real drivers are `#[ignore]`d. Run them on a machine with the interface
attached and its driver installed, one at a time:

```powershell
cargo test -p transit-audio-asio -- --ignored hardware_ --nocapture --test-threads 1
```

Or pick one by name: `hardware_lists_focusrite_and_tascam`, `hardware_open_close_each_driver_twice`,
`hardware_scarlett_duplex_streams_two_seconds`, `hardware_model12_duplex_streams_two_seconds` or
`hardware_control_panel_opens_and_returns`. Design notes:
[contributing/knowledge/asio-backend.md](contributing/knowledge/asio-backend.md).

ASIO is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries.

# 2026-11-07, Ryzen 9 5900X, Focusrite Scarlett 2i2 (ASIO): does loading and releasing the driver disturb the interface?

Issue #363. On 2026-11-07 the maintainer's interface went offline twice, and desktop audio stayed jittery until he replugged it. The first time, a scratch copy of the app was run with `--no-audio`. The second time, a UI test listed the machine's ASIO devices. Both loaded the ASIO driver while his own transit had it open with a stream running. This experiment asked whether loading the driver disturbs the interface by itself, and whether a driver left unreleased at process exit does.

Setup: Windows 11, AMD Ryzen 9 5900X, Focusrite Scarlett 2i2 over USB, driver "Focusrite USB ASIO". Desktop audio was playing through the interface and the maintainer's transit was closed. The tool is the three ignored `hardware_363_*` tests in `crates/audio-asio/src/lib.rs`, run one process per step from branch `fix/asio-listing-release` (02accbf1). The observations are the maintainer's, in his words.

| Step | What ran | UTC | Result | Maintainer |
|---|---|---|---|---|
| (a) | one load, query and release | finished 22:40:12 | exit 0; 2 in / 2 out; released in 16.8 ms | "played cleanly" |
| (b) | ten load/query/release cycles, one a second | 22:43:36 to 22:43:47 | exit 0; each cycle 5.2 to 11.3 ms | "clean" |
| (c) | a child process loads the driver and exits without releasing it | 22:46:24 to 22:46:26 | child exit 0 after 56 ms; parent exit 0; the 60 s kill did not fire | "no bug that I could tell" (no glitch, no dropout, no replug) |

## Conclusion

On this interface, a load and release while no stream runs does not disturb it (11 cycles). Neither does a load abandoned at process exit. The 2026-11-07 incidents therefore needed the part this experiment left out on purpose: the maintainer's own transit holding the driver open with a stream running while a second process loaded the same driver. In the first incident that second process was the `--no-audio` copy listing through a probe; in the second, the UI test listing the machine. That case was not reproduced, because doing so would knock the interface offline again.

The #363 fix removes every such load:
- Nothing is listed or probed without a player.
- The listing describes a driver only when this process already holds it (`probe_open`).
- A driver is instantiated only to open it, and every instance is released on the host thread.

No known-issues entry is needed.

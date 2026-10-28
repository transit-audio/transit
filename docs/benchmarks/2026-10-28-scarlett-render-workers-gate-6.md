# Gate 6: render workers against serial on the Scarlett (#124)

- **Date:** 2026-10-28
- **Machine:** AMD Ryzen 9 5900X (12 cores, 24 threads), 64 GB, Windows 11 Pro 26200, the same machine as the [2026-10-27 benchmark](2026-10-27-ryzen-5900x-render-workers-serial-vs-pool.md). Focusrite Scarlett over native ASIO at 48 kHz.
- **Build:** `eae56b78` (v0.3.2 plus #124 slices 1 to 4).
- **Gate:** #124 gate 6, measured through the device. [ADR 0029](../adr/0029-strip-job-slots.md) (amends ADR 0012 G1).

The 2026-10-27 figures were dev-machine numbers from a paced render loop with no device. This run is the first measurement of the workers through the device callback.

## Method

One saved session with eight plug-in instances: Jup-8000 V (two), Battery 4, Kontakt 7, Super 8, Massive X, Solid Bus Comp and Ozone 11 Vintage Compressor. The session loops, no plug-in editor is open, and each run lasts ten minutes. At the end of each run, Copy diagnostics writes the figures below.

The runs compare the serial path (the default) with the pool (`TRANSIT_RENDER_POOL=1`), at 64 and 128 frames, with the engine idle and under compile (a looped `cargo check --workspace` in a separate target directory). The runs were scripted through the app's control channel (open session, play, stop, Copy diagnostics, quit), so every run did the same thing.

The maintainer's own one-hour serial run at 64 frames, made while building the track, is a ninth report: 2,679,919 callbacks, 1,216 over budget, 8 of them while a plug-in loaded, 0 driver overloads, 0 stream errors, and a longest callback of 29.9 ms in Super 8 with its editor open.

## Results

| run | callbacks | over budget | load at end | longest callback | strip overrun blocks | busy | late | deadline overshoots |
|---|---|---|---|---|---|---|---|---|
| serial 64 idle | 478,648 | 1,290 | 78% | 33.9 ms Super 8 | n/a | | | |
| pool 64 idle | 480,036 | 2 | 78% | 5.1 ms Kontakt 7 | 830 | 203 | 627 | 5 |
| serial 64 under compile | 479,887 | 6,519 | 148% | 50.5 ms Super 8 | n/a | | | |
| pool 64 under compile | 479,720 | 45 | 88% | 41.2 ms Super 8 (inline) | 1,803 | 434 | 1,376 | 67 |
| serial 128 idle | 239,305 | 196 | 84% | 42.2 ms Super 8 | n/a | | | |
| pool 128 idle | 239,673 | 3 | 37% | 2.9 ms Jup-8000 V | 58 | 21 | 39 | 6 |
| serial 128 under compile | 239,684 | 885 | 81% | 47.5 ms Super 8 | n/a | | | |
| pool 128 under compile | 240,328 | 2 | 76% | 3.0 ms Jup-8000 V | 66 | 22 | 46 | 10 |

Every pool run: 8 workers promoted, 0 faulted strips, 0 suspect blocks, 0 deferred MIDI events, 0 protocol faults, 0 driver overloads, 0 stream errors. "n/a" for serial: there is no strip deadline on the serial path, so a slow strip only lengthens the callback.

The maintainer listened through the 64-frame runs and heard no artifacts in either mode.

## Findings

- **The callback is far safer with the pool at every row.** Over-budget callbacks fall by a factor of 65 to 645 (from 1,290 to 2 at 64 idle, from 6,519 to 45 at 64 under compile, from 196 to 3 at 128 idle, from 885 to 2 at 128 under compile). Gates 1 to 5 hold on this hardware.
- **Super 8 has a one-time load of about 40 ms.** Its first real render after every device open takes about 40 ms. The serial path takes it on the callback every time. The pool absorbed it on a worker in three runs. In the fourth (pool 64 under compile, the 41.2 ms longest callback), the admission logic rendered that strip inline, so the spike landed on the callback. Filed as [#168](https://github.com/transit-audio/transit/issues/168): a strip rendered inline has no deadline bound, so gate 3 is unmet for the inline case.
- **Strip overruns at 64 frames are a fixed-cost signature.** Overrun blocks at 64 frames (830 idle, 1,803 under compile) are each a one-block hole in one strip. They fall 14-fold when the buffer doubles to 128 frames idle (830 to 58) and 27-fold under compile (1,803 to 66). That is far more than a cost-bound miss would give, so a fixed overhead landing just past the 1 ms deadline is the main suspect. The report cannot attribute them to a strip or say how late they were. Filed as [#169](https://github.com/transit-audio/transit/issues/169); the deadline fraction or the wake path is decided from its data.

## Decision

Serial stays the default, and the pool stays opt-in (`TRANSIT_RENDER_POOL=1`). The default flips in its own reviewed commit, after #168 lands and #169's data has sized the 64-frame overruns, with the same eight runs repeated.

The bar for the repeat: no strip rendered inline past the budget, and overrun blocks at 64 frames idle at or near the 128-frame figure (58).

## Share runs

The strips' deadline share (#172), measured after #168 and #169 landed: build `ee038d33`, the same session and machine, 48 kHz, ten minutes per run, pool on, with `TRANSIT_STRIP_DEADLINE_PERCENT` at 75 (the former default), 100 and 150. The full per-run figures are on [#172](https://github.com/transit-audio/transit/issues/172).

| run | strip overrun blocks at 75 / 100 / 150 | longest callback at 75 / 100 / 150 |
|---|---|---|
| 64 idle | 1,565 / 406 / 73 | 2.5 / 2.0 / 3.1 ms |
| 64 under compile | 2,081 / 594 / 111 | 2.2 / 2.6 / 3.0 ms |
| 128 idle | 47 / 18 / 33 | 2.8 / 26.7 (one unattributed event) / 4.3 ms |
| 128 under compile | 73 / 29 / 18 | 4.1 / 4.1 / 4.3 ms |

- At 64 frames the share is the lever: strip overrun blocks fall from 1,565 to 406 to 73 idle and from 2,081 to 594 to 111 under compile, while the longest callback stays at 2 to 3 ms. The cost is over-budget callbacks (a few hundred per ten minutes at 100 and 150). Every run that kept its log had 0 driver overloads and 0 stream errors.
- At 128 frames the three shares are within noise of each other (18 to 73 blocks). The 26.7 ms callback at 100 idle is a single event the telemetry does not attribute; the 150 runs show nothing like it.
- Worker wake stayed under 50 µs for 99.99% of claims in every run.
- The maintainer's listening pass at 64 frames and 150% (about six minutes, the last part under a full workspace compile) was smooth until the compile pegged every core. The slight break-up then was the audio thread itself preempted (wake gap max 4.9 ms), which no share can help: 280 strip overrun blocks, 0 driver overloads, 0 stream errors.

**Decision (maintainer, 2026-10-28):** the share defaults to 150% of the period for device periods of 1.5 ms and below (64 frames at 48 kHz) and 100% for periods above 1.5 ms (128 frames at 48 kHz), taken from the device's callback; `TRANSIT_STRIP_DEADLINE_PERCENT` keeps overriding it for measurement ([ADR 0029](../adr/0029-strip-job-slots.md), "Default share"). The default flip of the pool itself (#124) still waits for #174.

## Related

- [2026-10-27 benchmark: render workers, serial against the pool (dev machine)](2026-10-27-ryzen-5900x-render-workers-serial-vs-pool.md)
- [ADR 0029: per-strip job slots for the multicore strip render](../adr/0029-strip-job-slots.md)

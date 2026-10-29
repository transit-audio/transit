# Keyed session through the task graph against serial: Scarlett (#174)

- **Date:** 2026-10-29
- **Machine:** AMD Ryzen 9 5900X (12 cores, 24 threads), 64 GB, Windows 11 Pro 26200. Focusrite Scarlett 2i2 over native ASIO at 48 kHz.
- **Build:** `46ce24f1` (#174 slice 1 on main: the keyed plan as one task per strip, behind `TRANSIT_RENDER_GRAPH=1`).
- **Question:** Does a session with a sidechain key keep the workers' benefit through the dependency graph?
- **Gate:** #174 section 8. [ADR 0029](../adr/0029-strip-job-slots.md) (amendment for the task graph). The unkeyed baseline is [gate 6](2026-10-28-scarlett-render-workers-gate-6.md).

## Method

The gate 6 reference session, with one change: the Solid Bus Comp on Inst 3 is keyed post-fader from Inst 1, so the plan has a sidechain key. The session is otherwise the same eight plug-in instances, and the engine is idle.

Each row is ten minutes of playback, driven through the control channel (open session, play, stop, Copy diagnostics, quit), so every run did the same thing. The render workers were on by default (8 workers), with the default strip deadline rule: 150% of the period at 64 frames and 100% at 128 frames.

- **Serial:** the keyed plan on the audio thread. This is the fallback that K88 documents for keyed plans today.
- **Graph:** the same keyed plan through the task graph, with `TRANSIT_RENDER_GRAPH=1`.

Every row had 0 driver overloads and 0 stream errors. The maintainer heard breakup in the 64-frame serial row and none in the 64-frame graph row. These runs were idle only; no run was made under compile.

## Results

| frames | path | callbacks | over budget | blocks with a strip overrun | upstream-missed skips | longest callback | driver overloads / stream errors |
|---|---|---|---|---|---|---|---|
| 64 | serial (keyed fallback) | 490,504 | 14,388 (2.9%) | 0 (nothing on the workers) | n/a | 42.1 ms | 0 / 0 |
| 64 | graph | 480,939 | 364 (0.08%) | 43 (busy 38, late 8) | 0 | 2.1 ms | 0 / 0 |
| 128 | serial (keyed fallback) | 240,129 | 107 (0.045%) | 0 | n/a | 49.3 ms | 0 / 0 |
| 128 | graph | 240,517 | 14 (0.006%) | 24 (busy 17, late 8, unstarted 1) | 0 | 3.3 ms | 0 / 0 |

Peak callback load since open fell from 3160% to 158% at 64 frames and from 1850% to 123% at 128 frames.

## Reading

- **The graph path is the safer callback at both sizes.** Over-budget callbacks fall by a factor of about 40 at 64 frames (14,388 to 364) and about 8 at 128 frames (107 to 14). The longest callback is bounded at 2 to 3 ms, against 42 to 49 ms on the serial path.
- **The keyed plan lands where the unkeyed pool rows of gate 6 did.** Gate 6's pool at 64 frames idle at the 150% share (its share runs, after #168 and #169) had 73 blocks with a strip overrun and a few hundred over-budget callbacks in ten minutes; the 2 over-budget callbacks and 830 overrun blocks of its main table were measured at the former 75% default. The keyed graph run has 364 over-budget callbacks and 43 blocks with a strip overrun, with a heavier Inst 1 to Inst 3 edge. The keyed serial run is the baseline that is audible at 64 frames.
- **The keyed compressor never missed its key.** Both graph runs had 0 upstream-missed skips in forty minutes. No dependent was skipped because its key arrived late.
- **Where the graph misses.** At 64 frames the strip overruns are busy 38 and late 8. The top attributed strip is strip 4 (36 overruns: 31 busy, 5 late). Strip 3 had 7 and strip 2 had 3. At 128 frames the top is again strip 4 (17 overruns: 14 busy, 3 late), then strip 2 (5) and strip 3 (4). The per-strip rows give a cost of 42 µs for strip 4 at 64 frames and 73 µs at 128; strip 2 costs 23 µs and 29 µs. The worst single renders were 128 µs for strip 4 at 64 frames, 460 µs for strip 4 at 128 frames and 309 µs for strip 3 at 128 frames.
- **Worker wake stayed fast.** At 64 frames, 1,880,981 of 1,903,452 claims woke in under 50 µs, 142 in the 500 µs to 1 ms bucket, 2 in the 1 to 2 ms bucket and none over 2 ms. At 128 frames, 942,120 claims woke in under 50 µs and 110 took more than 1 ms (105 in the 1 to 2 ms bucket and 5 over 2 ms).
- **Strip lateness was rare.** Two strip results arrived more than 2 ms late at 64 frames and one at 128 frames. The graph runs logged 8 late strip results each, and one deadline overshoot at 128 frames.
- **Inline renders.** The diagnostics counted 921,250 strips rendered inline at 64 frames and 465,050 at 128 frames. The inline renders refused by the admission rule were 2 (no history), 42 (worst case) and 12 (recent overrun) at 64 frames, and 3, 13 and 29 at 128 frames. These counts are reported as the diagnostics name them and are not separated further here.
- **Histograms.** The serial callbacks cluster at 512 µs to 1.024 ms at 64 frames (364,695) and at 1.024 to 2.048 ms at 128 frames (169,878). The graph callbacks sit lower: 343,197 in the 256 to 512 µs bucket at 64 frames and 179,266 in the 512 µs to 1.024 ms bucket at 128 frames. The serial 64-frame run has 6 callbacks in the 4.096 to 8.192 ms bucket and 1 over 16.384 ms; the graph 64-frame run has none over 4.096 ms.

## Decision

Yes: the keyed plan through the task graph keeps the workers' benefit. It lands where the unkeyed pool rows of gate 6 did, and the keyed compressor never missed its key.

The keyed default flip waits on one more gate. The maintainer asked for a signal-level regression showing that the compressor's gain reduction follows the intended key, with correct latency alignment, in both modes. That is #179 and #210 (DSP equivalence under seeded stall schedules), in progress. When it passes, the graph path becomes the default for keyed plans in its own reviewed commit, and K88's "renders serially" sentence goes ([known issues](../known-issues/README.md)). Until then keyed plans keep the serial fallback.

Limits of this record: one ten-minute run per row, on one machine, idle only, with the maintainer's listening pass covering the 64-frame pair only.

## Artifacts

The rows files, the full diagnostics reports and run logs for the four runs, the runner script and the keyed session are in the maintainer's diagnostics notes (measurement of 2026-10-29, #174 section 8).

## Related

- [2026-10-28 benchmark: gate 6, render workers through the Scarlett](2026-10-28-scarlett-render-workers-gate-6.md) (the unkeyed baseline)
- [2026-10-27 benchmark: render workers, serial against the pool (dev machine)](2026-10-27-ryzen-5900x-render-workers-serial-vs-pool.md)
- [ADR 0029: per-strip job slots for the multicore strip render](../adr/0029-strip-job-slots.md)
- Issues: [#174](https://github.com/transit-audio/transit/issues/174) (task graph), [#179](https://github.com/transit-audio/transit/issues/179) (gain-reduction regression, pending)

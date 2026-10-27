# Render workers against serial, engine level (#124 slice 4)

- **Date:** 2026-10-27
- **Machine:** AMD Ryzen 9 5900X (12 cores, 24 threads), 64 GB, Windows 11 Pro 26200. Development machine; **no audio device** (a paced render loop).
- **Code:** `feat/124-wiring` (slice 4 of #124, [ADR 0029](../adr/0029-strip-job-slots.md)).
- **Harness:** `crates/mix/tests/render_headroom.rs`, `cargo test --release -p transit-mix --test render_headroom -- --ignored --nocapture`.

**These are dev-machine numbers, not the decision.** They render through `MixEngine::render_active` on a promoted thread paced at the period, with live MIDI through `queue_event`, but without a device, a driver or the rest of the callback. The default stays serial until the maintainer's run on the Scarlett at 64 and 128 frames through the device (ADR 0012 G1, #124 gate 6).

## Fixture

16 audio strips (a compressor or a reverb each) and 4 continuity instruments (instrument strips played live with notes and the sustain pedal; `TRANSIT_BENCH_COST` sine evaluations per sample stand in for a plug-in's cost: 6 is light, 60 is heavy). 2000 blocks per row after 64 of warm-up, 48 kHz, workers 8 (cores minus one, capped), all promoted. A late block is a render longer than its period (1333 µs at 64, 2667 µs at 128). The strips' deadline is three quarters of the period. `workers+stall` forces one 10 ms stall in a worker inside a sustained note.

Loads: `idle`; `spin`, one busy normal-priority thread per logical core; `external`, `cargo check --workspace` in a fresh target directory running alongside (for cost 60 it ran through the whole table; for cost 6 it was looped and ran through the whole table).

## Light fixture (cost 6)

| frames | path | load | late /2000 | p50 µs | p95 µs | p99 µs | max µs | overruns busy/late/unstarted | inline | holes (blocks) | stuck |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 64 | serial | idle | 0 | 137 | 170 | 210 | 371 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | idle | 0 | 64 | 84 | 100 | 433 | 0/0/0 | 2612 | 0 | 0 |
| 64 | serial | spin | 0 | 169 | 190 | 201 | 226 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | spin | 0 | 78 | 85 | 91 | 387 | 0/0/0 | 3402 | 0 | 0 |
| 64 | serial | external | 0 | 222 | 253 | 273 | 333 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | external | 4 | 82 | 99 | 143 | 4263 | 0/0/0 | 4493 | 0 | 0 |
| 128 | serial | idle | 0 | 274 | 345 | 379 | 595 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | idle | 0 | 95 | 116 | 132 | 434 | 0/0/0 | 3153 | 0 | 0 |
| 128 | serial | spin | 0 | 316 | 330 | 353 | 435 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | spin | 0 | 106 | 116 | 128 | 172 | 0/0/0 | 4609 | 0 | 0 |
| 128 | serial | external | 0 | 305 | 507 | 601 | 853 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | external | 0 | 101 | 129 | 152 | 472 | 0/0/0 | 3082 | 0 | 0 |
| 128 | workers+stall | idle | 0 | 95 | 118 | 131 | 2018 | 3/1/0 | 3034 | 3 | 0 |

## Heavy fixture (cost 60)

| frames | path | load | late /2000 | p50 µs | p95 µs | p99 µs | max µs | overruns busy/late/unstarted | inline | holes (blocks) | stuck |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 64 | serial | idle | 0 | 358 | 447 | 530 | 718 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | idle | 0 | 134 | 152 | 162 | 449 | 0/0/0 | 2162 | 0 | 0 |
| 64 | serial | spin | 0 | 423 | 443 | 457 | 482 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | spin | 7 | 131 | 146 | 158 | 11025 | 0/0/0 | 2134 | 0 | 0 |
| 64 | serial | external | 0 | 483 | 572 | 755 | 1103 | 0/0/0 | 0 | 0 | 0 |
| 64 | workers | external | 3 | 130 | 148 | 186 | 5265 | 0/0/0 | 2111 | 0 | 0 |
| 128 | serial | idle | 0 | 819 | 880 | 932 | 1006 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | idle | 0 | 231 | 254 | 272 | 593 | 0/0/0 | 2115 | 0 | 0 |
| 128 | serial | spin | 0 | 840 | 1135 | 1168 | 1886 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | spin | 39 | 215 | 245 | 5830 | 18586 | 0/0/0 | 2128 | 0 | 0 |
| 128 | serial | external | 0 | 973 | 1058 | 1108 | 1369 | 0/0/0 | 0 | 0 | 0 |
| 128 | workers | external | 0 | 236 | 273 | 300 | 675 | 0/0/0 | 2086 | 0 | 0 |
| 128 | workers+stall | idle | 0 | 232 | 256 | 270 | 2025 | 3/1/0 | 2133 | 3 | 0 |

A second run of the heavy spin rows: workers 5 late at 64 (max 4817 µs), 0 at 128 (p99 2117 µs, max 2295 µs); serial 0 and 0.

## After the slice 4 review (finish spins instead of yielding)

`WorkerSet::finish` now spins between sweeps, bounded by the deadline (ADR 0029 deviation 4 amended). Same harness, same machine:

| fixture | frames | path | load | late /2000 before | late /2000 after | p50/p99/max µs after |
|---|---|---|---|---|---|---|
| heavy | 64 | workers | spin | 7 (and 5 in a second run) | 0 | 144/457/510 |
| heavy | 128 | workers | spin | 39 (and 0, p99 2117, in a second run) | 0 | 232/280/652 |
| light | 64 | workers | cargo check | 4 | 0 | 81/105/494 |

Every other row of both fixtures also read 0 late blocks after the change; serial is unchanged (heavy spin: 0 late at 64 and 128). The bench measures the deadline from the block's entry (no device callback); the app now measures it from the callback's entry.

## Reading

- **Typical cost.** The workers cut the median render time by about 2 to 3.5 times in every configuration, and the light fixture never came near the period on either path when idle.
- **Late blocks under load, with zero overruns.** Under a full spin load or a `cargo check`, the workers path had late blocks (3 to 39 per 2000, up to 18 ms) while serial had none, and **no overrun was counted in those blocks**: every strip finished, but the callback itself lost the processor. The cause is `std::thread::yield_now` in `WorkerSet::finish` (ADR 0029 deviation 4: spin 64 sweeps, then yield between sweeps). On Windows it is `SwitchToThread`, which hands the core to any ready thread, normal-priority ones included, for a scheduler quantum. Experiment (not committed): with the yield replaced by `spin_loop` hints, the same rows read 0 late blocks (heavy spin: max 386 µs at 64, 490 µs at 128; light external: max 587 µs and 576 µs). The slice 4 review made that change (see above): 0 late blocks in every row.
- **Overruns.** Only the forced stall made any: 3 busy and 1 late per run, each busy block a hole of one block in the instrument's time, no stuck note, the sustained note resumed and was released.
- **Inline renders.** The callback renders one to two strips per block itself (admission), so the workers path is never slower than serial at the median.

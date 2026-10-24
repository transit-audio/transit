# 2026-10-24, Ryzen 9 5900X, Behringer Model D on the Focusrite Scarlett 2i2 (native ASIO): the External Instrument Measure, which C, and where a note lands

The third #71 measurement. The first two ([onset latency](2026-10-24-behringer-model-d-midi-to-audio-onset-latency.md), [placement with the measured compensation](2026-10-24-behringer-model-d-placement-with-compensation.md)) ran in the bench's capture-position clock. This one runs the app's own path: the Measure of an External Instrument (ADR 0020 decision 3) through the Player, and then a grid clip played through the External Instrument with the compensation set to each candidate value. It asks which number the plan needs, why the Measure read 4 to 9 ms above the estimate in some sessions, and how fast Cancel silences the synth.

## Setup

- **Machine.** Windows 11, AMD Ryzen 9 5900X.
- **Chain.** MIDI out "MODEL D" (USB), channel 1, note 60, velocity 100, into a Behringer Model D. Its audio goes to input 1 of a Focusrite Scarlett 2i2, read over "Focusrite USB ASIO" at 48 kHz, duplex, native ASIO backend. No loopback cable was used for the Measure runs. Buffers 64 and 128.
- **Measure.** One warm-up note, then 8 notes 1500 ms apart, each held 100 ms. The result is the median of the per-note latency in plan frames.
- **Placement.** An 8-note clip (250 + 1500k ms, held 100 ms) on an External Instrument track, played through the Player with delay compensation on and the manual latency set to each candidate C. A tap on the strip input and on Main recorded the run by session sample. E is the Main onset minus the grid position of the note. All 8 notes were found in every pass.
- **Instrumentation.** A temporary read of the live input ring's backlog B after every block and the two taps. None of it is in the product. The software state was the slice 2 branch of the External Instrument work.

## Result 1: C moves with the input ring backlog B

`InputRing` is a plain FIFO. Whatever queues between opening the input and the plan adopting it stays queued for the life of the attachment, and push and pop then run in lockstep. B was constant within every attachment (minimum equal to maximum over the whole run) and differed between attachments.

| Buffer | Attach | B (frames) | Measured C (frames) |
|---|---|---:|---:|
| 64 | as the app does it, three sessions | 256 | 1341, 1340, 1337 |
| 64 | 3 ms pause before adoption | 192 | 1303 |
| 64 | 20 ms pause before adoption | 448 | 1540 |
| 64 | ring cleared after adoption, twice | 0 | 1083, 1083 |
| 128 | as the app does it | 256 | 1663 |
| 128 | 10 ms pause before adoption | 256 | 1644 |
| 128 | ring cleared after adoption | 0 | 1391 |

With B = 0, C = 1083 frames (22.6 ms) at 64 and 1391 (29.0 ms) at 128. C - 326 (the hardware insert round trip there, taken with the ring cleared) is 757 frames = 15.8 ms at 64, inside the 14.5 to 16.5 ms range the ADR expects. Against the position loop of 328 it is 755 frames (15.7 ms), and at 128 against 648 it is 743 frames (15.5 ms). The bench's estimate of MIDI delivery plus synth (15.8 ms) matches.

The model that fits every row is C = O + L_bench + B, with O the output offset (139 frames at 64), L_bench the bench's send-to-capture time (948 at 64, 1039 at 128) and B the backlog:

| Buffer | B | O + L + B (predicted) | Measured C | C - B |
|---|---:|---:|---:|---:|
| 64 | 0 | 1087 | 1083, 1083 | 1083 |
| 64 | 192 | 1279 | 1303 | 1111 |
| 64 | 256 | 1343 | 1341, 1340, 1337 | 1081 to 1085 |
| 64 | 448 | 1535 | 1540 | 1092 |
| 128 | 0 | 1386 | 1391 | 1391 |
| 128 | 256 | 1642 | 1663, 1644 | 1374 to 1407 |

The stamp of the notes (the presentation time of the render frame) minus callback entry was 135 to 153 frames in every 64 session, B = 0 included, so the stamp did not move. Recording is not affected (it captures by device frame), and neither is the position-loop calibration. The plan-frame consumers of the live input are the External Instrument return, hardware insert returns and input monitoring (3 to 7 blocks of extra latency today).

## Result 2: where a note lands

E is the median error of the onset on Main against the grid, in ms (positive is late). "In-app" is the C that Measure found in the same session. "Bench" is the 955 / 1039 median from the capture-position benchmarks. "In-app - 326" subtracts the hardware insert round trip. "Cross-session" uses the C of a session with B = 256 in a session with B = 0.

| Session | Buffer | Compensation | C used (frames) | E median (ms) | E sd (ms) | Worst note (ms) |
|---|---:|---|---:|---:|---:|---:|
| B = 256 | 64 | in-app | 1341 | -0.19 | 0.26 | 0.60 |
| B = 256 | 64 | bench | 955 | +7.93 | 0.38 | 8.67 |
| B = 256 | 64 | in-app - 326 | 1015 | +6.88 | 2.58 (one late send) | 14.58 |
| B = 0 | 64 | in-app | 1083 | +0.17 | 0.31 | 0.60 |
| B = 0 | 64 | bench | 955 | +2.77 | 0.32 | 3.23 |
| B = 0 | 64 | in-app - 326 | 757 | +6.67 | 0.31 | 7.17 |
| B = 0 | 64 | cross-session | 1341 | -5.38 | 1.00 | 8.17 |
| B = 0, repeat | 64 | in-app | 1083 | +0.29 | 0.24 | 0.65 |
| B = 256 | 128 | in-app | 1663 | -0.55 | 0.58 | 1.10 |
| B = 256 | 128 | bench | 1039 | +12.35 | 0.62 | 14.06 |
| B = 256 | 128 | in-app - 326 | 1337 | +6.91 | 0.74 | 8.60 |
| B = 0 | 128 | in-app | 1391 | -0.14 | 0.28 | 0.63 |
| B = 0 | 128 | bench | 1039 | +7.46 | 0.50 | 8.50 |
| B = 0 | 128 | in-app - 326 | 1065 | +6.75 | 0.31 | 7.19 |
| B = 0 | 128 | cross-session | 1663 | -5.88 | 0.32 | 6.42 |

Reading: the in-app value lands on the grid in every session (median within 0.6 ms, worst note within 1.1 ms). The other values are late by exactly the difference from the true C, and a C carried from a session with another B is off by minus the difference in B (-258 frames, -5.4 ms at 64; -282 frames, -5.9 ms at 128). The bench value is right for the bench's own clock and not for the plan: the bench never touches the output map or delay compensation.

## Result 3: Cancel

A plain note-off (no Cancel) of the same patch, measured on the strip input with B = 0: -23.4 dBFS at the note-off, -31 at +50 ms, -46 at +100, -53 at +200, -55 at +300 and -63 at +400. It reaches -60 dBFS (10 ms peak window) after 367 to 371 ms, identically on all 8 notes. Cancel runs reached -60 dBFS at 219 to 241 ms, faster than the synth's own release from a normal note-off. The time to -60 dBFS is a property of the patch, not of the app, so the acceptance for Cancel is on the wire (note-off and CC 123 within 20 ms of the Cancel) and on the return (the level never rises after the Cancel, and -60 dBFS within the synth's natural release plus margin), not a fixed 200 ms.

## Other observations

- **MIDI timing sawtooth.** Within a run the per-note value falls about 7 frames per note, then wraps by 41 to 48 frames (for example 1311, 1352, 1345, 1337, 1330, 1322, 1363, 1355). That is a 1 ms period: the MIDI path's 1 ms quantization beating against the 1.5 s note spacing. A uniform 1 ms spread has a standard deviation of 0.29 ms, which matches the 0.24 to 0.31 ms seen at B = 0. The median absorbs it. In one session the run stepped by about 1 ms between two notes with B and the stamp unchanged; the spread was 0.84 ms, under the 1.0 ms bar.
- **Late sends.** Late counts were 0 in every session except one (3 sends, the worst 6.7 ms late, one outlier note). The median absorbed it.
- **Measure and playback agree.** With the clip played through the real Player, the strip-input onset minus the note position read 1332 / 1335.5 / 1345 frames against a Measure of 1341 in the same session, and 1091 / 1088 / 1077 against 1083 in a B = 0 session.

## What this changed

The Measure now refuses a run during which B changed, and ADR 0020 records that C includes B, which the K22 fix makes 0 by construction. A C stored in one session is only meaningful in another if B is the same, so the hardware acceptance measures three times with a device reopen between runs.

# 2026-11-16, Ryzen 9 5900X, no audio device: what does the band-limited reader cost by rate ratio?

Issue #726. [ADR 0056](../adr/0056-audio-clip-editor.md) decision 8.8 sets a cost budget for the reader: a stereo clip at 48 kHz through the clip path costs at most max(1, q/2) percent of one core, where q is the read rate.

Setup: quiet machine, release build, stereo at 48 kHz, one second of output per run, median of five runs, percent of one core. The reader alone is `cargo run --release --locked -p transit-resample --example reader_bench`. The clip path (one stretched stereo clip with gain and fades) is `cargo run --release --locked -p transit-mix --example clip_path_bench`. The budget in the decision covers the whole clip path; the reader-alone column is held to the same figure, so it is the stricter check.

| Rate q | Budget | Reader alone | Clip path |
|---|---:|---:|---:|
| 0.5 | 1.00% | 0.33% | 0.55% |
| 1.0595 | 1.00% | 0.77% | 1.05% |
| 2 | 1.00% | 1.36% | 1.84% |
| 4 | 2.00% | 2.69% | 3.63% |
| 16.47 | 8.23% | 10.70% | 15.13% |
| 32 | 16.00% | 20.87% | 29.19% |

The unstretched clip path is 8% cheaper than before for mono and stereo and 31% cheaper for six channels.

## Conclusion

The reader alone is over budget above about q 1.06, and the clip path is over at every listed rate from 1.0595 up (the clip path costs 35 to 40% more than the reader alone at the same rate). Unstretched clips are not affected. This blocks Sampler voices on the reader (#605), which would each pay this cost. The work to bring both inside the budget, or to amend the budget with the measured figure and the reason, is tracked in #726; the output must stay as the decision requires.

# Export marker notice counted a marker already at the start

1. **Failure.** At 128 BPM and 44.1 kHz, a marker on the sample representing the pre-count
   zone's first bar line was reported as moved, although it exported at time zero.
2. **Mechanism.** `export_marker_notice` in `crates/engine/src/io/midi.rs` compared a stored
   sample converted to fractional ticks against the ideal bar-start tick. That bar line lies
   halfway between samples. The comparison now uses the export's rounded start sample.
3. **Why checks missed it.** The existing export tests used 120 BPM and 48 kHz, where the
   bar line is an integer sample. Review F1 found the missing fractional boundary case.
4. **Regression test.** The three `*_does_not_count_a_marker_on_a_fractional_bar_start`
   tests in `crates/engine/tests/export_markers.rs` each failed on the spurious notice before
   the fix. They also check time zero and count a marker one sample before the start.
5. **Gate class.** Export-boundary notices are tested with a boundary between samples,
   markers on either side and a marker on the stored start sample.
6. **Protocol change, owner, blind spot.** The engine tests and the review rule in
   `../contributing/evidence/gates.md` cover this class; the maintainer owns the gate.
   The fixture covers one tempo and rate with a fractional boundary, not every possible map.

# E70: the phase-marker test compared wall-clock stretches (#323)

1. **Failure.** Linux CI run 41773292749 on main d554a443 failed
   `strip_pool::tests::a_strip_rendered_inline_marks_the_phase_marker`: `gap_phase` was
   `Mixdown`, not `StripRender`, with `longest_gap_ns: 116536` and `gap_strip: 4294967295`.
   The Windows and macOS jobs passed.
2. **Mechanism.** `PhaseMarker::enter_at` (`crates/rt/src/phase_marker.rs`) records the
   previous phase and strip only when the elapsed gap exceeds the previous maximum. The test
   (`crates/mix/src/strip_pool/tests.rs`, from #162) therefore required a strip render to
   outlast every other phase. The marks in `crates/mix/src/active.rs` and `strip.rs` were
   unchanged by #321 (`a58ae016..d554a443`); the added status writes and hardware processing
   did not change which marks were made. This is a test defect, not a missing product mark.
3. **Why checks missed it.** Earlier runs happened to spend the longest stretch in a reverb
   strip. Timing rules (a) through (e) did not recognize a relative duration comparison
   expressed through a phase enum, with no duration bound in the assertion.
4. **Regression test.** The same test retains `inline == 5 * 6` and observes the last
   `StripRender` index, `Some(5)`, after the render thread joins. The marker retains that
   index with a relaxed atomic store and resets it at `begin`; later phases cannot erase it.
   Temporarily changing the strip's `enter` call to mark `Idle` fails this observation with
   `None` instead of `Some(5)`; restoring the call passes. The marker's unit test checks
   successive indices, preservation across other phases, reset and an absent index, all
   with injected instants. `gap_strip < 6` is removed: a non-strip winning phase correctly
   has `NO_STRIP`, so that assertion was also clock-dependent.
5. **Gate class.** E49: a relative comparison of measured stretches is a wall-clock outcome.
   Assert longest-gap fields only with injected instants; assert a deterministic entry or
   state for instrumentation coverage. A trial `cargo xtask timing` rule (f) rejected the
   original test but also rejected the deterministic telemetry test
   `the_longest_callback_keeps_the_phase_and_strip_it_stood_in`, which injects every instant
   through `worst_tests::run`. The function-local check is not exact and was not adopted.
6. **Protocol change, owner, blind spot.** The coordinator owns the E49 rule in
   `docs/contributing/evidence/gates.md`. Review checks the actual clock source and asserted
   marker through helpers; a future automated gate needs clock-provenance analysis. The trial
   also found `the_render_writes_its_phases_to_the_marker_without_allocating` in
   `crates/mix/src/plan_render_tests.rs`: its positive timestamp and non-idle longest-gap
   assertion remain a review follow-up. The retained last index proves that entry, not that
   every strip emitted a mark.

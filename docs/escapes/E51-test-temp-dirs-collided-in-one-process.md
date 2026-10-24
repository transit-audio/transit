# E51: test temp dirs collided between concurrent tests (#89)

1. **Failure.** In a full workspace run beside other test suites,
   `audio_review_tests::desktop_logic_saves_preferences_before_exit_and_keeps_future_audio` failed
   with `AlreadyExists` creating its temporary directory. It passed alone.
2. **Mechanism.** `config::TemporaryAutosave::new` (`apps/transit/src/config.rs`) named its folder
   `transit-test-<pid>-<clock nanoseconds>` and used `create_dir`. Tests are threads of one
   process, so the pid is shared, and two threads can read the same clock tick (coarse on
   Windows). The second `create_dir` failed.
3. **Why checks missed it.** Only a contended run lines threads up on one tick; a single test, or
   a quiet machine, never does.
4. **Regression test.** `temporary_autosaves_made_at_the_same_instant_get_distinct_folders`
   releases 16 threads together to create 800 folders and requires 800 distinct paths. The name
   now carries a per-process atomic counter.
5. **Gate class.** Tests create unique temp dirs (pid, a per-process counter, and the clock), never
   a fixed name or a clock-only suffix (`docs/contributing/evidence/gates.md`).
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spot: many tests name their folder `<prefix>-<pid>` only. That is unique per test within a
   process as long as each prefix is used once, and a stale folder from a killed run is removed
   first by most of them; a shared helper would remove the question.

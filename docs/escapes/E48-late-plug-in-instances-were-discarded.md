# E48: late plug-in instances were discarded (#82)

1. **Failure.** On build 8ee5c5f a session load with NI Battery 4 and Kontakt 7 on a heavily
   loaded machine left both slots reading "Not responding", with no way back short of reopening
   the session. The log shows Battery made 300 ms after the 10 s creation wait and Kontakt 70 ms
   after it: each "not made within 10s; the slot stays empty", and a moment later "a host thread
   that was not responding has returned". Both instances were made and then thrown away.
2. **Mechanism.** The plug-in host counted a creation past its 10 s request timeout as stuck
   (`Status::Stuck`) and wedged its thread. The Player's staging (`handles.rs`) took the first
   `Stuck` as final: it marked the slot stuck, compiled the session without the instance and
   dropped the pending result, so an instance that arrived after that was destroyed on its host
   thread. Only removing and re-adding the plug-in, or reopening the session, tried again.
3. **Why checks missed it.** The 10 s wait was there to decide what the slot displays and how
   long a request holds other edits, but it doubled as an abandonment deadline. The tests covered
   a creation that finishes well inside the wait and one that never returns; none covered a
   creation that finishes just after it, the common case for a heavy library under load.
4. **Regression tests.** `crates/playback/src/player/runtime/late_tests.rs`:
   `a_creation_slower_than_the_display_budget_is_adopted_not_discarded` (manual clock),
   `a_creation_that_never_returns_reads_not_responding_only_at_the_creation_budget`,
   `retry_makes_a_not_responding_plug_in_again_with_its_stored_state`,
   `retry_all_makes_every_not_responding_plug_in_again`. `crates/plugin-host/tests/host_thread.rs`:
   `a_creation_slower_than_the_request_timeout_is_delivered_and_hands_the_queue_over`,
   `a_creation_that_returns_after_reading_stuck_is_still_delivered`.
   `crates/ui-studio/src/plugin_retry_tests.rs`: the slot text sequence "Loading…" to
   "Loading… (slow) 11 s" to the plug-in, the card and mixer-row Retry buttons and context menu,
   and the status line's "Retry all"; `plugin_host_tests.rs`:
   `a_heavy_instrument_slower_than_the_display_budget_is_adopted` (the real slow-create VST3
   fixture, 1.5 s, against a 300 ms display budget). Mutations: putting the discard back fails
   the adoption tests; dropping the stored state from Retry fails the Retry tests.
   The refutation review added two more: a save during a slow creation gives up within about one
   request timeout for all the plug-ins on that thread, not one each
   (`a_save_during_a_slow_creation_waits_about_one_timeout_not_one_per_instance`), and picking
   the same plug-in again on a slow slot makes it again and binds it
   (`picking_the_same_plug_in_again_on_a_slow_slot_makes_it_again_and_binds_it`); with
   `an_abandoned_creation_returning_after_retry_is_destroyed_not_bound` beside them.
5. **Gate class.** A wait that only affects what is displayed must never discard work. A
   deadline that abandons a result needs its own name, its own budget and a test of a result
   that lands just after it.
6. **Protocol change, owner, blind spot.** The host tests include the heavy-instrument case: a
   creation that runs past the display budget and still lands (the slow-create VST3 fixture in
   `plugin_host_tests.rs`, the gated factories in the runtime, host and UI tests). Owner: the
   coordinator. Blind spots: the creation budget is a fixed 120 s (#67 tracks load timing; an
   adaptive budget is not designed); creations queued behind a slow one move to a fresh thread
   after 10 s, which spends a thread per slow load as before; the Play button still pulses
   "Waiting for plug-ins to load" while a slow plug-in loads (the session itself no longer waits
   for it); a hung VST3 module entry makes a Retry of the same binary hang again (K42).

# E58: an armed learn skipped the first message of a port it opened

1. **Failure.** `player::device::midi_map_tests::an_armed_learn_opens_every_port_and_reports_the_next_message_with_its_port_name`
   (`crates/playback`) failed once with "did not reach: the candidate" after a 10 s poll (#160). The
   worker wakes at least every 20 ms, so a 10 s miss is a lost event, not a slow host. A user would
   see a learn that waits for a second touch of the controller after the first was ignored.
2. **Mechanism.** The ingress worker's MIDI Learn scan (`Pump::step`, `crates/playback/src/player/ingress.rs`)
   skips, on the first step after the matcher turns on (`map_pass_on` false to true), everything
   already pending on every input, so a backlog from before the learn or the mappings is not
   matched. Main arms the learn (`Worker::arm_learn`, an atomic store plus an unpark), the learn
   opens every present port, and Main attaches each opened port (`Native::service`,
   `Worker::attach_input`, `Update::Input` on the worker channel; `attached` is set on Main in the
   same `&mut self` call, so `take_map_events` always finds the port id: no race there). If the
   worker thread is not scheduled between the arm and the port's first message, its next step
   drains the `Update::Input`, sees the learn armed for the first time, and skips that port's
   pending messages as "already pending", though every one of them arrived after the arm. The
   candidate is gone and nothing re-sends it.
3. **Why checks missed it.** The window needs the worker to stay unscheduled across the port open
   (an opener-thread round trip and a Main service), which a quiet host never does; the worker
   unit tests attach their port and step before arming, so the step that turns the matcher on
   never also carried an attach.
4. **Regression test.** `a_port_the_armed_learn_opens_keeps_its_first_message_when_the_worker_sees_both_in_one_step`
   (`crates/playback/src/player/ingress/map_tests.rs`) arms the learn, queues the attach and
   delivers two note-ons before a single `step`, and requires the first as the candidate, while a
   port the worker already held keeps its stale CC skipped. It failed on the old code (no event)
   and passes with the fix: the first-step skip applies only to the inputs the worker held before
   that step's attachments.
5. **Gate class.** A "skip what was already pending" cutoff applies only to sources the consumer
   held before the cutoff's trigger; a source attached in the same step is new
   (`docs/contributing/evidence/gates.md`). Worker tests that cover a cutoff include the case where
   the trigger and an attach land in one step.
6. **Protocol change, owner, blind spot.** The rule is in `gates.md`. Owner: the coordinator.
   Blind spot: a port opened for another reason (the Monitor) and attached in the same step as a
   later arm has its few pre-arm messages treated as new, so one could become the candidate;
   the worker would have to stay unscheduled across both the open and the arm, so this stays a note.

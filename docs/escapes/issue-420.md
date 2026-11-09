# E79: a plug-in destroy sent after host shutdown could still be accepted (#420)

1. **Failure.** `a_job_sent_after_shutdown_is_never_run` (`crates/plugin-host/tests/host_thread.rs`)
   failed in the Linux CI job on `main` 085c0438 (`cargo test --locked --workspace`): the Fake
   instance's destroy was logged on another thread (`left: Some(ThreadId(11))`) after
   `shutdown` had returned true. The commit did not touch the plug-in host. The same commit
   passed the local Windows and WSL Linux gates and the macOS CI job. The CI job was not rerun.
2. **Mechanism.** `Host::shutdown` (`crates/plugin-host/src/lib.rs`) returned once each host
   thread's shutdown job had run (its slot read DONE). Only after that did `run` store
   `running = false`, drain the queue (leaking it) and drop the receiver. `Worker::send` checked
   `running` and then sent, with no lock between the two. Interleaving: the test drops its
   handle; `send` reads `running == true`; the host thread stores false, drains an empty queue;
   the send lands; `drop(rx)` drops the queued job, whose unrun closure owned the instance, so
   the instance was destroyed on the host thread after the shutdown hook had run the VST3
   module exits. The job never ran, but the plug-in it carried was destroyed. That is the case
   the drain exists to prevent, and it breaks the issue #56 (b) rule that the sender leaks it.
   The order is in the shared `run`, the same on every platform.
3. **Why checks missed it.** The host thread goes straight from reporting DONE to the close,
   while the test thread first has to wake from a condition variable, so the close almost always
   won. Only a host thread preempted in that span on the loaded two-core Linux runner lost. The
   test then slept 100 ms and asserted no destroy, which detects the race only when the
   scheduler produces it. No test checked the gate at the moment `shutdown` reports done.
4. **Regression test.** The gate is now a lock-protected `Option<Sender>`. `Worker::close`
   takes it under the lock `send` holds for its check and its enqueue, and the shutdown job
   closes it first, before the hook and before DONE. `the_shutdown_job_closes_the_gate_before_it_reports_done`
   (lib unit test) probes the gate from inside the shutdown hook, then once more after `shutdown`
   returns. `a_send_after_the_close_is_refused_and_one_before_it_is_queued` covers the gate's
   atomicity. The integration test lost its sleep and its wall-clock bound. It asserts the
   refused instance is leaked inside the drop, so the check needs no wait. Controls: with the
   close removed from the shutdown job, the unit test fails on every run (`the gate was open
   while the shutdown hook ran`, `left: Some(true)`). With temporary sleeps forcing the CI
   interleaving on the old code (inside `send` between the check and the enqueue, and on the
   host thread around the drain), the old integration test fails with the CI message on every
   run. The same instrumentation passes with the fix.
5. **Gate class.** A thread that stops taking work closes its intake under the lock its
   senders hold for check and enqueue, and does so before it reports the stop. Tests of
   "nothing sent after the stop runs" probe the intake from inside the stop or assert the
   synchronous refusal, never sleep after the send.
6. **Protocol change, owner, blind spot.** The rule is in
   `docs/contributing/evidence/gates.md`. Owner: the maintainer. Blind spots: it is a review
   rule, since no scan finds check-then-send races. Host threads that were not responsive at
   shutdown get no shutdown job and keep their gate open (their modules did not exit either).
   A creation submitted at the same moment as `shutdown` can queue behind the shutdown job and
   read Waiting until its own deadline. Both are unchanged.

# E65: a hosted plug-in test ended its process before plug-in hosting shut down

1. **Failure.** `midi_learn_hosted_tests::a_learned_controller_moves_a_hosted_instruments_parameter_in_the_plug_in_and_its_editor`
   (`crates/ui-studio/src/midi_learn_hosted_tests.rs`) failed in the local WSL Linux gate at
   c9a9a7d0 (chain CG, the #197 racks branch, not pushed). Caught by the local gate, not CI. The
   test runs itself in a child process; the child's test passed ("1 passed") and the child then
   exited with status 3: `vst3-fixture: host contract violation: static teardown of a module that
   is not in use but was never exited`. The parent's exit status check failed. It passed on
   Windows in the same gate run, in the gate's starved rerun, and in 72 loaded single-test runs
   and 10 full `transit-ui-studio --lib` runs afterwards. No user-facing defect: the app's quit
   (`StudioApp::shut_down`) shuts plug-in hosting down before the process ends.
2. **Mechanism.** The player's instance of the fixture synth lives on the plug-in host thread
   (`plugin-host-0`, ADR 0018). Dropping the player sends its instances back to that thread,
   which releases them in its own time. The test ended there: no `shut_down`, so nothing waited
   for the host thread. Then the process exit handlers ran. `exit_modules`
   (`crates/vst3-host/src/ffi/module.rs`) found the bundle still referenced by the instance being
   released, so it kept the module, as it must (it never exits a module with a live instance).
   The host thread then finished the release, and the fixture's static teardown (an exit handler
   that stands in for a real plug-in's C++ static destructors) found the module entered, with no
   objects alive, and never exited. That is the K-series case real plug-ins crash in (HALion Sonic
   in its static destructors without its module exit, issue #104). Repro in WSL: a 100 ms pause at
   the start of a host-thread instance drop plus a 300 ms pause inside `exit_modules` (debug
   only, not committed) gave the exact message every run, at c9a9a7d0 and at 48ece848 (main before
   #197). Latent since the test was written (e390943a, #97); no #197 commit is involved. The same
   pause failed the four `plugin_header_tests` children (they also ended without `shut_down`).
   Windows: the fixture DLL's exit handlers run at `DLL_PROCESS_DETACH`, after `ExitProcess` has
   ended every other thread, so a release still running is frozen with objects alive and the
   check reads the module as in use; it fails only if the release ends between the executable's
   exit handler and the thread kill. The window is narrow on both systems, wider on Linux, where
   the host thread runs through all of glibc's exit handlers.
3. **Why checks missed it.** On a quiet host the host thread releases the instance before the
   test function returns, so the exit handler finds the bundle unused and exits it. Sibling
   tests (`plugin_host_tests`) already ended with the quit order; nothing required it of a test
   that hosts a plug-in through the player in a child process.
4. **Regression test.** The five children now end with the app's quit order,
   `StudioApp::shut_down`, and assert it was clean (`ShutDown::clean`: the host thread ended in
   time and every module had its exit). The fixture's contract check is unchanged. With the
   debug pauses: the midi learn test and all `plugin_header_tests` pass (they failed before).
5. **Gate class.** A test that hosts a plug-in through the player or the plug-in host ends with
   the app's quit order (`StudioApp::shut_down`, or the player dropped and then
   `transit_engine::shutdown_plugin_hosts`) and asserts it was clean, before its process ends.
6. **Protocol change, owner, blind spot.** The rule is in `docs/contributing/evidence/gates.md`.
   Owner: the coordinator. Blind spot: no lint finds a hosted-plug-in test without the quit
   order; the race shows only when the host thread's release overlaps the exit handlers.

See also: [E56](E56-a-lost-graphics-device-froze-the-app-in-a-plug-in-unload.md), the quit order's
other escape.

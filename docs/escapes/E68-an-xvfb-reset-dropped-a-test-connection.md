# E68: an Xvfb reset dropped a test's X11 connection

1. **Failure.** `an_embedded_gui_opens_in_an_x11_window` (`crates/clap-host/src/ffi.rs`) failed on
   main 4335c0b8 in ci.yml run 41370050198 (linux job, step "Test editor windows (Xvfb)") with
   "embedded, not floating": the editor opened, but floating. 4335c0b8 changed no code from
   8fe01bc2, where the same step passed. The embedded open had failed at the host window, and the
   editor link fell back to the plug-in's floating editor, as it should (gui.rs, the `Err(e) if
   floating` arm). The reason was logged at INFO, which the test does not show. Captured in WSL:
   `host window failed: cannot open the X11 display (DISPLAY=":99"): Broken pipe (os error 32)`,
   and in another run `Connection reset by peer (os error 104)`.
2. **Mechanism.** An X server resets when its last client disconnects unless it runs with
   `-noreset`: it closes every client, deletes the root window's properties and starts over. A
   connection that arrives during the reset is dropped while it sends its setup. `xvfb-run` starts
   Xvfb without `-noreset` (Ubuntu's default arguments are `-screen 0 1280x1024x24`). The editor
   window code opens one X11 connection per thread (`crates/rt/src/editor_window/x11.rs`, `conn`),
   and each test thread closes its connection when it ends. So whenever every other X test thread
   of the process had just finished, the client count reached zero, the server began a reset, and
   the next thread to connect failed. #289 (8fe01bc2) added eight `gui_diagnostics_tests` that open
   and close editors in parallel with this test, which made the count reach zero more often. Three
   further failures of the same kind were seen in WSL (`set_parent_false_warns_once`,
   `zero_size_warns_once`, `timer_required_...`: each open failed at the host window). A real
   desktop never resets: its window manager and the app's own window stay connected.
   Repro in WSL (Ubuntu 22.04, `xvfb-run -a`): the clap-host lib tests with one spinning process per
   CPU, 3 of 400 runs failed. Four threads that each connect, make one round trip and leave, 500
   times: 437 of 2000 connects failed with the default server, 0 of 2000 with `-noreset`. A root
   property set by one client is gone after it leaves with the default server, and kept with
   `-noreset`.
3. **Why checks missed it.** The WSL gate runs the tests on WSLg's display, where Weston stays
   connected, so the server never resets; it does not run CI's Xvfb step. CI ran the step on a
   resetting server, and only the timing of the test threads kept the count above zero. The
   private Xvfb of `crates/rt/tests/editor_x11.rs` lacked `-noreset` too. Not an E64 recurrence:
   E64 was request order between two live connections; here the connection never opened.
4. **Regression test.** `every_xvfb_the_tests_run_on_keeps_running_when_its_last_client_leaves`
   (`xtask/src/tests.rs`) checks every `xvfb-run` command in the workflows and every test that
   starts its own `Xvfb` for `-noreset`. It failed on 4335c0b8 (ci.yml:106 and editor_x11.rs) and
   passes now. The fix: both start Xvfb with `-noreset`. After it, under the same load, 0 of 400
   runs of the clap-host lib tests failed, and the full Xvfb step passed 10 of 10 in WSL. A test
   that observes a reset from inside cannot be deterministic: it would race the server's handling
   of its own disconnect.
5. **Gate class.** An X server that tests run on is started with `-noreset`: test threads open
   and close connections, so a resetting server can drop any of them. The xtask test enforces it
   for the workflows and the tests' own servers.
6. **Protocol change, owner, blind spot.** The rule is in this record, the ci.yml step's comment
   and the editor_x11 private Xvfb's docs. Owner: the editor window module's maintainer. Blind
   spot: the WSL gate script lives outside the repository and does not run CI's Xvfb step, so a
   local run on WSLg cannot show this class; the repro is the one in section 2.

See also: [E64](E64-an-x11-window-was-handed-out-before-the-server-made-it.md), the earlier X11
race under Xvfb.

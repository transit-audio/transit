# E64: an X11 editor window was handed out before the server made it (#147)

1. **Failure.** `create_resize_follow_and_destroy_with_a_fake_plugin` (`crates/rt/tests/editor_x11.rs`)
   failed on main b57efaac in rt.yml run 41052509463 (linux job): `left: None, right: Some((300, 200))`
   at the first geometry check, right after `EditorWindow::new`. The change that landed (#196) touched
   nothing in X11. The same failure is a product defect, not only a test defect: a plug-in handed the
   new window's id could ask the server for a child of a window that did not exist yet (`BadWindow`
   in the plug-in's own connection, which Xlib's default handler turns into an exit).
2. **Mechanism.** The X server keeps request order per connection only. `EditorWindow::new`
   (`crates/rt/src/editor_window/x11.rs`) created the frame and content windows on the host thread's
   connection and only flushed: flushing puts the requests on the socket, but the server may process
   another client's later request first. The test's observer (another connection) asked for the
   geometry before the server had read the creation, and got `BadWindow`, so `None`. A plug-in
   attaching from its own connection is the same case. So is a resize (`set_content`): a plug-in
   that reads its parent's size after `resizeView` could get the old one. And so is `close`: a
   destroy that other clients may not see yet. Repro in WSL (Ubuntu 22.04, the test's private
   Xvfb): with one spinning process per CPU, 20 of 20 runs failed at that line with the CI message.
3. **Why checks missed it.** The test ran on quiet hosts only (the WSL gate, WSLg, the review's
   runs). The CI Linux job ran it under Xvfb, but the server always read the creating connection
   first until a loaded runner reordered the two clients. No lint looks at cross-connection
   ordering. It is the E49 family (a test that depended on scheduling), but the wait it lacked was
   in the product.
4. **Regression test.** No test change was needed: the product now ends `EditorWindow::new`, every
   resize (`set_content`) and `close` with a round trip (`settle`, x11rb's `sync`: the server has
   processed everything the connection sent before the call returns). The test's checks right after
   those calls now pin that guarantee for windows the server alone decides: before a window is
   mapped, and for the content window. One test change: once the frame is mapped, a window manager
   applies its resize as a ConfigureRequest in its own time (WSLg's Weston), which no round trip of
   ours covers. The test now waits for the frame's size, with the failing 5 s stall bound. After
   the fix: 0 of 20 failed under load (20 of 20 before) and 0 of 20 unloaded on the private Xvfb;
   the rt tests passed 5 of 5 under `xvfb-run -a`, and editor_x11 passed on WSLg.
5. **Gate class.** Code that hands an X11 resource to another client (a plug-in, a test's observer)
   makes it with a round trip first. Code that checks one client's effect from another client waits
   for the first client's round trip, not for time to pass.
6. **Protocol change, owner, blind spot.** The rule is in the module docs of
   `crates/rt/src/editor_window/x11.rs` (the contract list) and in this record. Owner: the editor
   window module's maintainer. Blind spot: no gate runs the X11 tests on a loaded host; the WSL
   repro script (one spinning process per CPU) is the manual check.

See also: [E49](E49-echo-latency-test-wall-clock-bound.md), the scheduling-dependent test family.

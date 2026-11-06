# E74: a window manager moved a test's plug through a reused window id (#346)

1. **Failure.** The maintainer's Linux pre-push gate (WSL2 Ubuntu 22.04 on WSLg's display, the CI
   linux job's `cargo test --locked --workspace --no-fail-fast`) failed
   `ffi::gui_diagnostics_tests::xembed_plug_is_mapped_notified_resized_and_unmapped_by_the_host`
   (`crates/clap-host/src/ffi/gui_diagnostics_tests.rs`) on 0362d58d, a README-only commit, at
   the size assertion ("the child's initial 123x45 size follows the resized content"). The lib
   test binary finished in 0.08 s. The same gate passed on the commits before and after. The host
   machine was loaded by other work while it ran.
2. **Mechanism.** X hands a closed client's resource-id range to the next client that connects,
   and each client numbers its windows from the start of its range. The editor window code opens
   one X11 connection per thread (`crates/rt/src/editor_window/x11.rs`, `conn`), so every test
   thread's first editor frame has the first id of its range, and the connection closes when the
   test thread ends. `timer_required_paints_only_after_the_host_delivers_its_registered_timer`
   opened its editor with `open`, which maps (raises) the frame: WSLg's window manager (Weston's
   X window manager) took it over. The test closed the frame and its thread ended. About a
   millisecond later the XEmbed test's fixture connected for its plug (`tests/fixture/src/xembed_plug.rs`)
   and got the same range, so the plug got the frame's id. The window manager, still handling the
   closed frame, acted on that id and reparented the plug to its own window. What the test sees
   depends on when that lands: before the host detects the child, the host forgets it and never
   maps or notifies it (the mapped assertion fails); after the map and the notify, the host
   forgets it on the ReparentNotify, resizes the content for the fixture's `request_resize`, and
   the plug keeps its old size (the size assertion fails, the gate's signature). Evidence in WSL
   on WSLg, the whole crate's tests with one spinning process per CPU plus X clients, with
   uncommitted logging of every frame and plug id: the failures came at all three first
   assertions (mapped, notified, sized), and in 4 of 4 traced failures the plug's id was the
   frame id of the timer test's mapped and closed editor, 0.6 to 1.1 ms earlier, and the plug's
   parent was the window manager's own window. Plugs that got the id of a closed frame that had
   never been mapped (11 traced) passed. Reproduction counts on
   761d8baa: 0 of 20 runs of the test alone quiet, 0 of 20 loaded, 0 of 400 under heavier CPU and
   X client load, and 1 of 30 runs of the whole crate under load (the mapped assertion).
3. **Why checks missed it.** The test reads protocol state, not time, and its single runs never
   fail: the collision needs a sibling test in the same process that maps a top-level and closes
   it just before the plug connects, and a window manager that is behind. CI runs the editor
   tests on Xvfb with no window manager, where nothing acts on a reused id. #289 added the timer
   test with a raised editor next to this one.
4. **Regression test.** The timer test now opens its editor without raising it (`open_with`), so
   its frame is never managed, and the XEmbed test is the only test in the process that maps a
   top-level. Both tests document why. After the fix: 0 of 40 runs of the whole crate under the
   same load failed (1 of 30 before), and the test alone passed 20 of 20 quiet and 20 of 20
   loaded. A test that triggers the collision on purpose would depend on the window manager's
   schedule, so the evidence is the traced runs above, not a deterministic test. The hunt also
   found a latent E64 case in the same test, fixed with it: the host resizes and unmaps the plug
   with requests it only flushes, and the test read the plug's size from the fixture's connection
   right after the host's last pass. The test now round-trips the host connection before each
   readout. With the host's flushes withheld on purpose (an uncommitted change), the old test
   failed 10 of 10 at the size assertion on Xvfb and 1 of 1 on WSLg, and the new one passed 10 of
   10 on each.
5. **Gate class.** A test on a shared X display maps no top-level window unless its assertions
   need it mapped, and at most one test in a process maps one: a window manager that is still
   handling a closed window acts on whichever window gets its id next.
6. **Protocol change, owner, blind spot.** The rule is a bullet in
   `docs/contributing/evidence/gates.md` next to E64's, and in the two tests' docs. Owner: the
   editor window module's maintainer. Blind spot: no automated check; a candidate is an xtask scan
   for editor opens that raise (`.open(`, `raise()`) in Linux test code, allowing one per test
   binary. Ids reused across test processes (a previous binary's mapped windows) are not covered;
   no X test binary runs right before the clap-host lib tests today.

See also: [E64](E64-an-x11-window-was-handed-out-before-the-server-made-it.md) and
[E68](E68-an-xvfb-reset-dropped-a-test-connection.md), the earlier X11 test races.
